/*
 * Strata boot ROM.
 *
 * Runs at reset from 0x00000000 in M-mode. Loads a boot image (format:
 * docs/toolchain.md#boot-image-format) from the SPI flash at
 * FLASH_IMAGE_OFFSET into RAM, checking the CRC-32 of the header and of every
 * segment, then jumps to the image's entry point. If the flash holds no
 * valid image, the same format is accepted on the UART instead.
 *
 * The ROM uses no RAM except a scratch area in the top 512 bytes (the stack
 * and the segment table); images must not load there.
 *
 * Register use:
 *   s0  byte source: address of flash_getc or uart_getc
 *   s1  running CRC-32
 *   s2  CRC table address
 *   s3  segment count
 *   s4  entry point
 *   s5  loop pointer / segment table cursor
 *   s6  bytes remaining in a segment
 *   s7  destination address
 */
#include <strata/machine.h>

#define SCRATCH_TOP   (RAM_BASE + RAM_BYTES)
#define SEGTAB        (SCRATCH_TOP - 512)
#define MAX_SEGMENTS  15

    .section .text.start, "ax"
    .globl _start
_start:
    li      sp, SCRATCH_TOP
    la      t0, boot_trap
    csrw    mtvec, t0
    la      s2, crc_table

    la      a0, msg_banner
    jal     puts

    /* Check that this is a Strata machine. */
    li      t0, SYS_BASE
    lw      t1, SYS_ID(t0)
    li      t2, SYS_ID_VALUE
    beq     t1, t2, 1f
    li      a0, 0x1E
    j       halt
1:
    /* Flash: wake from power-down, then READ from the image offset. */
    li      t0, SPI_BASE
    li      t1, SPI_CS
    sw      t1, SPI_CTRL(t0)
    li      a0, 0xAB
    jal     spi_xfer
    li      t0, SPI_BASE
    sw      zero, SPI_CTRL(t0)
    li      t1, SPI_CS
    sw      t1, SPI_CTRL(t0)
    li      a0, 0x03
    jal     spi_xfer
    li      a0, (FLASH_IMAGE_OFFSET >> 16) & 0xFF
    jal     spi_xfer
    li      a0, (FLASH_IMAGE_OFFSET >> 8) & 0xFF
    jal     spi_xfer
    li      a0, FLASH_IMAGE_OFFSET & 0xFF
    jal     spi_xfer

    la      s0, flash_getc
    la      a0, msg_flash
    jal     puts
    jal     load_image
    /* load_image returns only on failure, with a0 = message. */
    jal     puts
    li      t0, SPI_BASE
    sw      zero, SPI_CTRL(t0)

uart_boot:
    la      a0, msg_uart
    jal     puts
    la      s0, uart_getc
    /* Resynchronize on the magic number, one byte at a time. */
    li      s5, 0
2:  jalr    s0
    slli    s5, s5, 8
    or      s5, s5, a0
    li      t0, (BOOT_MAGIC & 0xFF) << 24 | (BOOT_MAGIC >> 8 & 0xFF) << 16 | (BOOT_MAGIC >> 16 & 0xFF) << 8 | (BOOT_MAGIC >> 24)
    bne     s5, t0, 2b
    la      a0, msg_receiving
    jal     puts
    jal     load_image_after_magic
    jal     puts
    j       uart_boot

/*
 * load_image: reads an image from the byte source in s0. On success it does
 * not return: it jumps to the entry point. On failure it returns with a0
 * pointing at an error message. Clobbers s1, s3-s7, t*, a*.
 */
load_image:
    mv      s8, ra
    li      s1, -1
    jal     getw_crc
    li      t0, BOOT_MAGIC
    la      t1, err_magic
    bne     a0, t0, fail
    j       1f
load_image_after_magic:
    /* The magic has been consumed: account for it in the CRC. */
    mv      s8, ra
    li      s1, -1
    li      a0, BOOT_MAGIC & 0xFF
    jal     crc_byte
    li      a0, BOOT_MAGIC >> 8 & 0xFF
    jal     crc_byte
    li      a0, BOOT_MAGIC >> 16 & 0xFF
    jal     crc_byte
    li      a0, BOOT_MAGIC >> 24
    jal     crc_byte
