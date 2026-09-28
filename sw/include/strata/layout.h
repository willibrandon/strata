/*
 * RAM layout shared by the kernel and the build (sw/Makefile reads the
 * *_BASE and *_SIZE values from this file to generate the linker layouts).
 */
#ifndef STRATA_LAYOUT_H
#define STRATA_LAYOUT_H

#define KERNEL_BASE   0x10000000
#define KERNEL_SIZE   0x8000

/* User program slots. Each program is linked at its slot's base; a slot runs
 * one program at a time. */
#define SLOT0_BASE    0x10008000   /* shell */
#define SLOT0_SIZE    0x3000
#define SLOT1_BASE    0x1000B000   /* game */
#define SLOT1_SIZE    0x4000
#define SLOT2_BASE    0x1000F000   /* utilities */
#define SLOT2_SIZE    0x1000
#define NUM_SLOTS     3

/* The boot ROM uses the top 512 bytes of RAM while loading. */
#define BOOT_SCRATCH  0x1000FE00

/* Header at the base of every user program (sw/user/crt0.S). */
#define PROG_MAGIC    0x474F5250   /* "PROG" */

/* Program store in the boot flash (built by scripts/mkflash.py):
 *   0x00 magic STORE_MAGIC, 0x04 count, then count entries of
 *   { char name[16]; uint32_t offset, load_addr, size, crc32; }
 * with offset relative to the store base. */
#define STORE_OFFSET  0x00280000
#define STORE_MAGIC   0x52545350   /* "PSTR" */
#define STORE_MAX     16

#ifndef __ASSEMBLER__
#include <stdint.h>
struct prog_header {
    uint32_t magic;
    uint32_t entry;
    uint32_t rx_end;      /* end of code and read-only data */
    uint32_t data_end;    /* end of initialized data */
    uint32_t bss_end;     /* end of zero-initialized data */
    uint32_t stack_top;   /* top of the slot */
    char     name[16];
};

struct store_entry {
    char     name[16];
    uint32_t offset;
    uint32_t load_addr;
    uint32_t size;
    uint32_t crc32;
};
#endif

#endif
