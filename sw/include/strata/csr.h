/* CSR numbers and access helpers for the S1 core (docs/isa.md). */
#ifndef STRATA_CSR_H
#define STRATA_CSR_H

#define MSTATUS_MIE   (1 << 3)
#define MSTATUS_MPIE  (1 << 7)
#define MSTATUS_MPP   (3 << 11)
#define MSTATUS_MPP_M (3 << 11)
#define MSTATUS_MPP_U (0 << 11)
#define MSTATUS_MPRV  (1 << 17)
#define MSTATUS_TW    (1 << 21)

#define MIE_MSIE      (1 << 3)
#define MIE_MTIE      (1 << 7)
#define MIE_MEIE      (1 << 11)

#define MCAUSE_INTR   0x80000000
#define IRQ_M_SOFT    3
#define IRQ_M_TIMER   7
#define IRQ_M_EXT     11

#define EXC_INSN_MISALIGNED  0
#define EXC_INSN_ACCESS      1
#define EXC_ILLEGAL          2
#define EXC_BREAKPOINT       3
#define EXC_LOAD_MISALIGNED  4
#define EXC_LOAD_ACCESS      5
#define EXC_STORE_MISALIGNED 6
#define EXC_STORE_ACCESS     7
#define EXC_ECALL_U          8
#define EXC_ECALL_M          11

#define PMP_R     0x01
#define PMP_W     0x02
#define PMP_X     0x04
#define PMP_OFF   0x00
#define PMP_TOR   0x08
#define PMP_NA4   0x10
#define PMP_NAPOT 0x18
#define PMP_L     0x80

#ifndef __ASSEMBLER__
#include <stdint.h>

#define csr_read(csr) ({ uint32_t v_; __asm__ volatile("csrr %0, " #csr : "=r"(v_)); v_; })
#define csr_write(csr, v) __asm__ volatile("csrw " #csr ", %0" :: "r"((uint32_t)(v)))
#define csr_set(csr, v) __asm__ volatile("csrs " #csr ", %0" :: "r"((uint32_t)(v)))
#define csr_clear(csr, v) __asm__ volatile("csrc " #csr ", %0" :: "r"((uint32_t)(v)))

static inline uint64_t read_cycle64(void)
{
    uint32_t hi, lo, hi2;
    do {
        hi = csr_read(mcycleh);
        lo = csr_read(mcycle);
        hi2 = csr_read(mcycleh);
    } while (hi != hi2);
    return ((uint64_t)hi << 32) | lo;
}
#endif

#endif
