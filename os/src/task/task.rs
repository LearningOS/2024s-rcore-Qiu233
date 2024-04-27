//! Types related to task management

use alloc::collections::BTreeMap;

use super::TaskContext;

/// The task control block (TCB) of a task.
#[derive(Clone)]
pub struct TaskControlBlock {
    /// The task status in it's lifecycle
    pub task_status: TaskStatus,
    /// The task context
    pub task_cx: TaskContext,
    /// task info block
    pub task_info: TaskInfoBlock
}

#[derive(Clone)]
pub struct TaskInfoBlock {
    /// Whether the task has already been dispatched
    pub dispatched: bool,
    /// Timestamp in ms of the first time this task being dispatched
    pub dispatched_time: usize,
    /// Syscall times
    pub syscall_times: BTreeMap<usize, u32>
    // pub syscall_times: [u32; MAX_SYSCALL_NUM] // will take too much more space than needed
}

impl TaskInfoBlock {
    /// empty info block
    pub fn new() -> Self {
        TaskInfoBlock {
            dispatched: false,
            dispatched_time: 0,
            syscall_times: BTreeMap::new()
        }
    }
    /// Set the timestamp to now if it's the first to be dispatched
    pub fn set_timestamp_if_first_dispatched(&mut self) {
        if !self.dispatched {
            self.dispatched_time = crate::timer::get_time_ms();
            self.dispatched = true;
        }
    }
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
