/*
 * Deliberately misbehaves to show that the kernel contains faults in user
 * tasks. The argument selects the fault:
 *   1 illegal instruction        4 jump into kernel code
 *   2 read kernel memory         5 access a device directly
 *   3 write outside the slot     6 execute data (stack)
 * Each one traps; the kernel reports the task's fault and kills only it.
 */
#include <strata/layout.h>
#include <strata/machine.h>
#include <strata/user.h>

int main(int kind)
{
    printf("crash: fault %d\n", kind);
    switch (kind) {
    case 1:
        __asm__ volatile(".word 0");
        break;
    case 2:
        return (int)*(volatile uint32_t *)KERNEL_BASE;
    case 3:
        *(volatile uint32_t *)SLOT0_BASE = 0;
        break;
    case 4:
        ((void (*)(void))KERNEL_BASE)();
        break;
    case 5:
        mmio_write(UART_BASE, UART_DATA, '!');
        break;
    case 6: {
        uint32_t code[1] = {0x00000013};   /* nop */
        ((void (*)(void))code)();
        break;
    }
    default:
        puts_("usage: crash <1-6>\n");
        return 1;
    }
    puts_("crash: survived (this should not happen)\n");
    return 2;
}
