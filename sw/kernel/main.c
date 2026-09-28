/* Kernel initialization. */
#include "kernel.h"

void kmain(void) __attribute__((noreturn));

void kmain(void)
{
    uart_init();
    video_init();
    console_init();
    video_enable(1);
    timer_init();
    gpio_init();
    intc_init();
    flash_init();

    kprintf("\x1b[96mStrata\x1b[0m kernel 1.0: %u KiB RAM, %u MHz, flash id %06x\n",
            (unsigned)(mmio_read(SYS_BASE, SYS_RAM_BYTES) / 1024),
            (unsigned)(mmio_read(SYS_BASE, SYS_CLK_HZ) / 1000000), (unsigned)flash_jedec_id());

    int n = store_init();
    if (n < 0)
        panic("no program store in flash at 0x%08x", STORE_OFFSET);
    kprintf("program store: %d programs\n", n);

    if (task_spawn("shell", 0) < 0)
        panic("cannot start the shell");

    /* User mode takes timer and external interrupts; the kernel runs with
     * mstatus.MIE = 0 and polls them only when idle. */
    csr_write(mie, MIE_MTIE | MIE_MEIE);
    return_to_user(schedule());
}
