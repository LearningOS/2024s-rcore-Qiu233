//! Process management syscalls
//!

use crate::{
    config::{MAX_SYSCALL_NUM, PAGE_SIZE}, fs::{open_file, OpenFlags}, mm::{translated_refmut, translated_str, MapPermission, UserBuffer, VirtAddr}, task::{
        add_task, current_task, current_user_token, exit_current_and_run_next,
        suspend_current_and_run_next, TaskStatus,
    }
};

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

/// Task information
#[allow(dead_code)]
pub struct TaskInfo {
    /// Task status in it's life cycle
    status: TaskStatus,
    /// The numbers of syscall called by task
    syscall_times: [u32; MAX_SYSCALL_NUM],
    /// Total running time of task
    time: usize,
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
    // note: this modification is to avoid deadlock, please see `exit_current_and_run_next` for reason
    let idx = loop {
        let locks = inner.children.iter().map(|x|(x.clone(), x.try_lock())).collect::<alloc::vec::Vec<_>>();
        if locks.iter().any(|(_, lock)|lock.is_none()) {
            drop(locks);
            drop(inner);
            inner = task.inner_exclusive_access();
            continue;
        }
        break locks
            .into_iter()
            .map(|(x, p)|(x, p.unwrap()))
            .enumerate()
            .find(|(_, (x, p))| p.is_zombie() && (pid == -1 || pid as usize == x.getpid()))
            .map(|x|x.0);
    };
    if let Some(idx) = idx {
        let child = inner.children.remove(idx);

        // confirm that child will be deallocated after being removed from children list
        // Qiu: this is false when there are multiple harts,
        // another hart might have set process status to `Zombie` while the `Arc` has yet to be dropped.
        // assert_eq!(Arc::strong_count(&child), 1);

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


/// YOUR JOB: get time with second and microsecond
/// HINT: You might reimplement it with virtual memory management.
/// HINT: What if [`TimeVal`] is splitted by two pages ?
pub fn sys_get_time(ts: *mut TimeVal, _tz: usize) -> isize {
    trace!(
        "kernel:pid[{}] sys_get_time",
        current_task().unwrap().pid.0
    );
    let us = crate::timer::get_time_us();
    UserBuffer::from_mut_ptr(current_user_token(), ts).copy_from(&TimeVal {
        sec: us / 1_000_000,
        usec: us % 1_000_000,
    });
    0
}

/// YOUR JOB: Finish sys_task_info to pass testcases
/// HINT: You might reimplement it with virtual memory management.
/// HINT: What if [`TaskInfo`] is splitted by two pages ?
pub fn sys_task_info(ti: *mut TaskInfo) -> isize {
    trace!(
        "kernel:pid[{}] sys_task_info NOT IMPLEMENTED",
        current_task().unwrap().pid.0
    );
    let mut info: alloc::boxed::Box<TaskInfo> = alloc::boxed::Box::new(TaskInfo{
        status: TaskStatus::UnInit,
        syscall_times: [0; MAX_SYSCALL_NUM],
        time: 0
    });
    let task = current_task().unwrap();
    info.time = crate::timer::get_time_ms() - task.get_dispatched_time();
    info.status = task.get_task_status();
    task.get_syscall_times(&mut info.syscall_times);
    UserBuffer::from_mut_ptr(current_user_token(), ti).copy_from(info.as_ref());
    0
}

/// YOUR JOB: Implement mmap.
pub fn sys_mmap(start: usize, len: usize, prot: usize, fd: usize, offset: usize, shared: bool) -> isize {
    trace!(
        "kernel:pid[{}] sys_mmap",
        current_task().unwrap().pid.0
    );
    if offset % PAGE_SIZE != 0 {
        return -1;
    }
    if start % crate::config::PAGE_SIZE != 0 {
        return -1;
    }
    if prot & (!0x7) != 0 || prot & 0x7 == 0 {
        return -1;
    }
    let start_va: VirtAddr = start.into();
    let end_va: VirtAddr = (start + len).into();
    let flags = (prot as u8) << 1;
    if fd == 0 {
        current_task().unwrap().mmap(start_va, end_va, MapPermission::from_bits(flags).unwrap() | MapPermission::U, None, 0, false)
    } else {
        let fd = {
            let task = current_task().unwrap();
            let inner = task.inner_exclusive_access();
            let fd = inner.fd_table[fd].clone();
            drop(inner);
            // note: inner must be dropped explicitly here
            // for unknown reason, when in fluent syntax, inner is not dropped,
            // so the following mmap call would cause deadlock
            fd
        };
        if let Some(fd) = fd {
            let inode = fd.inode();
            if inode.is_none() {
                return -1;
            }
            let inode = inode.unwrap();
            current_task().unwrap().mmap(start_va, end_va, MapPermission::from_bits(flags).unwrap() | MapPermission::U, Some(inode), offset, shared)
        } else {
            -1
        }
    }
}

/// YOUR JOB: Implement munmap.
pub fn sys_munmap(start: usize, len: usize) -> isize {
    trace!(
        "kernel:pid[{}] sys_munmap",
        current_task().unwrap().pid.0
    );
    if start % crate::config::PAGE_SIZE != 0 {
        return -1;
    }
    let start_va: VirtAddr = start.into();
    let end_va: VirtAddr = (start + len).into();
    current_task().unwrap().munmap(start_va, end_va)
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

/// YOUR JOB: Implement spawn.
/// HINT: fork + exec =/= spawn
pub fn sys_spawn(path: *const u8) -> isize {
    trace!(
        "kernel:pid[{}] sys_spawn",
        current_task().unwrap().pid.0
    );
    let token = current_user_token();
    let path = translated_str(token, path);
    if let Some(app_inode) = open_file(path.as_str(), OpenFlags::RDONLY) {
        let all_data = app_inode.read_all();
        let task = current_task().unwrap().spawn(&all_data);
        let pid = task.pid.0;
        add_task(task);
        pid as isize
    } else {
        -1
    }
}

// YOUR JOB: Set task priority.
pub fn sys_set_priority(prio: isize) -> isize {
    trace!(
        "kernel:pid[{}] sys_set_priority",
        current_task().unwrap().pid.0
    );
    if prio <= 1 {
        return -1;
    }
    current_task().unwrap().set_priority(prio);
    prio
}
