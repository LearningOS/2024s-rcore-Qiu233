    .section .text.hart
    .globl _hart_start
_hart_start:
    ld sp, 0(a1) # sp
    ld t0, 8(a1) # satp
    csrw satp, t0
    addi a1, a1, 16 # &AtomicUsize
    call hart_main
