/* Trap handling: interrupts, system calls, and faults in user tasks. */
#include <fmt.h>
#include <stdarg.h>
#include <string.h>

#include "kernel.h"

#define MIP_MTIP (1u << 7)
#define MIP_MEIP (1u << 11)

static const char *const cause_names[] = {
    "instruction address misaligned", "instruction access fault", "illegal instruction",
    "breakpoint", "load address misaligned", "load access fault", "store address misaligned",
    "store access fault", "environment call from U-mode", "?", "?", "environment call from M-mode",
};

/* Gives waiting readers the input that has arrived. The task that owns the
 * screen gets keys first. */
static void deliver_input(void)
{
    while (input_available()) {
        struct task *r = 0;
        for (int i = 0; i < MAX_TASKS; i++) {
            struct task *t = &tasks[i];
            if (t->state == T_READING && (!r || t->owns_screen))
                r = t;
        }
        if (!r)
            return;
        int key = input_get();
        if (r->regs[17] == SC_GETKEY) {
            r->regs[10] = (uint32_t)key;
        } else {
            /* SC_READ: one byte per wake-up; special keys are dropped. */
            if (key >= 0x100)
                continue;
            *(uint8_t *)r->regs[10] = (uint8_t)key;
            r->regs[10] = 1;
        }
        r->state = T_READY;
    }
}

void video_irq(void)
{
    mmio_write(VIDEO_BASE, VIDEO_STATUS, VIDEO_VBLANK);
    uint32_t frame = video_frame();
    for (int i = 0; i < MAX_TASKS; i++) {
        if (tasks[i].state == T_VSYNC) {
            tasks[i].regs[10] = frame;
            tasks[i].state = T_READY;
        }
    }
}

void handle_interrupts(void)
{
    uint32_t mip = csr_read(mip);
    if (mip & MIP_MTIP) {
        task_wake_sleepers(timer_now());
        timer_set(~0ull);   /* re-armed by schedule() */
    }
    if (mip & MIP_MEIP) {
        uint32_t p = mmio_read(INTC_BASE, INTC_PENDING);
        if (p & IRQ_UART)
            input_poll();
        if (p & IRQ_GPIO)
            gpio_irq();
        if (p & IRQ_VIDEO)
            video_irq();
        deliver_input();
    }
}

struct task *trap_handler(struct task *t)
{
    task_account(t);
    uint32_t cause = csr_read(mcause);
    if (cause & MCAUSE_INTR) {
        handle_interrupts();
    } else if (cause == EXC_ECALL_U) {
        t->pc += 4;
        syscall(t);
    } else {
        char msg[96];
        snprintf(msg, sizeof msg, "%s at pc 0x%08x (mtval 0x%08x)",
                 cause < 12 ? cause_names[cause] : "unknown exception", t->pc, (unsigned)csr_read(mtval));
        task_kill(t, msg);
    }
    /* Ctrl-C is acted on here, after the interrupt or system call, so that a
     * task is never ended in the middle of its own system call. */
    if (input_take_interrupt()) {
        task_interrupt();
        deliver_input();
    }
    return schedule();
}

/* ---------------------------------------------------------------- system calls */

static uint32_t copy_name(const struct task *t, uint32_t uaddr, char *out, size_t size)
{
    for (size_t i = 0; i < size; i++) {
        if (!user_range_ok(t, uaddr + i, 1, 0))
            return E_FAULT;
        out[i] = *(const char *)(uaddr + i);
        if (!out[i])
            return 0;
    }
    out[size - 1] = 0;
    return 0;
}

struct task *syscall(struct task *t)
{
    uint32_t *a = &t->regs[10];   /* a0..a5 */
    uint32_t nr = t->regs[17];
    int32_t ret = 0;

