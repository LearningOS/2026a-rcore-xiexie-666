use crate::config::{DEADLOCK_SPACE_MUTEX, DEADLOCK_SPACE_SEMAPHORE};
use crate::sync::{Condvar, Mutex, MutexBlocking, MutexSpin, Semaphore};
use crate::task::{block_current_and_run_next, current_process, current_task};
use crate::timer::{add_timer, get_time_ms};
use alloc::sync::Arc;
use alloc::vec::Vec;

/// Returned by `mutex_lock` / `semaphore_down` when granting the request would
/// make the process deadlock.
const DEADLOCK_DETECTED: isize = -0xDEAD;

/// tid of the task that is running right now
fn current_tid() -> usize {
    current_task()
        .unwrap()
        .inner_exclusive_access()
        .res
        .as_ref()
        .unwrap()
        .tid
}

/// sleep syscall
pub fn sys_sleep(ms: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_sleep",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let expire_ms = get_time_ms() + ms;
    let task = current_task().unwrap();
    add_timer(expire_ms, task);
    block_current_and_run_next();
    0
}
/// mutex create syscall
pub fn sys_mutex_create(blocking: bool) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_mutex_create",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    let mutex: Option<Arc<dyn Mutex>> = if !blocking {
        Some(Arc::new(MutexSpin::new()))
    } else {
        Some(Arc::new(MutexBlocking::new()))
    };
    let mut process_inner = process.inner_exclusive_access();
    if let Some(id) = process_inner
        .mutex_list
        .iter()
        .enumerate()
        .find(|(_, item)| item.is_none())
        .map(|(id, _)| id)
    {
        process_inner.mutex_list[id] = mutex;
        id as isize
    } else {
        process_inner.mutex_list.push(mutex);
        process_inner.mutex_list.len() as isize - 1
    }
}
/// mutex lock syscall
pub fn sys_mutex_lock(mutex_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_mutex_lock",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let tid = current_tid();
    let process = current_process();
    let mutex = {
        let mut process_inner = process.inner_exclusive_access();
        let mutex = Arc::clone(process_inner.mutex_list[mutex_id].as_ref().unwrap());
        if process_inner.deadlock_detector.is_enabled() {
            let resources = process_inner.mutex_list.len();
            let available: Vec<usize> = process_inner
                .mutex_list
                .iter()
                .map(|item| item.as_ref().map(|m| m.available()).unwrap_or(0))
                .collect();
            // only a request that cannot be granted right now can deadlock
            if available[mutex_id] == 0 {
                let threads = process_inner.tasks.len();
                let blocked = process_inner.blocked_flags();
                let safe = process_inner.deadlock_detector.is_safe(
                    DEADLOCK_SPACE_MUTEX,
                    threads,
                    resources,
                    &available,
                    &blocked,
                    tid,
                    mutex_id,
                );
                if !safe {
                    return DEADLOCK_DETECTED;
                }
                process_inner.deadlock_detector.set_pending(
                    DEADLOCK_SPACE_MUTEX,
                    tid,
                    mutex_id,
                    threads,
                    resources,
                );
            }
        }
        mutex
    };
    mutex.lock();
    let mut process_inner = process.inner_exclusive_access();
    if process_inner.deadlock_detector.is_enabled() {
        let resources = process_inner.mutex_list.len();
        let threads = process_inner.tasks.len();
        process_inner.deadlock_detector.on_acquire(
            DEADLOCK_SPACE_MUTEX,
            tid,
            mutex_id,
            threads,
            resources,
        );
    }
    0
}
/// mutex unlock syscall
pub fn sys_mutex_unlock(mutex_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_mutex_unlock",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let tid = current_tid();
    let process = current_process();
    let mutex = {
        let mut process_inner = process.inner_exclusive_access();
        if process_inner.deadlock_detector.is_enabled() {
            process_inner.deadlock_detector.on_release(
                DEADLOCK_SPACE_MUTEX,
                tid,
                mutex_id,
            );
        }
        Arc::clone(process_inner.mutex_list[mutex_id].as_ref().unwrap())
    };
    mutex.unlock();
    0
}
/// semaphore create syscall
pub fn sys_semaphore_create(res_count: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_semaphore_create",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    let mut process_inner = process.inner_exclusive_access();
    let id = if let Some(id) = process_inner
        .semaphore_list
        .iter()
        .enumerate()
        .find(|(_, item)| item.is_none())
        .map(|(id, _)| id)
    {
        process_inner.semaphore_list[id] = Some(Arc::new(Semaphore::new(res_count)));
        id
    } else {
        process_inner
            .semaphore_list
            .push(Some(Arc::new(Semaphore::new(res_count))));
        process_inner.semaphore_list.len() - 1
    };
    id as isize
}
/// semaphore up syscall
pub fn sys_semaphore_up(sem_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_semaphore_up",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let tid = current_tid();
    let process = current_process();
    let sem = {
        let mut process_inner = process.inner_exclusive_access();
        if process_inner.deadlock_detector.is_enabled() {
            process_inner
                .deadlock_detector
                .on_release(DEADLOCK_SPACE_SEMAPHORE, tid, sem_id);
        }
        Arc::clone(process_inner.semaphore_list[sem_id].as_ref().unwrap())
    };
    sem.up();
    0
}
/// semaphore down syscall
pub fn sys_semaphore_down(sem_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_semaphore_down",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let tid = current_tid();
    let process = current_process();
    let sem = {
        let mut process_inner = process.inner_exclusive_access();
        let sem = Arc::clone(process_inner.semaphore_list[sem_id].as_ref().unwrap());
        if process_inner.deadlock_detector.is_enabled() {
            let resources = process_inner.semaphore_list.len();
            let available: Vec<usize> = process_inner
                .semaphore_list
                .iter()
                .map(|item| item.as_ref().map(|s| s.available()).unwrap_or(0))
                .collect();
            // only a request that cannot be granted right now can deadlock
            if available[sem_id] == 0 {
                let threads = process_inner.tasks.len();
                let blocked = process_inner.blocked_flags();
                let safe = process_inner.deadlock_detector.is_safe(
                    DEADLOCK_SPACE_SEMAPHORE,
                    threads,
                    resources,
                    &available,
                    &blocked,
                    tid,
                    sem_id,
                );
                if !safe {
                    return DEADLOCK_DETECTED;
                }
                process_inner.deadlock_detector.set_pending(
                    DEADLOCK_SPACE_SEMAPHORE,
                    tid,
                    sem_id,
                    threads,
                    resources,
                );
            }
        }
        sem
    };
    sem.down();
    let mut process_inner = process.inner_exclusive_access();
    if process_inner.deadlock_detector.is_enabled() {
        let resources = process_inner.semaphore_list.len();
        let threads = process_inner.tasks.len();
        process_inner.deadlock_detector.on_acquire(
            DEADLOCK_SPACE_SEMAPHORE,
            tid,
            sem_id,
            threads,
            resources,
        );
    }
    0
}
/// condvar create syscall
pub fn sys_condvar_create() -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_condvar_create",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    let mut process_inner = process.inner_exclusive_access();
    let id = if let Some(id) = process_inner
        .condvar_list
        .iter()
        .enumerate()
        .find(|(_, item)| item.is_none())
        .map(|(id, _)| id)
    {
        process_inner.condvar_list[id] = Some(Arc::new(Condvar::new()));
        id
    } else {
        process_inner
            .condvar_list
            .push(Some(Arc::new(Condvar::new())));
        process_inner.condvar_list.len() - 1
    };
    id as isize
}
/// condvar signal syscall
pub fn sys_condvar_signal(condvar_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_condvar_signal",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    let process_inner = process.inner_exclusive_access();
    let condvar = Arc::clone(process_inner.condvar_list[condvar_id].as_ref().unwrap());
    drop(process_inner);
    condvar.signal();
    0
}
/// condvar wait syscall
pub fn sys_condvar_wait(condvar_id: usize, mutex_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_condvar_wait",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    let process_inner = process.inner_exclusive_access();
    let condvar = Arc::clone(process_inner.condvar_list[condvar_id].as_ref().unwrap());
    let mutex = Arc::clone(process_inner.mutex_list[mutex_id].as_ref().unwrap());
    drop(process_inner);
    condvar.wait(mutex);
    0
}
/// enable deadlock detection syscall
///
/// `enabled` must be 0 (disable) or 1 (enable); anything else is rejected with
/// -1. The detection itself is done inside `sys_mutex_lock` and
/// `sys_semaphore_down`, so enabling it only flips a flag of this process.
pub fn sys_enable_deadlock_detect(enabled: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_enable_deadlock_detect",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    if enabled > 1 {
        return -1;
    }
    let process = current_process();
    process
        .inner_exclusive_access()
        .deadlock_detector
        .set_enabled(enabled == 1);
    0
}
