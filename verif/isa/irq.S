/* Interrupts: timer, software and external sources, priority, vectored
 * mtvec, WFI with interrupts disabled, and interrupts in U-mode. */
#include "strata_test.h"

#define MIP_MSIP (1 << 3)
#define MIP_MTIP (1 << 7)

/* mtimecmp = mtime + delta */
#define ARM_TIMER(delta) \
    li t0, TIMER_BASE; \
    lw t1, TIMER_MTIME_LO(t0); \
    addi t1, t1, delta; \
    li t2, -1; \
    sw t2, TIMER_MTIMECMP_HI(t0); \
    sw t1, TIMER_MTIMECMP_LO(t0); \
    sw zero, TIMER_MTIMECMP_HI(t0)

#define DISARM_TIMER \
    li t0, TIMER_BASE; li t2, -1; \
    sw t2, TIMER_MTIMECMP_HI(t0); sw t2, TIMER_MTIMECMP_LO(t0)

/* Spin until the trap count differs from reg. */
#define WAIT_TRAP(reg) \
1:  lw t3, TRAP_COUNT(tp); beq t3, reg, 1b

TEST_BEGIN
    TEST(1)     /* machine timer interrupt */
    lw      s0, TRAP_COUNT(tp)
    ARM_TIMER(200)
    li      t0, MIE_MTIE
    csrs    mie, t0
    csrsi   mstatus, MSTATUS_MIE
    WAIT_TRAP(s0)
    csrci   mstatus, MSTATUS_MIE
    lw      t4, TRAP_CAUSE(tp)
    li      t5, MCAUSE_INTR | IRQ_M_TIMER
    bne     t4, t5, fail
    DISARM_TIMER

    TEST(2)     /* pending but disabled interrupts are visible in mip only */
    li      t0, TIMER_BASE
    li      t1, 1
    sw      t1, TIMER_MSIP(t0)
    csrr    t2, mip
    andi    t2, t2, MIP_MSIP
    beqz    t2, fail
    lw      s0, TRAP_COUNT(tp)
    li      t0, MIE_MSIE
    csrs    mie, t0         /* enabled in mie, but mstatus.MIE = 0 */
    nop; nop; nop
    lw      t3, TRAP_COUNT(tp)
    bne     t3, s0, fail

    TEST(3)     /* priority: with both pending, software is taken before timer */
    ARM_TIMER(0)
    li      t0, MIE_MSIE | MIE_MTIE
    csrs    mie, t0
    csrsi   mstatus, MSTATUS_MIE
    addi    s1, s0, 1
    WAIT_TRAP(s0)
    WAIT_TRAP(s1)           /* the second one follows at once */
    csrci   mstatus, MSTATUS_MIE
    TRAP_CAUSE_N(t4, s0)
    li      t5, MCAUSE_INTR | IRQ_M_SOFT
    bne     t4, t5, fail
    TRAP_CAUSE_N(t4, s1)
    li      t5, MCAUSE_INTR | IRQ_M_TIMER
    bne     t4, t5, fail
    li      t0, TIMER_BASE
    sw      zero, TIMER_MSIP(t0)
    DISARM_TIMER
    csrw    mie, zero

    TEST(4)     /* WFI with mstatus.MIE = 0 resumes on an enabled pending interrupt */
    lw      s0, TRAP_COUNT(tp)
    ARM_TIMER(500)
    li      t0, MIE_MTIE
    csrs    mie, t0
    wfi
    csrr    t1, mip
    andi    t1, t1, MIP_MTIP
    beqz    t1, fail
    lw      t3, TRAP_COUNT(tp)
    bne     t3, s0, fail
    DISARM_TIMER
    csrw    mie, zero

    TEST(5)     /* vectored mtvec: interrupt 3 enters at base + 12 */
    la      t0, vectors
    ori     t0, t0, 1
    csrw    mtvec, t0
    li      t0, TIMER_BASE
    li      t1, 1
    sw      t1, TIMER_MSIP(t0)
    li      t0, MIE_MSIE
    csrs    mie, t0
    li      s1, 0
    csrsi   mstatus, MSTATUS_MIE
1:  beqz    s1, 1b
    csrci   mstatus, MSTATUS_MIE
    CHECK_EQ(s1, 3)
    la      t0, trap_handler
    csrw    mtvec, t0

    TEST(6)     /* external interrupt: VIDEO vertical blank through the INTC */
    lw      s0, TRAP_COUNT(tp)
    li      t0, VIDEO_BASE
    li      t1, VIDEO_VBLANK
    sw      t1, VIDEO_STATUS(t0)
    li      t1, VIDEO_VBLANK_IE
    sw      t1, VIDEO_CTRL(t0)
    li      t0, INTC_BASE
    li      t1, IRQ_VIDEO
    sw      t1, INTC_ENABLE(t0)
    li      t0, MIE_MEIE
    csrs    mie, t0
    csrsi   mstatus, MSTATUS_MIE
    WAIT_TRAP(s0)
    csrci   mstatus, MSTATUS_MIE
    lw      t4, TRAP_CAUSE(tp)
    li      t5, MCAUSE_INTR | IRQ_M_EXT
    bne     t4, t5, fail
    li      t0, INTC_BASE
    lw      t1, INTC_PENDING(t0)
    andi    t1, t1, IRQ_VIDEO
    beqz    t1, fail
    li      t0, VIDEO_BASE
    sw      zero, VIDEO_CTRL(t0)
    li      t1, VIDEO_VBLANK
    sw      t1, VIDEO_STATUS(t0)

    TEST(7)     /* U-mode takes interrupts even with mstatus.MIE = 0 */
    li      t0, -1
    csrw    pmpaddr0, t0
    li      t0, PMP_NAPOT | PMP_R | PMP_W | PMP_X
    csrw    pmpcfg0, t0
    lw      s0, TRAP_COUNT(tp)
    ARM_TIMER(100)
    li      t0, MIE_MTIE
    csrs    mie, t0
    csrci   mstatus, MSTATUS_MIE
    ENTER_U(2f)
2:  WAIT_TRAP(s0)
    EXPECT_TRAP_TO_M(EXC_ECALL_U, ecall)
    li      t0, TIMER_BASE
    li      t2, -1
    sw      t2, TIMER_MTIMECMP_HI(t0)
    sw      t2, TIMER_MTIMECMP_LO(t0)

    TEST(8)     /* in U-mode, WFI with TW=1 is illegal */
    li      t0, MSTATUS_TW
    csrs    mstatus, t0
    ENTER_U(3f)
3:  EXPECT_TRAP_TO_M(EXC_ILLEGAL, wfi)

    PASS
TEST_END

    .p2align 6
vectors:
    j       fail            /* 0: exceptions */
    j       fail
    j       fail
    j       vec_msi         /* 3: machine software */
vec_msi:
    li      s1, 3
    li      s11, TIMER_BASE
    sw      zero, TIMER_MSIP(s11)
    mret
