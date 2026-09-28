/* CSR fields: WARL behavior, read-only values and reset state. */
#include "strata_test.h"

TEST_BEGIN
    TEST(1)     /* identification */
    csrr    t1, misa
    CHECK_EQ(t1, 0x40100100)
    csrr    t1, mimpid
    CHECK_EQ(t1, 0x00010000)
    csrr    t1, mhartid
    CHECK_EQ(t1, 0)
    csrr    t1, marchid
    CHECK_EQ(t1, 0)

    TEST(2)     /* misa ignores writes */
    csrw    misa, zero
    csrr    t1, misa
    CHECK_EQ(t1, 0x40100100)

    TEST(3)     /* mstatus: only MIE, MPIE, MPP, MPRV, TW are writable */
    li      t1, -1
    csrw    mstatus, t1
    csrr    t2, mstatus
    CHECK_EQ(t2, MSTATUS_MIE | MSTATUS_MPIE | MSTATUS_MPP | MSTATUS_MPRV | MSTATUS_TW)
    csrw    mstatus, zero

    TEST(4)     /* MPP is WARL: 01 and 10 are stored as U (00) */
    li      t1, 1 << 11
    csrw    mstatus, t1
    csrr    t2, mstatus
    CHECK_EQ(t2, 0)
    li      t1, 2 << 11
    csrw    mstatus, t1
    csrr    t2, mstatus
    CHECK_EQ(t2, 0)

    TEST(5)     /* mie keeps MSIE, MTIE, MEIE only */
    li      t1, -1
    csrw    mie, t1
    csrr    t2, mie
    CHECK_EQ(t2, MIE_MSIE | MIE_MTIE | MIE_MEIE)
    csrw    mie, zero

    TEST(6)     /* mip ignores writes */
    li      t1, -1
    csrw    mip, t1
    csrr    t2, mip
    li      t3, 0x888 & ~0x80       /* MTIP may legitimately be set */
    and     t2, t2, t3
    CHECK_EQ(t2, 0)

    TEST(7)     /* mtvec bit 1 reads 0; mode bit 0 is kept */
    la      t3, trap_handler
    li      t1, 0x10000002
    csrw    mtvec, t1
    csrr    t2, mtvec
    CHECK_EQ(t2, 0x10000000)
    li      t1, 0x10000001
    csrw    mtvec, t1
    csrr    t2, mtvec
    CHECK_EQ(t2, 0x10000001)
    csrw    mtvec, t3

    TEST(8)     /* mepc bits 1:0 read 0 */
    li      t1, 0x12345677
    csrw    mepc, t1
    csrr    t2, mepc
    CHECK_EQ(t2, 0x12345674)

    TEST(9)     /* mcause keeps bit 31 and bits 4:0 */
    li      t1, -1
    csrw    mcause, t1
    csrr    t2, mcause
    CHECK_EQ(t2, 0x8000001F)

    TEST(10)    /* mscratch and mtval are full 32-bit registers */
    li      t1, 0xA5A5F00F
    csrw    mscratch, t1
    csrr    t2, mscratch
    bne     t1, t2, fail
    csrw    mtval, t1
    csrr    t2, mtval
    bne     t1, t2, fail

    TEST(11)    /* mcounteren keeps CY, TM, IR */
    li      t1, -1
    csrw    mcounteren, t1
    csrr    t2, mcounteren
    CHECK_EQ(t2, 7)
    csrw    mcounteren, zero

    TEST(12)    /* read-only-zero machine registers */
    li      t1, -1
    csrw    menvcfg, t1
    csrr    t2, menvcfg
    CHECK_EQ(t2, 0)
    csrw    mstatush, t1
    csrr    t2, mstatush
    CHECK_EQ(t2, 0)
    csrr    t2, mconfigptr
    CHECK_EQ(t2, 0)

    TEST(13)    /* CSRRSI/CSRRCI with uimm = 0 do not write (legal on read-only) */
    csrrsi  t1, mvendorid, 0
    csrrci  t1, mvendorid, 0

    PASS
TEST_END
