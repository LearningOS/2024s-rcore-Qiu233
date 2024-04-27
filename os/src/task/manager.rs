//! Implementation of [`TaskManager`]
//!
//! It is only used to manage processes and schedule process based on ready queue.
//! Other CPU process monitoring functions are in Processor.

use super::TaskControlBlock;
use alloc::collections::{BTreeMap, LinkedList};
use alloc::sync::Arc;
use lazy_static::*;
use spin::Mutex;

pub struct TaskManager {
    ready_queue: LinkedList<Arc<TaskControlBlock>>,
}

const BIG_STRIDE: usize = 0x100000;

/// A simple FIFO scheduler.
impl TaskManager {
    ///Creat an empty TaskManager
    pub fn new() -> Self {
        Self {
            ready_queue: LinkedList::new(),
        }
    }
    fn min_stride_pos(&self) -> Option<usize> {
        let (pos, _) = self.ready_queue
            .iter()
            .enumerate()
            .min_by_key(|(_, x)|x.inner_exclusive_access().stride)?;
        Some(pos)
    }
    /// Unify by stride for all TCB in list as well as the argument
    fn unify_stride(&mut self, task: &Arc<TaskControlBlock>) {
        let mut task = task.inner_exclusive_access();
        let pass = BIG_STRIDE / (task.priority as usize);
        if task.stride.wrapping_add(pass) > task.stride { // does not overflow
            task.stride += pass;
            return;
        }
        // overflow, must be unified
        let min_pos = self.min_stride_pos();
        match min_pos {
            None => {
                task.stride = 0; // reset stride if there are no other pending tasks
            }
            Some(min_pos) => {
                let min = self.ready_queue.iter().nth(min_pos).unwrap().inner_exclusive_access().stride;
                let min = core::cmp::min(min, task.stride);
                for x in self.ready_queue.iter_mut() {
                    x.inner_exclusive_access().stride -= min;
                }
                task.stride = (task.stride - min) + pass; // this cannot overflow in any way, or else `BIG_STRIDE` must be chosen smaller
            }
        }
    }
    /// Add process back to ready queue, with stride accumulated
    pub fn add(&mut self, task: Arc<TaskControlBlock>) {
        self.unify_stride(&task);
        self.ready_queue.push_back(task);
    }
    /// Take a process out of the ready queue
    pub fn fetch(&mut self) -> Option<Arc<TaskControlBlock>> {
        self.ready_queue.pop_front()
    }
}

lazy_static! {
    /// TASK_MANAGER instance through lazy_static!
    pub static ref TASK_MANAGER: Mutex<TaskManager> = Mutex::new(TaskManager::new());
    /// PID2PCB instance (map of pid to pcb)
    pub static ref PID2TCB: Mutex<BTreeMap<usize, Arc<TaskControlBlock>>> = Mutex::new(BTreeMap::new());
}

/// Add process to ready queue
pub fn add_task(task: Arc<TaskControlBlock>) {
	//trace!("kernel: TaskManager::add_task");
    PID2TCB
        .lock()
        .insert(task.getpid(), Arc::clone(&task));
    TASK_MANAGER.lock().add(task);
}

/// Take a process out of the ready queue
pub fn fetch_task() -> Option<Arc<TaskControlBlock>> {
	//trace!("kernel: TaskManager::fetch_task");
    TASK_MANAGER.lock().fetch()
}

/// Get process by pid
pub fn pid2task(pid: usize) -> Option<Arc<TaskControlBlock>> {
    let map = PID2TCB.lock();
    map.get(&pid).map(Arc::clone)
}

/// Remove item(pid, _some_pcb) from PDI2PCB map (called by exit_current_and_run_next)
pub fn remove_from_pid2task(pid: usize) {
    let mut map = PID2TCB.lock();
    if map.remove(&pid).is_none() {
        panic!("cannot find pid {} in pid2task!", pid);
    }
}
