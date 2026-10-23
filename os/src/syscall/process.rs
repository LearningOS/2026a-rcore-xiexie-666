//! Process management syscalls
//!
use alloc::sync::Arc;

use crate::{
    config::PAGE_SIZE,
    fs::{open_file, OpenFlags},
    loader::get_app_data_by_name,
    mm::{translated_byte_buffer, translated_refmut, translated_str, MapPermission, VirtAddr},
    task::{
        add_task, current_task, current_user_token, exit_current_and_run_next,
        suspend_current_and_run_next,
    },
    timer::get_time_us,
};

/// the `prot` bit meaning of `sys_mmap`: the area is readable
const MMAP_PROT_READ: usize = 1 << 0;
/// the `prot` bit meaning of `sys_mmap`: the area is writable
const MMAP_PROT_WRITE: usize = 1 << 1;
/// the `prot` bit meaning of `sys_mmap`: the area is executable
const MMAP_PROT_EXEC: usize = 1 << 2;

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

pub fn sys_exit(exit_code: i32) -> ! {
    trace!("kernel:pid[{}] sys_exit", current_task().unwrap().pid.0);
    exit_current_and_run_next(exit_code);
    panic!("Unreachable in sys_exit!");
}

pub fn sys_yield() -> isize {
    //trace!("kernel: sys_yield");
    suspend_current_and_run_next();
    0
}

pub fn sys_getpid() -> isize {
    trace!("kernel: sys_getpid pid:{}", current_task().unwrap().pid.0);
    current_task().unwrap().pid.0 as isize
}

pub fn sys_fork() -> isize {
    trace!("kernel:pid[{}] sys_fork", current_task().unwrap().pid.0);
    let current_task = current_task().unwrap();
    let new_task = current_task.fork();
    let new_pid = new_task.pid.0;
    // modify trap context of new_task, because it returns immediately after switching
    let trap_cx = new_task.inner_exclusive_access().get_trap_cx();
    // we do not have to move to next instruction since we have done it before
    // for child process, fork returns 0
    trap_cx.x[10] = 0;
    // add new task to scheduler
    add_task(new_task);
    new_pid as isize
}

pub fn sys_exec(path: *const u8) -> isize {
    trace!("kernel:pid[{}] sys_exec", current_task().unwrap().pid.0);
    let token = current_user_token();
    let path = translated_str(token, path);
    if let Some(app_inode) = open_file(path.as_str(), OpenFlags::RDONLY) {
        let all_data = app_inode.read_all();
        let task = current_task().unwrap();
        task.exec(all_data.as_slice());
        0
    } else {
        -1
    }
}

/// If there is not a child process whose pid is same as given, return -1.
/// Else if there is a child process but it is still running, return -2.
pub fn sys_waitpid(pid: isize, exit_code_ptr: *mut i32) -> isize {
    //trace!("kernel: sys_waitpid");
    let task = current_task().unwrap();
    // find a child process

    // ---- access current PCB exclusively
    let mut inner = task.inner_exclusive_access();
    if !inner
        .children
        .iter()
        .any(|p| pid == -1 || pid as usize == p.getpid())
    {
        return -1;
        // ---- release current PCB
    }
    let pair = inner.children.iter().enumerate().find(|(_, p)| {
        // ++++ temporarily access child PCB exclusively
        p.inner_exclusive_access().is_zombie() && (pid == -1 || pid as usize == p.getpid())
        // ++++ release child PCB
    });
    if let Some((idx, _)) = pair {
        let child = inner.children.remove(idx);
        // confirm that child will be deallocated after being removed from children list
        assert_eq!(Arc::strong_count(&child), 1);
        let found_pid = child.getpid();
        // ++++ temporarily access child PCB exclusively
        let exit_code = child.inner_exclusive_access().exit_code;
        // ++++ release child PCB
        *translated_refmut(inner.memory_set.token(), exit_code_ptr) = exit_code;
        found_pid as isize
    } else {
        -2
    }
    // ---- release current PCB automatically
}

