/* Exceptions: access faults from the memory map, misalignment, illegal
 * instructions and CSR accesses, breakpoints, environment calls, and
 * precision (nothing after a faulting instruction takes effect). */
#include "strata_test.h"

TEST_BEGIN
    TEST(1)     /* load from an unmapped address */
    li      t1, 0x30000000
    EXPECT_TRAP(EXC_LOAD_ACCESS, lw t2, 0(t1))
    CHECK_TVAL(t1)

    TEST(2)     /* store to the boot ROM */
    li      t1, 0x00000100
    EXPECT_TRAP(EXC_STORE_ACCESS, sw t2, 0(t1))
    CHECK_TVAL(t1)

    TEST(3)     /* store to an unmapped MMIO page */
    li      t1, 0x40007000
    EXPECT_TRAP(EXC_STORE_ACCESS, sw zero, 0(t1))

    TEST(4)     /* byte load from MMIO */
    li      t1, SYS_BASE
    EXPECT_TRAP(EXC_LOAD_ACCESS, lbu t2, SYS_ID(t1))

    TEST(5)     /* a halfword store to MMIO faults and writes nothing */
    li      t1, SYS_BASE
    li      t2, 0x1234
    sw      t2, SYS_SCRATCH(t1)
    EXPECT_TRAP(EXC_STORE_ACCESS, sh zero, SYS_SCRATCH(t1))
    lw      t3, SYS_SCRATCH(t1)
    CHECK_EQ(t3, 0x1234)

    TEST(6)     /* instruction fetch from VRAM */
    li      t1, VRAM_BASE
    EXPECT_TRAP(EXC_INSN_ACCESS, jr t1)
    CHECK_TVAL(t1)

    TEST(7)     /* a jump to a misaligned target traps on the jump */
    la      t1, 1f
    addi    t1, t1, 2
    li      t3, 0
    EXPECT_TRAP(EXC_INSN_MISALIGNED, jalr t3, 0(t1))
    CHECK_TVAL(t1)
    CHECK_EQ(t3, 0)     /* the jump did not write rd */
1:  nop

    TEST(8)     /* misaligned word load */
    la      t1, scratch + 2
    EXPECT_TRAP(EXC_LOAD_MISALIGNED, lw t2, 0(t1))
    CHECK_TVAL(t1)

    TEST(9)     /* misaligned halfword store */
    la      t1, scratch + 1
    EXPECT_TRAP(EXC_STORE_MISALIGNED, sh t2, 0(t1))
    CHECK_TVAL(t1)

    TEST(10)    /* precision: the store after a faulting load does not happen */
    la      t3, scratch
    sw      zero, 0(t3)
    li      t4, 0x55
    li      t1, 0x30000000
    EXPECT_TRAP(EXC_LOAD_ACCESS, lw t2, 0(t1); sw t4, 0(t3))
    lw      t5, 0(t3)
    CHECK_EQ(t5, 0)

    TEST(11)    /* illegal instruction: mtval holds the instruction */
    EXPECT_TRAP(EXC_ILLEGAL, .word 0x0000FFFF)
    li      t1, 0x0000FFFF
    CHECK_TVAL(t1)

    TEST(12)    /* the all-zeros word is illegal */
    EXPECT_TRAP(EXC_ILLEGAL, .word 0)

    TEST(13)    /* EBREAK: mtval is its address */
    EXPECT_TRAP(EXC_BREAKPOINT, ebreak)
    lw      t5, TRAP_EPC(tp)
    CHECK_TVAL(t5)

    TEST(14)    /* ECALL from M-mode */
    EXPECT_TRAP(EXC_ECALL_M, ecall)
    lw      t5, TRAP_TVAL(tp)
    CHECK_EQ(t5, 0)

    TEST(15)    /* writing a read-only CSR is illegal; reading it is not */
    EXPECT_TRAP(EXC_ILLEGAL, csrw mvendorid, zero)
    csrr    t1, mvendorid
    CHECK_EQ(t1, 0)

    TEST(16)    /* an unimplemented CSR */
    EXPECT_TRAP(EXC_ILLEGAL, csrr t1, 0x7C0)

    TEST(17)    /* SRET and SFENCE.VMA are illegal */
    EXPECT_TRAP(EXC_ILLEGAL, .word 0x10200073)
    EXPECT_TRAP(EXC_ILLEGAL, .word 0x12000073)

    TEST(18)    /* CSRRS with rs1 = x0 only reads; with any other rs1 it writes */
    csrrs   t1, mhartid, x0
    li      t2, 0
    EXPECT_TRAP(EXC_ILLEGAL, csrrs t1, mhartid, t2)

    TEST(19)    /* SRAI with a bad funct7 */
    EXPECT_TRAP(EXC_ILLEGAL, .word 0x6000d093)

    TEST(20)    /* FENCE and FENCE.TSO with nonzero rd/rs1 fields execute (no trap) */
    lw      t1, TRAP_COUNT(tp)
    .word   0x0FF5858F          /* fence iorw, iorw with rd = rs1 = x11 */
    .word   0x8335858F          /* fence.tso with rd = rs1 = x11 */
    lw      t2, TRAP_COUNT(tp)
    bne     t1, t2, fail

    PASS
TEST_END

    .p2align 2
scratch: .word 0, 0
