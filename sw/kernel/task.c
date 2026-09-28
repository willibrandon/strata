/*
 * Tasks and scheduling.
 *
 * Each user program is linked for one slot of RAM and runs as one task in
 * U-mode. PMP confines a task to its slot: code and read-only data are
 * readable and executable, data and stack readable and writable, and the
 * text screen is added only while the task owns it. Scheduling is
 * round-robin with a 10 ms time slice from the machine timer.
 */
#include <string.h>

#include "kernel.h"

struct task tasks[MAX_TASKS];
struct task *current;

static int next_pid = 1;
static uint64_t slice_end;
static uint32_t run_start;

static const uint32_t slot_base[NUM_SLOTS] = {SLOT0_BASE, SLOT1_BASE, SLOT2_BASE};
static const uint32_t slot_size[NUM_SLOTS] = {SLOT0_SIZE, SLOT1_SIZE, SLOT2_SIZE};

/* VRAM and font RAM as one naturally aligned 128 KiB PMP region. */
#define SCREEN_PMPADDR ((VRAM_BASE >> 2) | ((0x20000 >> 3) - 1))

struct task *task_by_pid(int pid)
{
    for (int i = 0; i < MAX_TASKS; i++)
        if (tasks[i].state != T_FREE && tasks[i].pid == pid)
            return &tasks[i];
    return 0;
}

static int slot_of(uint32_t addr)
{
    for (int i = 0; i < NUM_SLOTS; i++)
        if (addr == slot_base[i])
            return i;
    return -1;
}

static void setup_pmp(struct task *t)
{
    const struct prog_header *h = t->prog;
    t->pmpaddr[0] = slot_base[t->slot] >> 2;
    t->pmpaddr[1] = h->rx_end >> 2;
    t->pmpaddr[2] = h->stack_top >> 2;
    t->pmpaddr[3] = SCREEN_PMPADDR;
    uint32_t cfg1 = PMP_TOR | PMP_R | PMP_X;
    uint32_t cfg2 = PMP_TOR | PMP_R | PMP_W;
    uint32_t cfg3 = t->owns_screen ? (PMP_NAPOT | PMP_R | PMP_W) : PMP_OFF;
    t->pmpcfg = (cfg1 << 8) | (cfg2 << 16) | (cfg3 << 24);
}

static void load_pmp(const struct task *t)
{
    csr_write(pmpaddr0, t->pmpaddr[0]);
    csr_write(pmpaddr1, t->pmpaddr[1]);
    csr_write(pmpaddr2, t->pmpaddr[2]);
    csr_write(pmpaddr3, t->pmpaddr[3]);
    csr_write(pmpcfg0, t->pmpcfg);
}

int user_range_ok(const struct task *t, uint32_t a, uint32_t len, int want_write)
{
    const struct prog_header *h = t->prog;
    uint32_t lo = want_write ? h->rx_end : slot_base[t->slot];
    uint32_t hi = h->stack_top;
    return a >= lo && a <= hi && len <= hi - a;
}

int task_spawn(const char *name, int parent)
{
    const struct store_entry *e = store_find(name);
    if (!e)
        return E_NOENT;
    int slot = slot_of(e->load_addr);
    if (slot < 0 || e->size > slot_size[slot])
        return E_INVAL;
    for (int i = 0; i < MAX_TASKS; i++)
        if (tasks[i].state != T_FREE && tasks[i].slot == slot)
            return E_BUSY;
    struct task *t = 0;
    for (int i = 0; i < MAX_TASKS && !t; i++)
        if (tasks[i].state == T_FREE)
            t = &tasks[i];
    if (!t)
        return E_BUSY;

    int err = store_load(e);
    if (err)
        return err;
    const struct prog_header *h = (const struct prog_header *)e->load_addr;
    if (h->magic != PROG_MAGIC || h->stack_top > slot_base[slot] + slot_size[slot] ||
        h->bss_end > h->stack_top || h->data_end > h->bss_end || h->rx_end > h->data_end)
        return E_INVAL;
    memset((void *)h->data_end, 0, h->bss_end - h->data_end);

    memset(t, 0, sizeof *t);
    t->pid = next_pid++;
    t->parent = parent;
    t->slot = slot;
    t->prog = h;
    t->pc = h->entry;
    t->regs[2] = h->stack_top;
    strncpy(t->name, h->name, sizeof t->name - 1);
    setup_pmp(t);
    t->state = T_READY;
    return t->pid;
}

