//! Types related to task management
use alloc::collections::BTreeMap;

use super::TaskContext;
use crate::config::TRAP_CONTEXT_BASE;
use crate::mm::{
    kernel_stack_position, MapPermission, MemorySet, PhysPageNum, VirtAddr, KERNEL_SPACE,
};
use crate::trap::{trap_handler, TrapContext};

/// Holds task info. <br/>
pub struct TaskInfoBlock {
    /// Whether the task has already been dispatched
    pub dispatched: bool,
    /// Timestamp in ms of the first time this task being dispatched
    pub dispatched_time: usize,
    /// Syscall times
    pub syscall_times: BTreeMap<usize, u32>
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

/// The task control block (TCB) of a task.
pub struct TaskControlBlock {
    /// Save task context
    pub task_cx: TaskContext,

    /// task info block
    pub task_info: TaskInfoBlock,

    /// Maintain the execution status of the current process
    pub task_status: TaskStatus,

    /// Application address space
    pub memory_set: MemorySet,

    /// The phys page number of trap context
    pub trap_cx_ppn: PhysPageNum,

    /// The size(top addr) of program which is loaded from elf file
    pub base_size: usize,

    /// Heap bottom
    pub heap_bottom: usize,

    /// Program break
    pub program_brk: usize,
}

impl TaskControlBlock {
    /// get the trap context
    pub fn get_trap_cx(&self) -> &'static mut TrapContext {
        self.trap_cx_ppn.get_mut()
    }
    /// get the user token
    pub fn get_user_token(&self) -> usize {
        self.memory_set.token()
    }
    /// Based on the elf info in program, build the contents of task in a new address space
    pub fn new(elf_data: &[u8], app_id: usize) -> Self {
        // memory_set with elf program headers/trampoline/trap context/user stack
        let segs = MemorySet::from_elf(elf_data);
        assert!(segs.is_ok(), "failed to allocate memory for program, err={}", segs.err().unwrap());
        let (mut memory_set, user_sp, entry_point) = segs.unwrap();
        let trap_cx_ppn = memory_set
            .translate(VirtAddr::from(TRAP_CONTEXT_BASE).into())
            .unwrap()
            .ppn();
        let task_status = TaskStatus::Ready;
        // map a kernel-stack in kernel space
        let (kernel_stack_bottom, kernel_stack_top) = kernel_stack_position(app_id);
        let kernel_stack_alloc = KERNEL_SPACE.exclusive_access().insert_framed_area_strict(
            kernel_stack_bottom.into(),
            kernel_stack_top.into(),
            MapPermission::R | MapPermission::W,
        );
        assert!(kernel_stack_alloc.is_ok(), "failed to allocate memory for kernel stack for appid = {}, err = {}", app_id, kernel_stack_alloc.err().unwrap());
        let task_control_block = Self {
            task_status,
            task_cx: TaskContext::goto_trap_return(kernel_stack_top),
            task_info: TaskInfoBlock::new(),
            memory_set,
            trap_cx_ppn,
            base_size: user_sp,
            heap_bottom: user_sp,
            program_brk: user_sp,
        };
        // prepare TrapContext in user space
        let trap_cx = task_control_block.get_trap_cx();
        *trap_cx = TrapContext::app_init_context(
            entry_point,
            user_sp,
            KERNEL_SPACE.exclusive_access().token(),
            kernel_stack_top,
            trap_handler as usize,
        );
        task_control_block
    }
    /// change the location of the program break. return None if failed.
    pub fn change_program_brk(&mut self, size: i32) -> Option<usize> {
        let old_break = self.program_brk;
        let new_brk = self.program_brk as isize + size as isize;
        if new_brk < self.heap_bottom as isize {
            return None;
        }
        let result = if size < 0 {
            self.memory_set
                .shrink_to(VirtAddr(self.heap_bottom), VirtAddr(new_brk as usize))
        } else {
            self.memory_set
                .append_to(VirtAddr(self.heap_bottom), VirtAddr(new_brk as usize))
        };
        if result.is_ok() {
            self.program_brk = new_brk as usize;
            Some(old_break)
        } else {
            None
        }
    }
}

#[derive(Copy, Clone, PartialEq)]
/// task status: UnInit, Ready, Running, Exited
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
