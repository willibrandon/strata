/* Device drivers: UART, TIMER, GPIO, VIDEO, SPI flash, INTC. */
#include "kernel.h"

/* ---------------------------------------------------------------- UART */

void uart_init(void)
{
    /* Keep the reset bit period (1 Mbaud at 25 MHz); receive by interrupt. */
    mmio_write(UART_BASE, UART_STATUS, UART_RX_OVERRUN);
    mmio_write(UART_BASE, UART_CTRL, UART_RX_IE);
}

void uart_putc(char c)
{
    while (mmio_read(UART_BASE, UART_STATUS) & UART_TX_FULL)
        ;
    mmio_write(UART_BASE, UART_DATA, (uint8_t)c);
}

int uart_getc(void)
{
    uint32_t v = mmio_read(UART_BASE, UART_DATA);
    return (v & UART_RX_VALID) ? (int)(v & 0xFF) : -1;
}

void uart_drain(void)
{
    while (!(mmio_read(UART_BASE, UART_STATUS) & UART_TX_IDLE))
        ;
}

/* ---------------------------------------------------------------- TIMER */

#define TICKS_PER_MS (CLK_HZ / 1000)

void timer_init(void)
{
    timer_set(~0ull);
    mmio_write(TIMER_BASE, TIMER_MSIP, 0);
}

uint64_t timer_now(void)
{
    uint32_t hi, lo;
    do {
        hi = mmio_read(TIMER_BASE, TIMER_MTIME_HI);
        lo = mmio_read(TIMER_BASE, TIMER_MTIME_LO);
    } while (hi != mmio_read(TIMER_BASE, TIMER_MTIME_HI));
    return ((uint64_t)hi << 32) | lo;
}

uint64_t timer_ms(void)
{
    return timer_now() / TICKS_PER_MS;
}

/* Sets mtimecmp without a spurious interrupt from a half-written value. */
void timer_set(uint64_t when)
{
    mmio_write(TIMER_BASE, TIMER_MTIMECMP_HI, 0xFFFFFFFF);
    mmio_write(TIMER_BASE, TIMER_MTIMECMP_LO, (uint32_t)when);
    mmio_write(TIMER_BASE, TIMER_MTIMECMP_HI, (uint32_t)(when >> 32));
}

/* ---------------------------------------------------------------- GPIO */

#define DEBOUNCE_MS 20

static uint64_t last_press[7];

void gpio_init(void)
{
    mmio_write(GPIO_BASE, GPIO_EDGES, 0x7F);
    mmio_write(GPIO_BASE, GPIO_IRQ_EN, 0x7F);
}

uint32_t gpio_buttons(void)
{
    return mmio_read(GPIO_BASE, GPIO_BUTTONS);
}

void gpio_leds(uint32_t v)
{
    mmio_write(GPIO_BASE, GPIO_LEDS, v);
}

/* Press events, debounced in software: a second press of the same button
 * within DEBOUNCE_MS is contact bounce and is ignored. */
void gpio_irq(void)
{
    uint32_t edges = mmio_read(GPIO_BASE, GPIO_EDGES);
    mmio_write(GPIO_BASE, GPIO_EDGES, edges);
    uint64_t now = timer_ms();
    uint32_t accepted = 0;
    for (int i = 0; i < 7; i++) {
        if ((edges & (1u << i)) && (last_press[i] == 0 || now - last_press[i] >= DEBOUNCE_MS)) {
            accepted |= 1u << i;
            last_press[i] = now;
        }
    }
    if (accepted)
        input_buttons(accepted);
}

/* ---------------------------------------------------------------- VIDEO */

static const uint16_t default_palette[16] = {
    0x000, 0x00A, 0x0A0, 0x0AA, 0xA00, 0xA0A, 0xA50, 0xAAA,
    0x555, 0x55F, 0x5F5, 0x5FF, 0xF55, 0xF5F, 0xFF5, 0xFFF,
};

void video_init(void)
{
    /* Load the font into font RAM, word by word. */
    volatile uint32_t *font = (volatile uint32_t *)FONT_BASE;
    const uint32_t *src = (const uint32_t *)strata_font;
    for (unsigned i = 0; i < FONT_BYTES / 4; i++)
        font[i] = src[i];
    for (unsigned i = 0; i < 16; i++)
        video_set_palette(i, default_palette[i]);
    mmio_write(VIDEO_BASE, VIDEO_STATUS, VIDEO_VBLANK);
}

void video_enable(int on)
{
    mmio_write(VIDEO_BASE, VIDEO_CTRL, (on ? VIDEO_ENABLE : 0) | VIDEO_VBLANK_IE);
}

void video_set_palette(unsigned i, uint32_t rgb)
{
    mmio_write(VIDEO_BASE, VIDEO_PALETTE + 4 * (i & 15), rgb & 0xFFF);
}

uint32_t video_frame(void)
{
    return mmio_read(VIDEO_BASE, VIDEO_FRAME);
}

/* ---------------------------------------------------------------- SPI flash */

static void spi_select(int on)
{
    mmio_write(SPI_BASE, SPI_CTRL, (on ? SPI_CS : 0) | SPI_HALF(0));
}

static uint8_t spi_xfer(uint8_t b)
{
    mmio_write(SPI_BASE, SPI_DATA, b);
    while (mmio_read(SPI_BASE, SPI_STATUS) & SPI_BUSY)
        ;
    return (uint8_t)mmio_read(SPI_BASE, SPI_DATA);
}

void flash_init(void)
{
    spi_select(1);
    spi_xfer(0xAB);   /* release from power-down */
    spi_select(0);
}

uint32_t flash_jedec_id(void)
{
    spi_select(1);
    spi_xfer(0x9F);
    uint32_t id = (uint32_t)spi_xfer(0) << 16;
    id |= (uint32_t)spi_xfer(0) << 8;
    id |= spi_xfer(0);
    spi_select(0);
    return id;
}

int flash_read(uint32_t addr, void *dst, uint32_t len)
{
    uint8_t *d = dst;
    spi_select(1);
    spi_xfer(0x03);
    spi_xfer((uint8_t)(addr >> 16));
    spi_xfer((uint8_t)(addr >> 8));
    spi_xfer((uint8_t)addr);
    while (len--)
        *d++ = spi_xfer(0);
    spi_select(0);
    return 0;
}

/* ---------------------------------------------------------------- INTC */

void intc_init(void)
{
    mmio_write(INTC_BASE, INTC_ENABLE, IRQ_UART | IRQ_GPIO | IRQ_VIDEO);
}

/* ---------------------------------------------------------------- CRC-32 */

uint32_t crc32(uint32_t crc, const void *p, uint32_t len)
{
    const uint8_t *b = p;
    crc = ~crc;
    while (len--) {
        crc ^= *b++;
        for (int k = 0; k < 8; k++)
            crc = (crc >> 1) ^ (0xEDB88320u & -(crc & 1));
    }
    return ~crc;
}
