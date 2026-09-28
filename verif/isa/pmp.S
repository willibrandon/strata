/* Physical memory protection: TOR, NA4 and NAPOT matching, priority,
 * U-mode checks for fetch/load/store, the default for unmatched U-mode
 * accesses, MPRV, WARL behavior, and locking. */
#define TRAP_RECORD_EXTERNAL
#include "strata_test.h"

TEST_BEGIN
    TEST(1)     /* reset state: all entries off */
    csrr    t1, pmpcfg0
    CHECK_EQ(t1, 0)

    TEST(2)     /* WARL: R=0,W=1 is stored as W=0; bits 6:5 read 0 */
    li      t1, PMP_W | PMP_NA4 | 0x60
    csrw    pmpcfg0, t1
    csrr    t2, pmpcfg0
    CHECK_EQ(t2, PMP_NA4)
    csrw    pmpcfg0, zero

    TEST(3)     /* entries 4-15 and pmpcfg1-3 read zero */
    li      t1, -1
    csrw    pmpaddr7, t1
    csrr    t2, pmpaddr7
    CHECK_EQ(t2, 0)
    csrw    pmpcfg1, t1
    csrr    t2, pmpcfg1
    CHECK_EQ(t2, 0)

    /*
     * Configuration for U-mode:
     *   entry 0 TOR  [0, user_code_end)       R X   (all code, incl. this test)
     *   entry 1 NA4  ro_word                   R
     *   entry 2 NAPOT user_data (128 bytes)    R W   (holds the trap record)
     *   entry 3 off
     */
    la      t0, user_code_end
    srli    t0, t0, 2
    csrw    pmpaddr0, t0
    la      t0, ro_word
    srli    t0, t0, 2
    csrw    pmpaddr1, t0
    la      t0, user_data
    srli    t0, t0, 2
    ori     t0, t0, (128 / 8) - 1
    csrw    pmpaddr2, t0
    li      t0, ((PMP_TOR | PMP_R | PMP_X) << 0) | ((PMP_NA4 | PMP_R) << 8) | ((PMP_NAPOT | PMP_R | PMP_W) << 16)
    csrw    pmpcfg0, t0

    TEST(4)     /* U-mode: permitted accesses work */
    ENTER_U(1f)
1:  la      t1, user_area
    li      t2, 0x77
    sw      t2, 60(t1)
    lw      t3, 60(t1)
    bne     t2, t3, fail
    la      t1, ro_word
    lw      t3, 0(t1)
    CHECK_EQ(t3, 0x5AFE)

    TEST(5)     /* store to a read-only entry */
    la      t1, ro_word
    EXPECT_TRAP(EXC_STORE_ACCESS, sw zero, 0(t1))
    CHECK_TVAL(t1)

    TEST(6)     /* load just past the NAPOT region matches nothing: denied in U-mode */
    la      t1, after_region
    EXPECT_TRAP(EXC_LOAD_ACCESS, lw t2, 0(t1))
    CHECK_TVAL(t1)

    TEST(7)     /* devices are not accessible */
    li      t1, SYS_BASE
    EXPECT_TRAP(EXC_LOAD_ACCESS, lw t2, SYS_ID(t1))

    TEST(8)     /* data is not executable */
    la      t1, user_area
    EXPECT_TRAP(EXC_INSN_ACCESS, jr t1)
    CHECK_TVAL(t1)

    TEST(9)     /* M-mode CSRs are illegal in U-mode, and so is MRET */
    EXPECT_TRAP(EXC_ILLEGAL, csrr t1, mstatus)
    EXPECT_TRAP(EXC_ILLEGAL, mret)

    TEST(10)    /* back to M-mode with ECALL (cause 8, previous mode U) */
    EXPECT_TRAP_TO_M(EXC_ECALL_U, ecall)
    lw      t1, TRAP_MODE(tp)
    CHECK_EQ(t1, 0)

    TEST(11)    /* M-mode ignores unlocked entries */
    la      t1, ro_word
    li      t2, 0x5AFE
    sw      t2, 0(t1)
    la      t1, after_region
    lw      t2, 0(t1)

    TEST(12)    /* MPRV with MPP=U: M-mode loads use U-mode permissions */
    li      t0, MSTATUS_MPP
    csrc    mstatus, t0
    li      t0, MSTATUS_MPRV
    csrs    mstatus, t0
    la      t1, after_region
    EXPECT_TRAP(EXC_LOAD_ACCESS, lw t2, 0(t1))
    li      t0, MSTATUS_MPRV
    csrc    mstatus, t0
    lw      t2, 0(t1)       /* allowed again */

    TEST(13)    /* the lowest-numbered matching entry wins: entry 3 (no permissions)
                   over a word inside entry 2's region does not apply */
    la      t0, user_area
    srli    t0, t0, 2
    csrw    pmpaddr3, t0
    li      t0, (PMP_NA4) << 24
    csrs    pmpcfg0, t0
    ENTER_U(2f)
2:  la      t1, user_area
    lw      t2, 0(t1)
    EXPECT_TRAP_TO_M(EXC_ECALL_U, ecall)

    TEST(14)    /* a locked entry applies to M-mode and cannot be changed */
    la      t0, locked_word
    srli    t0, t0, 2
    csrw    pmpaddr3, t0
    li      t0, 0xFF << 24
    csrc    pmpcfg0, t0
    li      t0, (PMP_L | PMP_NA4 | PMP_R) << 24
    csrs    pmpcfg0, t0
    la      t1, locked_word
    lw      t2, 0(t1)
    EXPECT_TRAP(EXC_STORE_ACCESS, sw zero, 0(t1))
    csrw    pmpaddr3, zero
    csrr    t2, pmpaddr3
    srli    t3, t1, 2
    bne     t2, t3, fail
    li      t0, 0xFF << 24
    csrc    pmpcfg0, t0
    csrr    t2, pmpcfg0
    srli    t2, t2, 24
    CHECK_EQ(t2, PMP_L | PMP_NA4 | PMP_R)

    PASS
TEST_END
user_code_end:

    .data
    .p2align 7
user_data:
trap_record:  .space 64
user_area:    .space 64
after_region: .word 0
ro_word:      .word 0x5AFE
locked_word:  .word 0x1111
