/*
 * User program entry and header. The kernel reads the header at the base of
 * the program's slot, zeroes .bss, points sp at the top of the slot and
 * starts _start in U-mode with a0 = the argument given to SC_SPAWN.
 * PROG_NAME is defined on the preprocessor command line.
 */
#include <strata/layout.h>
#include <strata/syscall.h>

    .section .text.header, "ax"
    .globl __prog_header
__prog_header:
    .word   PROG_MAGIC
    .word   _start
    .word   __rx_end
    .word   __data_end
    .word   __bss_end
    .word   __stack_top
1:  .ascii  PROG_NAME
    .fill   16 - (. - 1b), 1, 0

    .text
    .globl _start
_start:
    call    main
    li      a7, SC_EXIT
    ecall
2:  j       2b
