//! Process management syscalls
use crate::config::PAGE_SIZE;
use crate::mm::{MapPermission, PageTable, VirtAddr};
use crate::task::{
    change_program_brk, current_user_token, exit_current_and_run_next, get_syscall_count,
    mmap_current, munmap_current, suspend_current_and_run_next,
};
use crate::timer::get_time_us;

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

/// the `port` bit of `sys_mmap` asking for a readable mapping
const MMAP_PROT_READ: usize = 1 << 0;
/// the `port` bit of `sys_mmap` asking for a writable mapping
const MMAP_PROT_WRITE: usize = 1 << 1;
/// the `port` bit of `sys_mmap` asking for an executable mapping
const MMAP_PROT_EXEC: usize = 1 << 2;
/// the first virtual address that no user process may legitimately hold
///
/// SV39 only looks at the low 39 bits of a virtual address, so something like
/// `isize::MAX` wraps around onto the trap context and the trampoline instead
/// of falling off the end of the page table. Rejecting anything outside the
/// lower half keeps the page table walk honest.
const USER_MAX_VA: usize = 1 << 38;

/// Translate one byte of the calling process' address space into a pointer to
/// the physical byte that backs it.
///
/// Return `None` when the page is not mapped at all, or when its permission
/// does not allow the kind of access we are about to perform. The kernel must
/// never trap on a user pointer: a misbehaving application gets `-1` back
/// instead, and stays alive.
fn user_byte_ptr(token: usize, addr: usize, writable: bool) -> Option<*mut u8> {
    if addr >= USER_MAX_VA || addr.checked_add(1).is_none() {
        return None;
    }
    let page_table = PageTable::from_token(token);
    let pte = page_table.translate(VirtAddr::from(addr).floor())?;
    if !pte.is_valid() {
        return None;
    }
    let allowed = if writable {
        pte.writable()
    } else {
        pte.readable()
    };
    if !allowed {
        return None;
    }
    let offset = VirtAddr::from(addr).page_offset();
    Some(unsafe { pte.ppn().get_bytes_array().as_mut_ptr().add(offset) })
}

/// Read one byte from the calling process' address space.
fn user_read_u8(token: usize, addr: usize) -> Option<u8> {
    let ptr = user_byte_ptr(token, addr, false)?;
    Some(unsafe { *ptr })
}

/// Write one byte into the calling process' address space.
fn user_write_u8(token: usize, addr: usize, data: u8) -> bool {
    match user_byte_ptr(token, addr, true) {
        Some(ptr) => {
            unsafe {
                *ptr = data;
            }
            true
        }
        None => false,
    }
}

/// Copy `data` into the calling process' address space, byte by byte.
///
/// Doing it per byte keeps the copy correct when the buffer straddles a page
/// boundary, and it lets us bail out on the very first byte that is not
/// writable instead of touching memory the application does not own.
fn user_write_bytes(token: usize, addr: usize, data: &[u8]) -> bool {
    for (offset, byte) in data.iter().enumerate() {
        if !user_write_u8(token, addr + offset, *byte) {
            return false;
        }
    }
    true
}

/// task exits and submit an exit code
pub fn sys_exit(_exit_code: i32) -> ! {
    trace!("kernel: sys_exit");
    exit_current_and_run_next();
    panic!("Unreachable in sys_exit!");
}

/// current task gives up resources for other tasks
pub fn sys_yield() -> isize {
    trace!("kernel: sys_yield");
    suspend_current_and_run_next();
    0
}

/// get time with second and microsecond
///
/// The user pointer has to go through the page table: `ts` is a virtual
/// address, and the two words of [`TimeVal`] may even live on two pages.
pub fn sys_get_time(ts: *mut TimeVal, _tz: usize) -> isize {
    trace!("kernel: sys_get_time");
    let token = current_user_token();
    let us = get_time_us();
    let time_val = TimeVal {
        sec: us / 1_000_000,
        usec: us % 1_000_000,
    };
    let word = core::mem::size_of::<usize>();
    let mut buffer = [0u8; core::mem::size_of::<TimeVal>()];
    buffer[..word].copy_from_slice(&time_val.sec.to_ne_bytes());
    buffer[word..].copy_from_slice(&time_val.usec.to_ne_bytes());
    if user_write_bytes(token, ts as usize, &buffer) {
        0
    } else {
        -1
    }
}

/// trace syscall, defined by the ch3 lab and reworked for virtual memory
///
/// - `trace_request == 0`: read the byte at address `id`, return it
/// - `trace_request == 1`: write the low byte of `data` to address `id`
/// - `trace_request == 2`: return how many times the current task has called
///   the syscall numbered `id` (this very call included)
/// - anything else: return -1
pub fn sys_trace(trace_request: usize, id: usize, data: usize) -> isize {
    match trace_request {
        0 => match user_read_u8(current_user_token(), id) {
            Some(value) => value as isize,
            None => -1,
        },
        1 => {
            if user_write_u8(current_user_token(), id, data as u8) {
                0
            } else {
                -1
            }
        }
        2 => get_syscall_count(id),
        _ => -1,
    }
}

/// Map `[start, start + len)` of the calling process' address space on demand
///
/// `port` is a bit mask -- bit 0 readable, bit 1 writable, bit 2 executable --
/// and every other bit makes the request invalid. The range has to start on a
/// page boundary and must not overlap memory that is already mapped. Return 0
/// on success and -1 otherwise.
pub fn sys_mmap(start: usize, len: usize, port: usize) -> isize {
    trace!("kernel: sys_mmap");
    if len == 0 || !VirtAddr::from(start).aligned() {
        return -1;
    }
    let known_ports = MMAP_PROT_READ | MMAP_PROT_WRITE | MMAP_PROT_EXEC;
    if port == 0 || port & !known_ports != 0 {
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
    if mmap_current(start, len, permission) {
        0
    } else {
        -1
    }
}

/// Tear down the mapping `[start, start + len)` of the calling process
///
/// Both ends have to be page aligned, and the range must be exactly an area
/// that [`sys_mmap`] created earlier. Return 0 on success and -1 otherwise.
pub fn sys_munmap(start: usize, len: usize) -> isize {
    trace!("kernel: sys_munmap");
    if len == 0 || !VirtAddr::from(start).aligned() || len % PAGE_SIZE != 0 {
        return -1;
    }
    if start.checked_add(len).is_none() {
        return -1;
    }
    if munmap_current(start, len) {
        0
    } else {
        -1
    }
}
/// change data segment size
pub fn sys_sbrk(size: i32) -> isize {
    trace!("kernel: sys_sbrk");
    if let Some(old_brk) = change_program_brk(size) {
        old_brk as isize
    } else {
        -1
    }
}
