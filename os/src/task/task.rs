//! Types related to task management

use super::TaskContext;
use crate::config::MAX_SYSCALL_NUM;

/// The task control block (TCB) of a task.
#[derive(Copy, Clone)]
pub struct TaskControlBlock {
    /// The task status in it's lifecycle
    pub task_status: TaskStatus,
    /// The task context
    pub task_cx: TaskContext,
    /// How many times each syscall has been called by this task, indexed by syscall id.
    ///
    /// This is what `sys_trace` with `trace_request == 2` reports back to the user.
    /// A `u16` counter saturated at `u16::MAX` is plenty for diagnostics and keeps
    /// the TCB small enough that the whole task table still fits on the boot stack.
    pub syscall_count: [u16; MAX_SYSCALL_NUM],
}

/// The status of a task
#[derive(Copy, Clone, PartialEq)]
pub enum TaskStatus {
    /// uninitialized
    UnInit,
    /// ready to run
    Ready,
    /// running
    Running,
    /// exited
    Exited,
}
