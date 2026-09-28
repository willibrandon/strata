/* Prints a greeting and some facts about the machine it is running on. */
#include <strata/user.h>

int main(int arg)
{
    struct sys_info s;
    sys_info(&s);
    printf("Hello from pid %d in user mode.\n", sys_getpid());
    printf("The machine has run %u ms (%u video frames).\n", (unsigned)s.time_ms, (unsigned)s.frames);
    return arg;
}
