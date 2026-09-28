/* Counts on the LEDs every 100 ms, forever or for <arg> steps. Run it in the
 * background ('counter &') to see preemptive multitasking with the shell. */
#include <strata/user.h>

int main(int steps)
{
    for (uint32_t i = 1; steps == 0 || i <= (uint32_t)steps; i++) {
        sys_leds(i);
        sys_sleep(100);
    }
    return 0;
}
