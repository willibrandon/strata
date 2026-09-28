/* Differential-test program for --gc-sections: a mix of reachable and
 * unreachable functions and data. gc_main is placed in .text.gc_root, which
 * the test layouts KEEP; nothing else refers to it. */
#include <stdint.h>

#define NOINLINE __attribute__((noinline))

extern void kprintf(const char *fmt, ...);

int used_counter;
int unused_counter = 5;
uint8_t unused_buffer[256];
uint32_t used_buffer[16];
const char used_msg[] = "printed";
const char unused_msg[] = "never printed";
const uint16_t unused_table[8] = {1, 2, 3, 4, 5, 6, 7, 8};

/* An unreachable cycle. */
NOINLINE int cyc_b(int n);
NOINLINE int cyc_a(int n) { return n ? cyc_b(n - 1) + 1 : unused_counter; }
NOINLINE int cyc_b(int n) { return n ? cyc_a(n - 1) * 2 : unused_msg[n & 7]; }

/* Global but unreferenced; roots the cycle only when kept explicitly. */
NOINLINE int dead_entry(int x) {
    kprintf("dead entry %d\n", x); /* shares .rodata.str1.1 with a live string */
    return cyc_a(x) + unused_table[x & 7] + unused_buffer[x];
}

/* A chain reachable only through a (writable) table of function pointers. */
NOINLINE int chain3(int x) { return x * 3 + used_msg[x & 3]; }
NOINLINE int chain2(int x) { return chain3(x) + 2; }
NOINLINE int chain1(int x) { return chain2(x) + 1; }
NOINLINE int unused_leaf(int x) { return x - 1; }

int (*handlers[])(int) = {chain1, chain2};

__attribute__((section(".text.gc_root"))) int gc_main(void) {
    used_counter++;
    used_buffer[used_counter & 15] = (uint32_t)used_counter;
    kprintf("gc %d\n", used_counter);
    return handlers[used_counter & 1](used_counter);
}