void task_grant_screen(struct task *t, int on)
{
    if (t->owns_screen == on)
        return;
    t->owns_screen = on;
    setup_pmp(t);
    console_set_visible(!on);
    if (!on) {
        video_init();   /* restore the font and palette the task may have changed */
    }
}

static void free_task(struct task *t)
{
    t->state = T_FREE;
}

void task_exit(struct task *t, int code)
{
    if (t->owns_screen)
        task_grant_screen(t, 0);
    /* Orphans are freed when they exit; exited children are reaped now. */
    for (int i = 0; i < MAX_TASKS; i++) {
        struct task *c = &tasks[i];
        if (c->state != T_FREE && c != t && c->parent == t->pid) {
            if (c->state == T_ZOMBIE)
                free_task(c);
            else
                c->parent = -1;
        }
    }
    t->exit_code = code;
    t->state = T_ZOMBIE;
    struct task *p = task_by_pid(t->parent);
    if (p && p->state == T_WAITING && (p->wait_pid == t->pid || p->wait_pid == 0)) {
        p->regs[10] = (uint32_t)code;
        p->state = T_READY;
        free_task(t);
    } else if (!p) {
        free_task(t);
    }
    /* The first program is the shell: keep one running. */
    if (t->parent == 0 && t->state == T_FREE && !task_by_pid(t->pid)) {
        kprintf("\n[kernel] shell exited with %d; restarting\n", code);
        if (task_spawn("shell", 0) < 0)
            panic("cannot restart the shell");
    }
}

void task_kill(struct task *t, const char *why)
{
    kprintf("\x1b[91m[kernel] %s (pid %d): %s\x1b[0m\n", t->name, t->pid, why);
    task_exit(t, -1);
}

/* Ctrl-C on the terminal: ends the program the shell is waiting for. When
 * the shell is not waiting for one, it gets the key and discards its line. */
void task_interrupt(void)
{
    for (int i = 0; i < MAX_TASKS; i++) {
        struct task *s = &tasks[i];
        if (s->state != T_WAITING || s->parent != 0)
            continue;
        struct task *fg = task_by_pid(s->wait_pid);
        if (fg && fg->state != T_ZOMBIE) {
            kprintf("^C\n");
            task_exit(fg, EXIT_INTERRUPTED);
            return;
        }
    }
    input_put(0x03);
}

void task_block(struct task *t, int state)
{
    t->state = state;
}

void task_wake_sleepers(uint64_t now)
{
    for (int i = 0; i < MAX_TASKS; i++) {
        struct task *t = &tasks[i];
        if (t->state == T_SLEEPING && now >= t->wake_at) {
            t->regs[10] = 0;
            t->state = T_READY;
        }
    }
}

/* Arms the timer for the earlier of the end of the time slice and the first
 * sleeper's wake-up time. */
static void arm_timer(int running)
{
    uint64_t next = running ? slice_end : ~0ull;
    for (int i = 0; i < MAX_TASKS; i++)
        if (tasks[i].state == T_SLEEPING && tasks[i].wake_at < next)
            next = tasks[i].wake_at;
    timer_set(next);
}

/* Ends the current time slice at the next scheduling decision. */
void task_yield(void)
{
    slice_end = 0;
}

/* Charges the running task for the cycles since it was resumed. */
void task_account(struct task *t)
{
    if (t)
        t->cycles += (uint32_t)csr_read(mcycle) - run_start;
}

/* Picks the next task to run. Keeps the current task until its slice ends,
 * then rotates round-robin. With nothing to run, waits for an interrupt. */
struct task *schedule(void)
{
    for (;;) {
        uint64_t now = timer_now();
        struct task *next = 0;
        if (current && current->state == T_READY && now < slice_end) {
            next = current;
        } else {
            int start = current ? (int)(current - tasks) + 1 : 0;
            for (int i = 0; i < MAX_TASKS && !next; i++) {
                struct task *t = &tasks[(start + i) % MAX_TASKS];
                if (t->state == T_READY)
                    next = t;
            }
            if (next)
                slice_end = now + (uint64_t)QUANTUM_MS * (CLK_HZ / 1000);
        }
        if (next) {
            current = next;
            load_pmp(next);
            arm_timer(1);
            run_start = csr_read(mcycle);
            return next;
        }
        /* Idle: interrupts stay disabled in M-mode; WFI still wakes on any
         * pending, enabled interrupt, which is then handled here. */
        current = 0;
        arm_timer(0);
        __asm__ volatile("wfi");
        handle_interrupts();
    }
}
