.option arch, +a # enable atomic instructions
.altmacro
.macro SAVE_SN n
    sd s\n, (\n+2)*8(a0)
.endm
.macro LOAD_SN n
    ld s\n, (\n+2)*8(a1)
.endm
    .section .text
    .globl __switch
__switch:
    # __switch(
    #     current_task_cx_ptr: *mut TaskContext,
    #     next_task_cx_ptr: *const TaskContext
    # )
    # save kernel stack of current task
    sd sp, 8(a0)
    # save ra & s0~s11 of current execution
    sd ra, 0(a0)
    .set n, 0
    .rept 12
        SAVE_SN %n
        .set n, n + 1
    .endr

    # CRITICAL: the following snippet is to
    # * release the lock on previous `TaskContext`
    # * acquire the lock on next `TaskContext`
    # This is critical because `Arc<TaskControlBlock>` is returned to task queue before `__switch` is called,
    # so it's **VERY** likely that other hart fetches the tcb from queue and `__switch` to it before it's filled with meaningful data.

    # Design note:
    # We can also have a lock in TCB rather than `TaskContext`, and by the former solution it's nice to have no assembly code written by hand.
    # But the latter solution requires only modification of `TaskContext` and `__switch`, thus is transparent to the task scheduler.
    # I think this issue is more related to context switch synchronization rather than task scheduling.

    # t* are saved by caller, thus can be used freely
    addi t3, a0, 14*8
rel_prev:
    lr.d.aq t0, (t3) # acquired though not compared to 1
                     # because both intiial idle task context and the *dummy* context passed on exit will have 0
    sc.d.rl t2, x0, (t3) # write 0
    bnez t2, rel_prev

    li t1, 1
    addi t3, a1, 14*8
acq_next:
    lr.d.aq t0, (t3)
    bnez t0, acq_next # must compare, `next_task_cx_ptr`'s content is only meaningful after other hart has released it
    sc.d.rl t2, t1, (t3) # write 1
    bnez t2, acq_next


    # restore ra & s0~s11 of next execution
    ld ra, 0(a1)
    .set n, 0
    .rept 12
        LOAD_SN %n
        .set n, n + 1
    .endr
    # restore kernel stack of next task
    ld sp, 8(a1)
    ret

