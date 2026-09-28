/* Strata kernel: internal interfaces. */
#ifndef KERNEL_H
#define KERNEL_H

#include <stdint.h>
#include <stddef.h>
#include <strata/machine.h>
#include <strata/csr.h>
#include <strata/layout.h>
#include <strata/syscall.h>

#define MAX_TASKS   NUM_SLOTS
#define QUANTUM_MS  10

/* ---------------------------------------------------------------- tasks */
enum task_state {
    T_FREE,
    T_READY,
    T_SLEEPING,       /* until wake_at */
    T_READING,        /* blocked in SYS_READ until input arrives */
    T_VSYNC,          /* blocked in SYS_VSYNC until the next vertical blank */
    T_WAITING,        /* blocked in SYS_WAIT for a child */
    T_ZOMBIE,
};

/* The first 33 words are used by trap.S: x0-x31 (x0 unused) and the pc. */
struct task {
    uint32_t regs[32];
    uint32_t pc;
    int      state;
    int      pid;
    int      parent;
    int      slot;
    int      exit_code;
    int      wait_pid;
    uint64_t wake_at;
    uint64_t cycles;          /* CPU cycles consumed */
    uint32_t pmpaddr[4];
    uint32_t pmpcfg;
    int      owns_screen;
    const struct prog_header *prog;
    char     name[16];
};

extern struct task tasks[MAX_TASKS];
extern struct task *current;

struct task *task_by_pid(int pid);
int  task_spawn(const char *name, int parent);
void task_exit(struct task *t, int code);
void task_kill(struct task *t, const char *why);
void task_wake_sleepers(uint64_t now);
void task_account(struct task *t);
void task_yield(void);
struct task *schedule(void);
void task_block(struct task *t, int state);
void task_grant_screen(struct task *t, int on);

/* ---------------------------------------------------------------- program store */
int  store_init(void);
int  store_count(void);
const struct store_entry *store_entry(int i);
const struct store_entry *store_find(const char *name);
int  store_load(const struct store_entry *e);

/* ---------------------------------------------------------------- devices */
void     uart_init(void);
void     uart_putc(char c);
int      uart_getc(void);                 /* -1 if none */
void     uart_drain(void);
void     timer_init(void);
uint64_t timer_now(void);
uint64_t timer_ms(void);
void     timer_set(uint64_t when);
void     gpio_init(void);
uint32_t gpio_buttons(void);
void     gpio_leds(uint32_t v);
void     gpio_irq(void);
void     video_init(void);
void     video_enable(int on);
void     video_irq(void);
uint32_t video_frame(void);
void     video_set_palette(unsigned i, uint32_t rgb);
void     flash_init(void);
int      flash_read(uint32_t addr, void *dst, uint32_t len);
uint32_t flash_jedec_id(void);
void     intc_init(void);
uint32_t crc32(uint32_t crc, const void *p, uint32_t len);

/* ---------------------------------------------------------------- input */
void input_uart_byte(int c);
void input_buttons(uint32_t pressed);
int  input_get(void);                     /* key code or -1 */
int  input_available(void);
void input_poll(void);

/* ---------------------------------------------------------------- console */
void console_init(void);
void console_putc(char c);
void console_write(const char *s, size_t n);
void console_redraw(void);
void console_set_visible(int on);
void kprintf(const char *fmt, ...) __attribute__((format(printf, 1, 2)));

/* ---------------------------------------------------------------- traps */
struct task *trap_handler(struct task *t);
struct task *syscall(struct task *t);
void handle_interrupts(void);
void panic(const char *fmt, ...) __attribute__((noreturn, format(printf, 1, 2)));
void return_to_user(struct task *t) __attribute__((noreturn));

/* Addresses in [a, a+len) that a task may read (write if want_write). */
int user_range_ok(const struct task *t, uint32_t a, uint32_t len, int want_write);

extern const unsigned char strata_font[256 * 16];

#endif
