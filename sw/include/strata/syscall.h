/*
 * System call interface of the Strata kernel (docs/software.md).
 * ecall with the SC_* number in a7 and arguments in a0-a5; the result is in a0.
 * Negative results are errors.
 */
#ifndef STRATA_SYSCALL_H
#define STRATA_SYSCALL_H

#define SC_EXIT      0   /* (code) */
#define SC_WRITE     1   /* (buf, len) -> len; writes to the console */
#define SC_READ      2   /* (buf, len, block) -> n; reads console input bytes */
#define SC_GETKEY    3   /* (block) -> key code, or -1 */
#define SC_SLEEP     4   /* (ms) */
#define SC_TIME      5   /* () -> milliseconds since boot */
#define SC_YIELD     6
#define SC_SPAWN     7   /* (name, arg) -> pid; arg is passed to main() */
#define SC_WAIT      8   /* (pid) -> exit code */
#define SC_KILL      9   /* (pid) */
#define SC_PS        10  /* (buf, max) -> count; fills struct ps_entry */
#define SC_SCREEN    11  /* (on) -> VRAM address when on; grants the text screen */
#define SC_VSYNC     12  /* () -> frame number; waits for the next vertical blank */
#define SC_PALETTE   13  /* (index, rgb) */
#define SC_LEDS      14  /* (value) */
#define SC_BUTTONS   15  /* () -> button levels */
#define SC_INFO      16  /* (buf) fills struct sys_info */
#define SC_PROGRAMS  17  /* (index, name_buf) -> 0, or -1 past the end */
#define SC_HALT      18  /* (code) stops the machine (for tests) */
#define SC_RESET     19  /* () resets the machine */
#define SC_GETPID    20

/* Exit code of a program stopped with Ctrl-C (as in Unix shells: 128 + 2). */
#define EXIT_INTERRUPTED 130

#define E_INVAL  (-1)
#define E_NOENT  (-2)
#define E_BUSY   (-3)
#define E_FAULT  (-4)
#define E_PERM   (-5)

/* Key codes from SC_GETKEY: ASCII, or one of these. */
#define KEY_UP     0x100
#define KEY_DOWN   0x101
#define KEY_LEFT   0x102
#define KEY_RIGHT  0x103
#define KEY_A      0x104
#define KEY_B      0x105
#define KEY_START  0x106

#ifndef __ASSEMBLER__
#include <stdint.h>
struct ps_entry {
    int      pid;
    int      state;
    int      slot;
    uint32_t cycles_lo, cycles_hi;
    char     name[16];
};

struct sys_info {
    uint32_t cycle_lo, cycle_hi;
    uint32_t instret_lo, instret_hi;
    uint32_t time_ms;
    uint32_t clk_hz;
    uint32_t ram_bytes;
    uint32_t flash_id;
    uint32_t frames;
};
#endif

#endif
