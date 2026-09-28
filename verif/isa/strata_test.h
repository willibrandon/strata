/*
 * Directed-test framework for S1's privileged architecture (docs/isa.md).
 *
 * Register conventions inside tests:
 *   gp   current test number (reported as the halt code on failure)
 *   tp   base of the trap record (TRAP_* offsets below); never modify
 *   s11  reserved for the trap handler
 *
 * The trap handler records mcause, mepc, mtval and the previous privilege,
 * counts traps, and resumes at the address set with EXPECT_TRAP (or, if
 * none, after the trapping instruction for exceptions and at mepc for
 * interrupts). A resume address with bit 0 set resumes in M-mode. For an
 * interrupt, the handler also clears its enable bit in mie, so the
 * interrupt is taken once until the test re-enables it.
 * PASS halts with code 0; FAIL halts with the test number in gp.
 */
#ifndef STRATA_TEST_H
#define STRATA_TEST_H

#include <strata/machine.h>
#include <strata/csr.h>

#define TRAP_CAUSE   0
#define TRAP_EPC     4
#define TRAP_TVAL    8
#define TRAP_RESUME  12
#define TRAP_COUNT   16
#define TRAP_MODE    20   /* MPP after the trap: previous privilege */
#define TRAP_HIST    40   /* causes of the last four traps: entry (count & 3) */

#define TEST(n) li gp, n

#define TEST_BEGIN \
    .section .text.init, "ax"; \
    .globl _start; \
_start: \
    la tp, trap_record; \
    la t0, trap_handler; \
    csrw mtvec, t0; \
    li gp, 0; \
    j test_body; \
    .p2align 2; \
trap_handler: \
    csrr s11, mcause; sw s11, TRAP_CAUSE(tp); \
    csrr s11, mepc;   sw s11, TRAP_EPC(tp); \
    csrr s11, mtval;  sw s11, TRAP_TVAL(tp); \
    csrr s11, mstatus; srli s11, s11, 11; andi s11, s11, 3; sw s11, TRAP_MODE(tp); \
    sw t6, TRAP_COUNT+8(tp); \
    lw s11, TRAP_COUNT(tp); andi t6, s11, 3; slli t6, t6, 2; add t6, t6, tp; \
    addi s11, s11, 1; sw s11, TRAP_COUNT(tp); \
    csrr s11, mcause; sw s11, TRAP_HIST(t6); \
    lw t6, TRAP_COUNT+8(tp); \
    csrr s11, mcause; \
    bgez s11, 3f; \
    andi s11, s11, 31; sw t6, TRAP_COUNT+8(tp); li t6, 1; sll s11, t6, s11; csrc mie, s11; \
    lw t6, TRAP_COUNT+8(tp); \
3:  lw s11, TRAP_RESUME(tp); \
    beqz s11, 1f; \
    sw zero, TRAP_RESUME(tp); \
    andi s11, s11, 1; \
    beqz s11, 4f; \
    li s11, MSTATUS_MPP; csrs mstatus, s11; \
4:  lw s11, TRAP_RESUME+20(tp); \
    csrw mepc, s11; \
    mret; \
1:  csrr s11, mcause; \
    bltz s11, 2f; \
    csrr s11, mepc; addi s11, s11, 4; csrw mepc, s11; \
2:  mret; \
test_body:

/* Execute `insn...`; it must trap with `cause` (then continue after it). */
#define EXPECT_TRAP(cause, insn...) \
    la s11, 91f; sw s11, TRAP_RESUME(tp); sw s11, TRAP_RESUME+20(tp); \
    sw zero, TRAP_CAUSE(tp); \
    insn; \
    j fail; \
91: lw s11, TRAP_CAUSE(tp); li t6, cause; bne s11, t6, fail

/* Like EXPECT_TRAP, but resume in M-mode (for traps taken in U-mode). */
#define EXPECT_TRAP_TO_M(cause, insn...) \
    la s11, 92f; sw s11, TRAP_RESUME+20(tp); ori s11, s11, 1; sw s11, TRAP_RESUME(tp); \
    sw zero, TRAP_CAUSE(tp); \
    insn; \
    j fail; \
92: lw s11, TRAP_CAUSE(tp); li t6, cause; bne s11, t6, fail

/* The last trap's mtval must equal reg. */
#define CHECK_TVAL(reg) lw t6, TRAP_TVAL(tp); bne t6, reg, fail
#define CHECK_EPC_LABEL(label) lw t6, TRAP_EPC(tp); la t5, label; bne t6, t5, fail
#define CHECK_EQ(reg, value) li t6, value; bne reg, t6, fail

/* Enter U-mode at `label` (with the current PMP configuration). */
#define ENTER_U(label) \
    li t0, MSTATUS_MPP; csrc mstatus, t0; \
    la t0, label; csrw mepc, t0; \
    mret

#define PASS \
    li t0, SYS_BASE; sw zero, SYS_HALT(t0); \
99: j 99b

/* Cause of trap number `nreg` (a register; traps count from 0) into reg,
 * valid for the last four traps. Clobbers t6. */
#define TRAP_CAUSE_N(reg, nreg) \
    andi t6, nreg, 3; slli t6, t6, 2; add t6, t6, tp; lw reg, TRAP_HIST(t6)

/* A test whose U-mode code needs the trap record inside its own read/write
 * region defines TRAP_RECORD_EXTERNAL and places `trap_record` (64 bytes). */
#ifdef TRAP_RECORD_EXTERNAL
#define TRAP_RECORD
#else
#define TRAP_RECORD .section .data; .p2align 2; trap_record: .space 64
#endif

#define TEST_END \
fail: \
    andi gp, gp, 0xFF; \
    bnez gp, 1f; li gp, 0xFE; \
1:  li t0, SYS_BASE; sw gp, SYS_HALT(t0); \
98: j 98b; \
    TRAP_RECORD

#endif
