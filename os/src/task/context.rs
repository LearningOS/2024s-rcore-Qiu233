//! Implementation of [`TaskContext`]
use core::sync::atomic::AtomicUsize;

use crate::trap::trap_return;

#[repr(C)]
/// task context structure containing some registers
pub struct TaskContext {
    /// Ret position after task switching
    ra: usize,
    /// Stack pointer
    sp: usize,
    /// s0-11 register, callee saved
    s: [usize; 12],
    /// TaskContext derives **NO** meaningful conclusion until this lock is acquired.<br/>
    /// Do **NOT** use this in Rust code. It's intended for `__switch` synchronization.<br/>
    /// Use `AtomicUsize` instead of `usize` so the compiler will align it to be atomically accessed.
    lock: AtomicUsize
}

impl TaskContext {
    /// Create a new empty task context
    pub fn zero_init() -> Self {
        Self {
            ra: 0,
            sp: 0,
            s: [0; 12],
            lock: AtomicUsize::new(0)
        }
    }
    /// Create a new task context with a trap return addr and a kernel stack pointer
    pub fn goto_trap_return(kstack_ptr: usize) -> Self {
        Self {
            ra: trap_return as usize,
            sp: kstack_ptr,
            s: [0; 12],
            lock: AtomicUsize::new(0)
        }
    }
}
