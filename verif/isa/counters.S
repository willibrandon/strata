/* Counters: minstret counts exactly, writes replace a half and suppress the
 * increment, time matches the TIMER, and mcounteren gates U-mode access. */
#include "strata_test.h"

TEST_BEGIN
    TEST(1)     /* 10 NOPs between two reads: the first read also retires */
    csrr    t1, minstret
    nop; nop; nop; nop; nop; nop; nop; nop; nop; nop
    csrr    t2, minstret
    sub     t2, t2, t1
    CHECK_EQ(t2, 11)

    TEST(2)     /* an instruction that writes minstret is not counted */
    li      t1, 100
    csrw    minstret, t1
    csrr    t2, minstret
    CHECK_EQ(t2, 100)

    TEST(3)     /* writing the high half leaves the low half */
    li      t1, 7
    csrw    minstreth, t1
    csrr    t2, minstreth
    CHECK_EQ(t2, 7)
    csrw    minstreth, zero

    TEST(4)     /* mcycle advances, and a write replaces it */
    csrr    t1, mcycle
    csrr    t2, mcycle
    bgeu    t1, t2, fail
    li      t1, 0x1000
    csrw    mcycle, t1
    csrr    t2, mcycle
    sub     t2, t2, t1
    li      t3, 16
    bgeu    t2, t3, fail

    TEST(5)     /* time follows the TIMER's mtime */
    li      t0, TIMER_BASE
    lw      t1, TIMER_MTIME_LO(t0)
    csrr    t2, time
    sub     t2, t2, t1
    li      t3, 16
    bgeu    t2, t3, fail

    TEST(6)     /* read-only-zero counters and inhibit register */
    csrr    t1, mhpmcounter3
    CHECK_EQ(t1, 0)
    li      t2, -1
    csrw    mhpmcounter3, t2
    csrr    t1, mhpmcounter3
    CHECK_EQ(t1, 0)
    csrw    mcountinhibit, t2
    csrr    t1, mcountinhibit
    CHECK_EQ(t1, 0)

    /* U-mode access needs mcounteren; PMP entry 0 opens all memory. */
    li      t0, -1
    csrw    pmpaddr0, t0
    li      t0, PMP_NAPOT | PMP_R | PMP_W | PMP_X
    csrw    pmpcfg0, t0

    TEST(7)     /* with mcounteren = 0, U-mode reads of cycle are illegal */
    csrw    mcounteren, zero
    ENTER_U(1f)
1:  EXPECT_TRAP_TO_M(EXC_ILLEGAL, rdcycle t1)

    TEST(8)     /* with CY, TM, IR set, U-mode may read all three */
    li      t0, 7
    csrw    mcounteren, t0
    ENTER_U(2f)
2:  rdcycle t1
    rdtime  t2
    rdinstret t3
    EXPECT_TRAP_TO_M(EXC_ECALL_U, ecall)
    beqz    t1, fail
    beqz    t3, fail

    TEST(9)     /* hpmcounters stay illegal from U-mode */
    ENTER_U(3f)
3:  EXPECT_TRAP_TO_M(EXC_ILLEGAL, csrr t1, hpmcounter3)

    PASS
TEST_END
