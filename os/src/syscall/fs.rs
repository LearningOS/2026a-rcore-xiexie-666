//! File and filesystem-related syscalls
use crate::fs::{open_file, OpenFlags, Stat, ROOT_INODE};
use crate::mm::{translated_byte_buffer, translated_str, UserBuffer};
use crate::task::{current_task, current_user_token};

pub fn sys_write(fd: usize, buf: *const u8, len: usize) -> isize {
    trace!("kernel:pid[{}] sys_write", current_task().unwrap().pid.0);
    let token = current_user_token();
    let task = current_task().unwrap();
    let inner = task.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return -1;
    }
    if let Some(file) = &inner.fd_table[fd] {
        if !file.writable() {
            return -1;
        }
        let file = file.clone();
        // release current task TCB manually to avoid multi-borrow
        drop(inner);
        file.write(UserBuffer::new(translated_byte_buffer(token, buf, len))) as isize
    } else {
        -1
    }
}

pub fn sys_read(fd: usize, buf: *const u8, len: usize) -> isize {
    trace!("kernel:pid[{}] sys_read", current_task().unwrap().pid.0);
    let token = current_user_token();
    let task = current_task().unwrap();
    let inner = task.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return -1;
    }
    if let Some(file) = &inner.fd_table[fd] {
        let file = file.clone();
        if !file.readable() {
            return -1;
        }
        // release current task TCB manually to avoid multi-borrow
        drop(inner);
        trace!("kernel: sys_read .. file.read");
        file.read(UserBuffer::new(translated_byte_buffer(token, buf, len))) as isize
    } else {
        -1
    }
}

pub fn sys_open(path: *const u8, flags: u32) -> isize {
    trace!("kernel:pid[{}] sys_open", current_task().unwrap().pid.0);
    let task = current_task().unwrap();
    let token = current_user_token();
    let path = translated_str(token, path);
    if let Some(inode) = open_file(path.as_str(), OpenFlags::from_bits(flags).unwrap()) {
        let mut inner = task.inner_exclusive_access();
        let fd = inner.alloc_fd();
        inner.fd_table[fd] = Some(inode);
        fd as isize
    } else {
        -1
    }
}

pub fn sys_close(fd: usize) -> isize {
    trace!("kernel:pid[{}] sys_close", current_task().unwrap().pid.0);
    let task = current_task().unwrap();
    let mut inner = task.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return -1;
    }
    if inner.fd_table[fd].is_none() {
        return -1;
    }
    inner.fd_table[fd].take();
    0
}

/// Fill the `Stat` structure of the file whose descriptor is `fd`.
///
/// The structure is written through the page table of the current process, so
/// it works even if it is split by two pages.
pub fn sys_fstat(fd: usize, st: *mut Stat) -> isize {
    trace!("kernel:pid[{}] sys_fstat", current_task().unwrap().pid.0);
    let token = current_user_token();
    let task = current_task().unwrap();
    let file = {
        let inner = task.inner_exclusive_access();
        if fd >= inner.fd_table.len() {
            return -1;
        }
        match &inner.fd_table[fd] {
            Some(file) => file.clone(),
            None => return -1,
        }
    };
    let mut stat = Stat::new();
    if file.stat(&mut stat) != 0 {
        return -1;
    }
    let len = core::mem::size_of::<Stat>();
    let src = unsafe { core::slice::from_raw_parts(&stat as *const Stat as *const u8, len) };
    let mut buffers = translated_byte_buffer(token, st as *const u8, len);
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

/// Create a hard link: `new_name` refers to the same inode as `old_name`.
///
/// Both paths are relative to the root directory, and the file descriptor
/// arguments of the real `linkat` are ignored for simplicity.
pub fn sys_linkat(old_name: *const u8, new_name: *const u8) -> isize {
    trace!("kernel:pid[{}] sys_linkat", current_task().unwrap().pid.0);
    let token = current_user_token();
    let old_name = translated_str(token, old_name);
    let new_name = translated_str(token, new_name);
    if ROOT_INODE.link(old_name.as_str(), new_name.as_str()).is_some() {
        0
    } else {
        -1
    }
}

/// Remove the directory entry `name` from the root directory.
///
/// The inode and its data blocks are reclaimed when the last link is removed.
pub fn sys_unlinkat(name: *const u8) -> isize {
    trace!("kernel:pid[{}] sys_unlinkat", current_task().unwrap().pid.0);
    let token = current_user_token();
    let name = translated_str(token, name);
    if ROOT_INODE.unlink(name.as_str()).is_some() {
        0
    } else {
        -1
    }
}