1:
    jal     getw_crc                /* version */
    li      t0, 1
    la      t1, err_version
    bne     a0, t0, fail
    jal     getw_crc                /* entry */
    mv      s4, a0
    jal     getw_crc                /* segment count */
    mv      s3, a0
    la      t1, err_nseg
    beqz    s3, fail
    li      t0, MAX_SEGMENTS
    bgtu    s3, t0, fail
    /* Header CRC field: read it, but feed zeros into the running CRC. */
    jal     getw
    mv      s9, a0
    li      s5, 4
2:  li      a0, 0
    jal     crc_byte
    addi    s5, s5, -1
    bnez    s5, 2b
    /* Segment table, saved to the scratch area. */
    li      s5, SEGTAB
    mv      s6, s3
3:  jal     getw_crc
    sw      a0, 0(s5)
    jal     getw_crc
    sw      a0, 4(s5)
    jal     getw_crc
    sw      a0, 8(s5)
    addi    s5, s5, 12
    addi    s6, s6, -1
    bnez    s6, 3b
    not     t0, s1
    la      t1, err_hcrc
    bne     t0, s9, fail

    /* Segments. */
    li      s5, SEGTAB
4:  lw      s7, 0(s5)               /* address */
    lw      s6, 4(s5)               /* length */
    /* Must be word aligned and lie in RAM below the scratch area. */
    or      t0, s7, s6
    andi    t0, t0, 3
    la      t1, err_range
    bnez    t0, fail
    li      t0, RAM_BASE
    bltu    s7, t0, fail
    add     t2, s7, s6
    bltu    t2, s7, fail
    li      t0, SEGTAB
    bgtu    t2, t0, fail
    li      s1, -1
5:  beqz    s6, 6f
    jalr    s0
    sb      a0, 0(s7)
    jal     crc_byte
    addi    s7, s7, 1
    addi    s6, s6, -1
    j       5b
6:  not     t0, s1
    lw      t1, 8(s5)
    bne     t0, t1, bad_segment
    addi    s5, s5, 12
    addi    s3, s3, -1
    bnez    s3, 4b

    /* Done: release the flash, report, and enter the image. */
    li      t0, SPI_BASE
    sw      zero, SPI_CTRL(t0)
    la      a0, msg_entry
    jal     puts
    mv      a0, s4
    jal     puthex
    la      a0, msg_crlf
    jal     puts
    jal     uart_drain
    fence.i
    jr      s4

bad_segment:
    la      t1, err_scrc
fail:
    mv      a0, t1
    jr      s8

/* getw: a0 = next little-endian word from the source (no CRC). */
getw:
    mv      s10, ra
    li      s11, 0
    li      t5, 0
1:  jalr    s0
    sll     a0, a0, t5
    or      s11, s11, a0
    addi    t5, t5, 8
    li      t0, 32
    bne     t5, t0, 1b
    mv      a0, s11
    jr      s10

/* getw_crc: like getw, and feeds the bytes into the CRC. */
getw_crc:
    mv      s10, ra
    li      s11, 0
    li      t5, 0
1:  jalr    s0
    sll     t6, a0, t5
    or      s11, s11, t6
    jal     crc_byte
    addi    t5, t5, 8
    li      t0, 32
    bne     t5, t0, 1b
    mv      a0, s11
    jr      s10

/* crc_byte: s1 = CRC-32 update of s1 with byte a0. Clobbers t0. */
crc_byte:
    xor     t0, s1, a0
    andi    t0, t0, 0xFF
    slli    t0, t0, 2
    add     t0, t0, s2
    lw      t0, 0(t0)
    srli    s1, s1, 8
    xor     s1, s1, t0
    ret

/* spi_xfer: exchange one byte; a0 = byte out, returns byte in. */
spi_xfer:
    li      t0, SPI_BASE
    sw      a0, SPI_DATA(t0)
1:  lw      t1, SPI_STATUS(t0)
    andi    t1, t1, SPI_BUSY
    bnez    t1, 1b
    lw      a0, SPI_DATA(t0)
    ret

/* flash_getc: next byte of the READ stream. */
flash_getc:
    li      t0, SPI_BASE
    sw      zero, SPI_DATA(t0)
