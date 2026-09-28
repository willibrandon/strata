# Startup code and stub runtime helpers for the differential C programs.
# Must assemble identically with strata-as and llvm-mc.

	.equ STACK_SIZE, 1024

	.macro STUB name, result=0
	.globl \name
	.type \name, @function
	.p2align 2
\name:
	li a0, \result
	li a1, 0
	ret
	.size \name, . - \name
	.endm

	.section .text.start, "ax", @progbits
	.globl _start
	.type _start, @function
_start:
	la sp, stack_top
	lui gp, %hi(stack)
	addi gp, gp, %lo(stack)
	call sort_main
	mv s0, a0
	call kernel_main
	add s0, s0, a0
	call math_main
	add a0, a0, s0
	call _halt
1:	wfi
	j 1b
	.size _start, . - _start

	.text
	.irp f, __mulsi3, __divsi3, __udivsi3, __modsi3, __umodsi3, __muldi3, __divdi3, __udivdi3, __moddi3, __umoddi3
	STUB \f
	.endr
	STUB memcpy, 1
	STUB memset, 2
	STUB memmove, 3
	STUB memcmp

	.p2align 4
counters:
	.rept 3
	csrr a0, cycle
	csrr a1, instret
	.endr
	ret

	.section .rodata.runtime, "a", @progbits
	.p2align 2
vectors:
	.word _start, counters, memcpy + 4, stack_top - stack
	.half 0x1234, -2
	.byte 1, 2, 3, 'x'
	.ascii "rt\n"
	.asciz "runtime"

	.bss
	.p2align 4
stack:
	.zero STACK_SIZE
stack_top:
