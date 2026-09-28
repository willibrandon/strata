/*
 * Kernel entry, trap entry and return to user mode.
 *
 * The kernel runs in M-mode with interrupts disabled (mstatus.MIE = 0). User
 * tasks run in U-mode with interrupts enabled. mscratch holds the current
 * task's register save area while a task runs, and 0 while the kernel runs;
 * a trap with mscratch = 0 therefore came from the kernel itself and is fatal.
 */
#include <strata/machine.h>
#include <strata/csr.h>

#define FRAME_PC 128

    .section .text.start, "ax"
    .globl _start
_start:
    la      sp, kernel_stack_top
    csrw    mscratch, zero
    la      t0, trap_entry
    csrw    mtvec, t0
    /* Clear .bss. */
    la      t0, __bss_start
    la      t1, __bss_end
1:  bgeu    t0, t1, 2f
    sw      zero, 0(t0)
    addi    t0, t0, 4
    j       1b
2:  call    kmain
    /* kmain does not return. */
3:  j       3b

    .text
    .p2align 2
    .globl trap_entry
trap_entry:
    csrrw   sp, mscratch, sp
    beqz    sp, kernel_trap
    /* sp = task save area; mscratch = user sp. */
    sw      x1, 4(sp)
    sw      x3, 12(sp)
    sw      x4, 16(sp)
    sw      x5, 20(sp)
    sw      x6, 24(sp)
    sw      x7, 28(sp)
    sw      x8, 32(sp)
    sw      x9, 36(sp)
    sw      x10, 40(sp)
    sw      x11, 44(sp)
    sw      x12, 48(sp)
    sw      x13, 52(sp)
    sw      x14, 56(sp)
    sw      x15, 60(sp)
    sw      x16, 64(sp)
    sw      x17, 68(sp)
    sw      x18, 72(sp)
    sw      x19, 76(sp)
    sw      x20, 80(sp)
    sw      x21, 84(sp)
    sw      x22, 88(sp)
    sw      x23, 92(sp)
    sw      x24, 96(sp)
    sw      x25, 100(sp)
    sw      x26, 104(sp)
    sw      x27, 108(sp)
    sw      x28, 112(sp)
    sw      x29, 116(sp)
    sw      x30, 120(sp)
    sw      x31, 124(sp)
    csrr    t0, mscratch
    sw      t0, 8(sp)
    csrr    t0, mepc
    sw      t0, FRAME_PC(sp)
    csrw    mscratch, zero
    mv      a0, sp
    la      sp, kernel_stack_top
    call    trap_handler
    /* Fall through with a0 = task to run next. */

/* return_to_user(struct task *t): resume t in U-mode. */
    .globl return_to_user
return_to_user:
    lw      t0, FRAME_PC(a0)
    csrw    mepc, t0
    /* mret to U-mode with interrupts enabled. */
    li      t0, MSTATUS_MPP
    csrc    mstatus, t0
    li      t0, MSTATUS_MPIE
    csrs    mstatus, t0
    csrw    mscratch, a0
    lw      x1, 4(a0)
    lw      x2, 8(a0)
    lw      x3, 12(a0)
    lw      x4, 16(a0)
    lw      x5, 20(a0)
    lw      x6, 24(a0)
    lw      x7, 28(a0)
    lw      x8, 32(a0)
    lw      x9, 36(a0)
    lw      x11, 44(a0)
    lw      x12, 48(a0)
    lw      x13, 52(a0)
    lw      x14, 56(a0)
    lw      x15, 60(a0)
    lw      x16, 64(a0)
    lw      x17, 68(a0)
    lw      x18, 72(a0)
    lw      x19, 76(a0)
    lw      x20, 80(a0)
    lw      x21, 84(a0)
    lw      x22, 88(a0)
    lw      x23, 92(a0)
    lw      x24, 96(a0)
    lw      x25, 100(a0)
    lw      x26, 104(a0)
    lw      x27, 108(a0)
    lw      x28, 112(a0)
    lw      x29, 116(a0)
    lw      x30, 120(a0)
    lw      x31, 124(a0)
    lw      x10, 40(a0)
    mret

/* A trap while the kernel was running: report it and stop. */
kernel_trap:
    csrrw   sp, mscratch, sp
    la      sp, kernel_stack_top
    call    kernel_fault
4:  j       4b

    .section .bss
    .p2align 4
kernel_stack:
    .space  2048
kernel_stack_top:
