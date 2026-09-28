/* User-mode library: system call wrappers and console output. */
#ifndef STRATA_USER_H
#define STRATA_USER_H

#include <stddef.h>
#include <stdint.h>
#include <strata/syscall.h>

static inline int32_t syscall3(int nr, uint32_t a, uint32_t b, uint32_t c)
{
    register uint32_t a0 __asm__("a0") = a;
    register uint32_t a1 __asm__("a1") = b;
    register uint32_t a2 __asm__("a2") = c;
    register uint32_t a7 __asm__("a7") = (uint32_t)nr;
    __asm__ volatile("ecall" : "+r"(a0) : "r"(a1), "r"(a2), "r"(a7) : "memory");
    return (int32_t)a0;
}

static inline void     sys_exit(int code) { syscall3(SC_EXIT, (uint32_t)code, 0, 0); for (;;) ; }
static inline int      sys_write(const void *buf, size_t n) { return syscall3(SC_WRITE, (uint32_t)buf, n, 0); }
static inline int      sys_read(void *buf, size_t n, int block) { return syscall3(SC_READ, (uint32_t)buf, n, (uint32_t)block); }
static inline int      sys_getkey(int block) { return syscall3(SC_GETKEY, (uint32_t)block, 0, 0); }
static inline void     sys_sleep(uint32_t ms) { syscall3(SC_SLEEP, ms, 0, 0); }
static inline uint32_t sys_time(void) { return (uint32_t)syscall3(SC_TIME, 0, 0, 0); }
static inline void     sys_yield(void) { syscall3(SC_YIELD, 0, 0, 0); }
static inline int      sys_spawn(const char *name, uint32_t arg) { return syscall3(SC_SPAWN, (uint32_t)name, arg, 0); }
static inline int      sys_wait(int pid) { return syscall3(SC_WAIT, (uint32_t)pid, 0, 0); }
static inline int      sys_kill(int pid) { return syscall3(SC_KILL, (uint32_t)pid, 0, 0); }
static inline int      sys_ps(struct ps_entry *out, int max) { return syscall3(SC_PS, (uint32_t)out, (uint32_t)max, 0); }
static inline volatile uint16_t *sys_screen(int on) { return (volatile uint16_t *)syscall3(SC_SCREEN, (uint32_t)on, 0, 0); }
static inline uint32_t sys_vsync(void) { return (uint32_t)syscall3(SC_VSYNC, 0, 0, 0); }
static inline int      sys_palette(unsigned i, uint32_t rgb) { return syscall3(SC_PALETTE, i, rgb, 0); }
static inline void     sys_leds(uint32_t v) { syscall3(SC_LEDS, v, 0, 0); }
static inline uint32_t sys_buttons(void) { return (uint32_t)syscall3(SC_BUTTONS, 0, 0, 0); }
static inline int      sys_info(struct sys_info *s) { return syscall3(SC_INFO, (uint32_t)s, 0, 0); }
static inline int      sys_programs(int i, char name[16]) { return syscall3(SC_PROGRAMS, (uint32_t)i, (uint32_t)name, 0); }
static inline void     sys_halt(int code) { syscall3(SC_HALT, (uint32_t)code, 0, 0); }
static inline void     sys_reset(void) { syscall3(SC_RESET, 0, 0, 0); }
static inline int      sys_getpid(void) { return syscall3(SC_GETPID, 0, 0, 0); }

int  printf(const char *fmt, ...) __attribute__((format(printf, 1, 2)));
void putchar_(int c);
void puts_(const char *s);

#endif
