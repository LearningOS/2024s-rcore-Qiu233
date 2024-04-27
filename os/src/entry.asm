    .section .text.entry
    .globl _start
_start:
    la sp, boot_stack_top
    la t0, BOOT_HART
    sd a0, 0(t0)
    la t0, DTB_POINTER
    sd a1, 0(t0)
    call rust_main

    .section .bss.stack
    .globl boot_stack_lower_bound
boot_stack_lower_bound:
    .space 4096 * 16
    .globl boot_stack_top
boot_stack_top:

    .section .data # in data, so it's not cleared by `clear_bss`
    .globl BOOT_HART
    .globl DTB_POINTER
BOOT_HART:
    .quad 0
DTB_POINTER:
    .quad 0