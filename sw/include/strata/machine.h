/*
 * The Strata machine interface: a direct transcription of docs/memory-map.md.
 * Guest software uses nothing else to talk to the hardware.
 */
#ifndef STRATA_MACHINE_H
#define STRATA_MACHINE_H

#define ROM_BASE        0x00000000
#define ROM_BYTES       4096
#define RAM_BASE        0x10000000
#define RAM_BYTES       65536
#define VRAM_BASE       0x20000000
#define VRAM_BYTES      8192
#define FONT_BASE       0x20010000
#define FONT_BYTES      4096
#define MMIO_BASE       0x40000000

#define CLK_HZ          25000000

/* Text display geometry. Cell (r, c) is the halfword at VRAM_BASE + r*256 + c*2:
 * bits [7:0] character, [11:8] foreground color, [15:12] background color. */
#define TEXT_COLS       80
#define TEXT_ROWS       30
#define TEXT_PITCH      256
#define GLYPH_BYTES     16

/* SYS */
#define SYS_BASE        0x40000000
#define SYS_ID          0x00
#define SYS_VERSION     0x04
#define SYS_CLK_HZ      0x08
#define SYS_RAM_BYTES   0x0C
#define SYS_SCRATCH     0x10
#define SYS_HALT        0x20
#define SYS_RESET       0x24
#define SYS_ID_VALUE    0x41525453   /* "STRA" */
#define SYS_RESET_MAGIC 0x52455345

/* UART */
#define UART_BASE       0x40001000
#define UART_DATA       0x00
#define UART_STATUS     0x04
#define UART_CTRL       0x08
#define UART_DIV        0x0C
#define UART_RX_VALID   (1 << 8)
#define UART_TX_FULL    (1 << 0)
#define UART_TX_IDLE    (1 << 1)
#define UART_RX_AVAIL   (1 << 2)
#define UART_RX_OVERRUN (1 << 3)
#define UART_RX_IE      (1 << 0)
#define UART_TX_IE      (1 << 1)

/* TIMER */
#define TIMER_BASE        0x40002000
#define TIMER_MTIME_LO    0x00
#define TIMER_MTIME_HI    0x04
#define TIMER_MTIMECMP_LO 0x08
#define TIMER_MTIMECMP_HI 0x0C
#define TIMER_MSIP        0x10

/* GPIO */
#define GPIO_BASE       0x40003000
#define GPIO_LEDS       0x00
#define GPIO_BUTTONS    0x04
#define GPIO_EDGES      0x08
#define GPIO_IRQ_EN     0x0C
#define BTN_UP          (1 << 0)
#define BTN_DOWN        (1 << 1)
#define BTN_LEFT        (1 << 2)
#define BTN_RIGHT       (1 << 3)
#define BTN_A           (1 << 4)
#define BTN_B           (1 << 5)
#define BTN_START       (1 << 6)

/* SPI */
#define SPI_BASE        0x40004000
#define SPI_DATA        0x00
#define SPI_STATUS      0x04
#define SPI_CTRL        0x08
#define SPI_EVENTS      0x0C
#define SPI_BUSY        (1 << 0)
#define SPI_CS          (1 << 0)
#define SPI_HALF(n)     ((n) << 8)
#define SPI_DONE_IE     (1 << 16)

/* VIDEO */
#define VIDEO_BASE      0x40005000
#define VIDEO_CTRL      0x00
#define VIDEO_STATUS    0x04
#define VIDEO_FRAME     0x08
#define VIDEO_PALETTE   0x40
#define VIDEO_ENABLE    (1 << 0)
#define VIDEO_VBLANK_IE (1 << 1)
#define VIDEO_IN_VBLANK (1 << 0)
#define VIDEO_VBLANK    (1 << 1)

/* INTC */
#define INTC_BASE       0x40006000
#define INTC_PENDING    0x00
#define INTC_ENABLE     0x04
#define IRQ_UART        (1 << 0)
#define IRQ_GPIO        (1 << 1)
#define IRQ_VIDEO       (1 << 2)
#define IRQ_SPI         (1 << 3)

/* Boot flash */
#define FLASH_IMAGE_OFFSET 0x00200000
#define BOOT_MAGIC         0x41525453   /* "STRA" */

#ifndef __ASSEMBLER__
#include <stdint.h>

/* MMIO registers must be accessed with aligned 32-bit loads and stores. */
static inline uint32_t mmio_read(uint32_t base, uint32_t off)
{
    return *(volatile uint32_t *)(base + off);
}

static inline void mmio_write(uint32_t base, uint32_t off, uint32_t v)
{
    *(volatile uint32_t *)(base + off) = v;
}

static inline volatile uint16_t *text_cell(unsigned row, unsigned col)
{
    return (volatile uint16_t *)(VRAM_BASE + row * TEXT_PITCH + col * 2);
}
#endif

#endif
