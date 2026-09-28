# Boot ROM layout (sw/boot).
memory ROM origin=0x00000000 length=4K
entry _start

output .text   in ROM { .text.start .text .text.* }
output .rodata in ROM align=4 { .rodata .rodata.* .srodata .srodata.* }
