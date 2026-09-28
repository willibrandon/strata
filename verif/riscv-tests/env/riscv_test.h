/*
 * riscv-tests environment for the Strata machine, replacing the upstream
 * "p" environment. Differences from upstream:
 *   - results are reported by writing SYS.HALT: code 0 for pass, otherwise
 *     the failing test number (or 0xFF for an unexpected trap);
 *   - no tohost/fromhost, satp, medeleg or stvec (not implemented);
 *   - PMP entry 0 grants all of memory, as in upstream, so the U-mode tests
 *     can run.
 * The test is linked at RAM base; a two-instruction ROM stub jumps to it.
 */
#ifndef STRATA_RISCV_TEST_H
#define STRATA_RISCV_TEST_H

#include "encoding.h"

#define SYS_HALT_ADDR 0x40000020

#define RVTEST_RV32U .macro init; .endm
#define RVTEST_RV64U RVTEST_RV32U
#define RVTEST_RV32M .macro init; RVTEST_ENABLE_MACHINE; .endm
#define RVTEST_RV64M RVTEST_RV32M

#define RVTEST_ENABLE_MACHINE \
        li a0, MSTATUS_MPP; \
        csrs mstatus, a0;

#define TESTNUM gp

#define INIT_XREG \
        li x1, 0; li x2, 0; li x3, 0; li x4, 0; li x5, 0; li x6, 0; li x7, 0; \
        li x8, 0; li x9, 0; li x10, 0; li x11, 0; li x12, 0; li x13, 0; li x14, 0; \
        li x15, 0; li x16, 0; li x17, 0; li x18, 0; li x19, 0; li x20, 0; li x21, 0; \
        li x22, 0; li x23, 0; li x24, 0; li x25, 0; li x26, 0; li x27, 0; li x28, 0; \
        li x29, 0; li x30, 0; li x31, 0;

#define INIT_PMP \
        li t0, -1; \
        csrw pmpaddr0, t0; \
        li t0, PMP_NAPOT | PMP_R | PMP_W | PMP_X; \
        csrw pmpcfg0, t0;

#define RVTEST_CODE_BEGIN \
        .section .text.init; \
        .align 6; \
        .weak mtvec_handler; \
        .globl _start; \
_start: \
        j reset_vector; \
        .align 2; \
trap_vector: \
        csrr t5, mcause; \
        li t6, CAUSE_USER_ECALL; \
        beq t5, t6, report; \
        li t6, CAUSE_MACHINE_ECALL; \
        beq t5, t6, report; \
        la t5, mtvec_handler; \
        beqz t5, 1f; \
        jr t5; \
1:      li TESTNUM, (0xFF << 1) | 1; \
report: \
        /* TESTNUM = 1 for pass, (n << 1) | 1 for failure of test n */ \
        srli t5, TESTNUM, 1; \
        andi t5, t5, 0xFF; \
        li t6, SYS_HALT_ADDR; \
        sw t5, 0(t6); \
2:      j 2b; \
reset_vector: \
        INIT_XREG; \
        INIT_PMP; \
        li TESTNUM, 0; \
        la t0, trap_vector; \
        csrw mtvec, t0; \
        csrwi mstatus, 0; \
        init; \
        la t0, 1f; \
        csrw mepc, t0; \
        csrr a0, mhartid; \
        mret; \
1:

#define RVTEST_CODE_END unimp

#define RVTEST_PASS \
        fence; \
        li TESTNUM, 1; \
        li a7, 93; \
        li a0, 0; \
        ecall

#define RVTEST_FAIL \
        fence; \
1:      beqz TESTNUM, 1b; \
        sll TESTNUM, TESTNUM, 1; \
        or TESTNUM, TESTNUM, 1; \
        li a7, 93; \
        addi a0, TESTNUM, 0; \
        ecall

#define EXTRA_DATA

#define RVTEST_DATA_BEGIN \
        EXTRA_DATA \
        .align 4; .global begin_signature; begin_signature:

#define RVTEST_DATA_END .align 4; .global end_signature; end_signature:

#endif
