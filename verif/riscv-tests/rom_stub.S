/* Boot ROM for ISA tests: jump to the test preloaded at RAM base. */
    .text
    .globl _start
_start:
    lui     t0, 0x10000
    jr      t0