1:  lw      t1, SPI_STATUS(t0)
    andi    t1, t1, SPI_BUSY
    bnez    t1, 1b
    lw      a0, SPI_DATA(t0)
    ret

/* uart_getc: wait for a received byte. */
uart_getc:
    li      t0, UART_BASE
1:  lw      a0, UART_DATA(t0)
    andi    t1, a0, UART_RX_VALID
    beqz    t1, 1b
    andi    a0, a0, 0xFF
    ret

/* puts: write the NUL-terminated string at a0. Clobbers t0-t2. */
puts:
    li      t0, UART_BASE
1:  lbu     t1, 0(a0)
    beqz    t1, 3f
2:  lw      t2, UART_STATUS(t0)
    andi    t2, t2, UART_TX_FULL
    bnez    t2, 2b
    sw      t1, UART_DATA(t0)
    addi    a0, a0, 1
    j       1b
3:  ret

/* puthex: write a0 as eight hex digits. Clobbers t0-t4. */
puthex:
    li      t0, UART_BASE
    li      t3, 28
1:  srl     t1, a0, t3
    andi    t1, t1, 0xF
    la      t4, hex_digits
    add     t4, t4, t1
    lbu     t1, 0(t4)
2:  lw      t2, UART_STATUS(t0)
    andi    t2, t2, UART_TX_FULL
    bnez    t2, 2b
    sw      t1, UART_DATA(t0)
    addi    t3, t3, -4
    bgez    t3, 1b
    ret

/* uart_drain: wait until everything written has left the transmitter. */
uart_drain:
    li      t0, UART_BASE
1:  lw      t1, UART_STATUS(t0)
    andi    t1, t1, UART_TX_IDLE
    beqz    t1, 1b
    ret

/* halt: stop the machine with code a0. */
halt:
    mv      s0, a0
    jal     uart_drain
    li      t0, GPIO_BASE
    sw      s0, GPIO_LEDS(t0)
    li      t0, SYS_BASE
    sw      s0, SYS_HALT(t0)
1:  j       1b

/* Any trap during boot is fatal: report mcause, mepc and mtval. */
    .p2align 2
boot_trap:
    li      sp, SCRATCH_TOP
    la      a0, msg_trap
    jal     puts
    csrr    a0, mcause
    jal     puthex
    li      a0, ' '
    jal     putc_a0
    csrr    a0, mepc
    jal     puthex
    li      a0, ' '
    jal     putc_a0
    csrr    a0, mtval
    jal     puthex
    la      a0, msg_crlf
    jal     puts
    li      a0, 0x1F
    j       halt

putc_a0:
    li      t0, UART_BASE
1:  lw      t2, UART_STATUS(t0)
    andi    t2, t2, UART_TX_FULL
    bnez    t2, 1b
    sw      a0, UART_DATA(t0)
    ret

    .section .rodata
    .p2align 2
/* CRC-32 (IEEE, reflected, polynomial 0xEDB88320), computed by the assembler. */
crc_table:
    .set    crc_i, 0
    .rept   256
    .set    crc_c, crc_i
    .rept   8
    .set    crc_c, ((crc_c >> 1) ^ (0xEDB88320 & -(crc_c & 1))) & 0xFFFFFFFF
    .endr
    .word   crc_c
    .set    crc_i, crc_i + 1
    .endr

hex_digits:     .ascii "0123456789abcdef"
msg_banner:     .asciz "\r\nStrata boot ROM 1.0\r\n"
msg_flash:      .asciz "boot: loading from flash\r\n"
msg_uart:       .asciz "boot: waiting for an image on the UART\r\n"
msg_receiving:  .asciz "boot: receiving\r\n"
msg_entry:      .asciz "boot: starting at 0x"
msg_trap:       .asciz "boot: trap "
msg_crlf:       .asciz "\r\n"
err_magic:      .asciz "boot: no image\r\n"
err_version:    .asciz "boot: unsupported image version\r\n"
err_nseg:       .asciz "boot: bad segment count\r\n"
err_hcrc:       .asciz "boot: header CRC mismatch\r\n"
err_range:      .asciz "boot: segment outside RAM\r\n"
err_scrc:       .asciz "boot: segment CRC mismatch\r\n"
