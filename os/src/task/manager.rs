//!Implementation of [`TaskManager`]
use super::TaskControlBlock;
use crate::sync::UPSafeCell;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use lazy_static::*;
///A ready queue of `TaskControlBlock` that is thread-safe
///
/// The key is `(stride, pid)`: `stride` orders the processes, and `pid`
/// keeps the key unique when two processes happen to have the same stride.
pub struct TaskManager {
    ready_queue: BTreeMap<(usize, usize), Arc<TaskControlBlock>>,
}

/// A stride scheduler.
///
/// The process with the smallest stride is always selected, and its stride is
/// advanced by `BIG_STRIDE / priority` right after it is scheduled. In this
/// way the cpu time a process obtains is proportional to its priority.
impl TaskManager {
    ///Creat an empty TaskManager
    pub fn new() -> Self {
        Self {
            ready_queue: BTreeMap::new(),
        }
    }
    /// Add process back to ready queue
    pub fn add(&mut self, task: Arc<TaskControlBlock>) {
        let stride = task.inner_exclusive_access().stride;
        self.ready_queue.insert((stride, task.pid.0), task);
    }
    /// Take the process with the smallest stride out of the ready queue
    pub fn fetch(&mut self) -> Option<Arc<TaskControlBlock>> {
        let (_, task) = self.ready_queue.pop_first()?;
        {
            let mut inner = task.inner_exclusive_access();
            let pass = inner.pass();
            inner.stride += pass;
        }
        Some(task)
    }
}

lazy_static! {
    /// TASK_MANAGER instance through lazy_static!
    pub static ref TASK_MANAGER: UPSafeCell<TaskManager> =
        unsafe { UPSafeCell::new(TaskManager::new()) };
}

/// Add process to ready queue
pub fn add_task(task: Arc<TaskControlBlock>) {
    //trace!("kernel: TaskManager::add_task");
    TASK_MANAGER.exclusive_access().add(task);
}

/// Take a process out of the ready queue
pub fn fetch_task() -> Option<Arc<TaskControlBlock>> {
    //trace!("kernel: TaskManager::fetch_task");
    TASK_MANAGER.exclusive_access().fetch()
}
