/* Differential-test program: MMIO access, formatted output with varargs,
 * a state machine, static locals, struct copies, and zero-initialized data. */
#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>

#define UART_BASE 0x40001000u
#define UART_DATA (*(volatile uint32_t *)(UART_BASE + 0x00))
#define UART_STATUS (*(volatile uint32_t *)(UART_BASE + 0x04))
#define SYS_HALT (*(volatile uint32_t *)0x40000020u)

struct task {
    uint32_t regs[8];
    void (*entry)(void);
    uint32_t ticks;
    uint8_t state;
};

static struct task tasks[4];
static uint32_t tick_count;
uint8_t rx_buffer[64];
volatile uint32_t irq_count;
static const uint8_t hex_digits[16] = "0123456789abcdef";

void uart_putc(char c) {
    while (UART_STATUS & 1u)
        ;
    UART_DATA = (uint8_t)c;
}

void trace(int event, int value) {
    static int last_event = -1;
    if (event != last_event) {
        uart_putc('#');
        uart_putc((char)('0' + event));
        last_event = event;
    }
    irq_count += (uint32_t)value;
}

static void print_unsigned(uint32_t v, unsigned base) {
    char buf[12];
    int n = 0;
    do {
        buf[n++] = (char)hex_digits[v % base];
        v /= base;
    } while (v);
    while (n)
        uart_putc(buf[--n]);
}

void kprintf(const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    for (; *fmt; fmt++) {
        if (*fmt != '%') {
            uart_putc(*fmt);
            continue;
        }
        switch (*++fmt) {
        case 'd': {
            int v = va_arg(ap, int);
            if (v < 0) {
                uart_putc('-');
                v = -v;
            }
            print_unsigned((uint32_t)v, 10);
            break;
        }
        case 'u': print_unsigned(va_arg(ap, unsigned), 10); break;
        case 'x': print_unsigned(va_arg(ap, unsigned), 16); break;
        case 'c': uart_putc((char)va_arg(ap, int)); break;
        case 's': {
            const char *s = va_arg(ap, const char *);
            while (*s)
                uart_putc(*s++);
            break;
        }
        case '%': uart_putc('%'); break;
        default: uart_putc('?'); break;
        }
    }
    va_end(ap);
}

static void idle(void) { tick_count++; }
static void worker(void) { tick_count += 3; }

enum { T_FREE, T_READY, T_RUNNING, T_BLOCKED };

bool schedule(void) {
    static unsigned next;
    for (unsigned i = 0; i < 4; i++) {
        struct task *t = &tasks[(next + i) & 3];
        switch (t->state) {
        case T_READY:
            t->state = T_RUNNING;
            t->entry();
            t->ticks++;
            t->state = T_READY;
            next = (next + i + 1) & 3;
            return true;
        case T_BLOCKED:
            if (t->ticks < tick_count)
                t->state = T_READY;
            break;
        default:
            break;
        }
    }
    return false;
}

int kernel_main(void) {
    struct task proto = {{0}, idle, 0, T_READY};
    for (int i = 0; i < 4; i++) {
        tasks[i] = proto;
        tasks[i].entry = (i & 1) ? worker : idle;
        tasks[i].regs[i] = (uint32_t)i * 0x01010101u;
    }
    for (int i = 0; i < 100; i++)
        schedule();
    kprintf("ticks=%u irq=%x %s %c %d%%\n", tick_count, irq_count, "ok", 'Z', -12345);
    return (int)tick_count;
}

void _halt(int code) {
    SYS_HALT = (uint32_t)code;
    for (;;)
        ;
}