/// get time with second and microsecond
///
/// The `TimeVal` is written through the page table of the current process,
/// byte by byte, so it works even if the struct is split by two pages.
pub fn sys_get_time(ts: *mut TimeVal, _tz: usize) -> isize {
    trace!("kernel:pid[{}] sys_get_time", current_task().unwrap().pid.0);
    let us = get_time_us();
    let time_val = TimeVal {
        sec: us / 1_000_000,
        usec: us % 1_000_000,
    };
    let len = core::mem::size_of::<TimeVal>();
    let src = unsafe { core::slice::from_raw_parts(&time_val as *const TimeVal as *const u8, len) };
    let mut buffers = translated_byte_buffer(current_user_token(), ts as *const u8, len);
    let mut copied = 0;
    for buffer in buffers.iter_mut() {
        let n = core::cmp::min(buffer.len(), len - copied);
        buffer[..n].copy_from_slice(&src[copied..copied + n]);
        copied += n;
    }
    if copied == len {
        0
    } else {
        -1
    }
}

/// Implement mmap.
///
/// `start` must be page aligned, `len` need not be, and `prot` is a bitmap of
/// read/write/execute. Return 0 on success, -1 otherwise.
pub fn sys_mmap(start: usize, len: usize, port: usize) -> isize {
    trace!("kernel:pid[{}] sys_mmap", current_task().unwrap().pid.0);
    let start_va: VirtAddr = start.into();
    if len == 0 || !start_va.aligned() || port == 0 || port & !0x7 != 0 {
        return -1;
    }
    if start.checked_add(len).is_none() {
        return -1;
    }
    let mut permission = MapPermission::U;
    if port & MMAP_PROT_READ != 0 {
        permission |= MapPermission::R;
    }
    if port & MMAP_PROT_WRITE != 0 {
        permission |= MapPermission::W;
    }
    if port & MMAP_PROT_EXEC != 0 {
        permission |= MapPermission::X;
    }
    let task = current_task().unwrap();
    if task.mmap(start, len, permission) {
        0
    } else {
        -1
    }
}

/// Implement munmap.
///
/// The area `[start, start + len)` must exactly match an area which has been
/// created by `sys_mmap`, so both `start` and `len` must be page aligned.
pub fn sys_munmap(start: usize, len: usize) -> isize {
    trace!("kernel:pid[{}] sys_munmap", current_task().unwrap().pid.0);
    let start_va: VirtAddr = start.into();
    if len == 0 || !start_va.aligned() || len % PAGE_SIZE != 0 {
        return -1;
    }
    let task = current_task().unwrap();
    if task.munmap(start, len) {
        0
    } else {
        -1
    }
}

/// change data segment size
pub fn sys_sbrk(size: i32) -> isize {
    trace!("kernel:pid[{}] sys_sbrk", current_task().unwrap().pid.0);
    if let Some(old_brk) = current_task().unwrap().change_program_brk(size) {
        old_brk as isize
    } else {
        -1
    }
}

/// Implement spawn.
///
/// The new process runs `path` from scratch: the address space of the parent
/// is not copied, which is what makes `spawn` much cheaper than `fork`+`exec`.
pub fn sys_spawn(path: *const u8) -> isize {
    trace!("kernel:pid[{}] sys_spawn", current_task().unwrap().pid.0);
    let token = current_user_token();
    let path = translated_str(token, path);
    if let Some(data) = get_app_data_by_name(path.as_str()) {
        let task = current_task().unwrap();
        let new_task = task.spawn(data);
        let new_pid = new_task.pid.0;
        add_task(new_task);
        new_pid as isize
    } else {
        -1
    }
}

/// Set task priority. The priority must not be smaller than 2.
///
/// Return the new priority on success, -1 otherwise.
pub fn sys_set_priority(prio: isize) -> isize {
    trace!(
        "kernel:pid[{}] sys_set_priority [{}]",
        current_task().unwrap().pid.0,
        prio
    );
    if prio < 2 {
        return -1;
    }
    let task = current_task().unwrap();
    task.set_priority(prio as usize);
    prio
}