    switch (nr) {
    case SC_EXIT:
        task_exit(t, (int)a[0]);
        return t;
    case SC_WRITE:
        if (!user_range_ok(t, a[0], a[1], 0)) {
            ret = E_FAULT;
            break;
        }
        console_write((const char *)a[0], a[1]);
        ret = (int32_t)a[1];
        break;
    case SC_READ:
        if (a[1] == 0 || !user_range_ok(t, a[0], 1, 1)) {
            ret = E_FAULT;
            break;
        }
        input_poll();
        if (input_available()) {
            int k;
            while ((k = input_get()) >= 0x100)
                ;
            if (k >= 0) {
                *(uint8_t *)a[0] = (uint8_t)k;
                ret = 1;
                break;
            }
        }
        if (!a[2]) {
            ret = 0;
            break;
        }
        task_block(t, T_READING);
        return t;
    case SC_GETKEY:
        input_poll();
        if (input_available()) {
            ret = input_get();
            break;
        }
        if (!a[0]) {
            ret = -1;
            break;
        }
        task_block(t, T_READING);
        return t;
    case SC_SLEEP:
        t->wake_at = timer_now() + (uint64_t)a[0] * (CLK_HZ / 1000);
        task_block(t, T_SLEEPING);
        return t;
    case SC_TIME:
        ret = (int32_t)timer_ms();
        break;
    case SC_YIELD:
        task_yield();
        break;
    case SC_SPAWN: {
        char name[16];
        if (copy_name(t, a[0], name, sizeof name)) {
            ret = E_FAULT;
            break;
        }
        ret = task_spawn(name, t->pid);
        if (ret > 0)
            task_by_pid(ret)->regs[10] = a[1];   /* argument for main() */
        break;
    }
    case SC_WAIT: {
        struct task *c = task_by_pid((int)a[0]);
        if (!c || c->parent != t->pid) {
            ret = E_NOENT;
            break;
        }
        if (c->state == T_ZOMBIE) {
            ret = c->exit_code;
            c->state = T_FREE;
            break;
        }
        t->wait_pid = c->pid;
        task_block(t, T_WAITING);
        return t;
    }
    case SC_KILL: {
        struct task *c = task_by_pid((int)a[0]);
        if (!c || c->state == T_ZOMBIE) {
            ret = E_NOENT;
            break;
        }
        task_kill(c, "killed");
        if (c == t)
            return t;
        break;
    }
    case SC_PS: {
        uint32_t max = a[1];
        if (!user_range_ok(t, a[0], max * sizeof(struct ps_entry), 1)) {
            ret = E_FAULT;
            break;
        }
        struct ps_entry *out = (struct ps_entry *)a[0];
        uint32_t n = 0;
        for (int i = 0; i < MAX_TASKS && n < max; i++) {
            struct task *x = &tasks[i];
            if (x->state == T_FREE)
                continue;
            out[n].pid = x->pid;
            out[n].state = x->state;
            out[n].slot = x->slot;
            out[n].cycles_lo = (uint32_t)x->cycles;
            out[n].cycles_hi = (uint32_t)(x->cycles >> 32);
            memcpy(out[n].name, x->name, sizeof out[n].name);
            n++;
        }
        ret = (int32_t)n;
        break;
    }
    case SC_SCREEN: {
        int on = a[0] != 0;
        for (int i = 0; i < MAX_TASKS; i++)
            if (on && tasks[i].owns_screen && &tasks[i] != t) {
                ret = E_BUSY;
                goto done;
            }
        task_grant_screen(t, on);
        ret = on ? (int32_t)VRAM_BASE : 0;
        break;
    }
    case SC_VSYNC:
        task_block(t, T_VSYNC);
        return t;
    case SC_PALETTE:
        if (!t->owns_screen) {
            ret = E_PERM;
            break;
        }
        video_set_palette(a[0], a[1]);
        break;
    case SC_LEDS:
        gpio_leds(a[0]);
        break;
    case SC_BUTTONS:
        ret = (int32_t)gpio_buttons();
        break;
    case SC_INFO: {
        if (!user_range_ok(t, a[0], sizeof(struct sys_info), 1)) {
            ret = E_FAULT;
            break;
        }
        struct sys_info *s = (struct sys_info *)a[0];
        uint64_t c = read_cycle64();
        s->cycle_lo = (uint32_t)c;
        s->cycle_hi = (uint32_t)(c >> 32);
        s->instret_lo = csr_read(minstret);
        s->instret_hi = csr_read(minstreth);
        s->time_ms = (uint32_t)timer_ms();
        s->clk_hz = mmio_read(SYS_BASE, SYS_CLK_HZ);
        s->ram_bytes = mmio_read(SYS_BASE, SYS_RAM_BYTES);
        s->flash_id = flash_jedec_id();
        s->frames = video_frame();
        break;
    }
    case SC_PROGRAMS: {
        const struct store_entry *e = store_entry((int)a[0]);
        if (!e) {
            ret = -1;
            break;
        }
        if (!user_range_ok(t, a[1], sizeof e->name, 1)) {
            ret = E_FAULT;
            break;
        }
        memcpy((void *)a[1], e->name, sizeof e->name);
        break;
    }
    case SC_HALT:
        uart_drain();
        mmio_write(SYS_BASE, SYS_HALT, a[0]);
        for (;;)
            ;
    case SC_RESET:
        uart_drain();
        mmio_write(SYS_BASE, SYS_RESET, SYS_RESET_MAGIC);
        for (;;)
            ;
    case SC_GETPID:
        ret = t->pid;
        break;
    default:
        ret = E_INVAL;
        break;
    }
done:
    a[0] = (uint32_t)ret;
    return t;
}

/* ---------------------------------------------------------------- fatal errors */

void panic(const char *fmt, ...)
{
    va_list ap;
    char buf[128];
    va_start(ap, fmt);
    vsnprintf(buf, sizeof buf, fmt, ap);
    va_end(ap);
    console_set_visible(1);
    kprintf("\n\x1b[97;41m KERNEL PANIC \x1b[0m %s\n", buf);
    uart_drain();
    mmio_write(SYS_BASE, SYS_HALT, 0xEE);
    for (;;)
        ;
}

/* Called from trap.S for a trap taken while the kernel itself was running. */
void kernel_fault(void)
{
    uint32_t cause = csr_read(mcause);
    panic("trap in kernel: mcause 0x%08x mepc 0x%08x mtval 0x%08x", (unsigned)cause,
          (unsigned)csr_read(mepc), (unsigned)csr_read(mtval));
}
