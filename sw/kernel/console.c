/*
 * The kernel console: an 80x30 text screen in VRAM, mirrored to the UART.
 * Understands \n, \r, \b, \t, form feed (clear) and the ANSI sequences
 * ESC[<n>m for colors (30-37, 90-97 foreground; 40-47 background; 0 reset),
 * ESC[2J (clear) and ESC[H (home). The same bytes go to the UART, so a host
 * terminal shows the same colors.
 *
 * When a task owns the screen (SYS_SCREEN), the console keeps its contents
 * in a shadow buffer and redraws the screen when the task releases it.
 */
#include <fmt.h>
#include <stdarg.h>
#include <string.h>

#include "kernel.h"

#define DEFAULT_ATTR 0x07   /* light grey on black */

static uint16_t shadow[TEXT_ROWS][TEXT_COLS];
static int row, col;
static uint8_t attr = DEFAULT_ATTR;
static int visible;
static int esc, esc_num, esc_args[4], esc_n;

static void cell_put(int r, int c, uint16_t v)
{
    shadow[r][c] = v;
    if (visible)
        *text_cell(r, c) = v;
}

static void cursor_show(int on)
{
    uint16_t v = shadow[row][col];
    if (visible)
        *text_cell(row, col) = on ? (uint16_t)((v & 0xFF) | ((v >> 4) & 0x0F00) | ((v << 4) & 0xF000)) : v;
}

void console_redraw(void)
{
    if (!visible)
        return;
    for (int r = 0; r < TEXT_ROWS; r++)
        for (int c = 0; c < TEXT_COLS; c++)
            *text_cell(r, c) = shadow[r][c];
    cursor_show(1);
}

void console_set_visible(int on)
{
    visible = on;
    console_redraw();
}

static void clear(void)
{
    for (int r = 0; r < TEXT_ROWS; r++)
        for (int c = 0; c < TEXT_COLS; c++)
            cell_put(r, c, (uint16_t)(attr << 8 | ' '));
    row = col = 0;
}

static void scroll(void)
{
    memmove(shadow[0], shadow[1], sizeof shadow - sizeof shadow[0]);
    for (int c = 0; c < TEXT_COLS; c++)
        shadow[TEXT_ROWS - 1][c] = (uint16_t)(attr << 8 | ' ');
    row = TEXT_ROWS - 1;
    console_redraw();
}

static void newline(void)
{
    col = 0;
    if (++row == TEXT_ROWS)
        scroll();
}

static void sgr(int n)
{
    static const uint8_t ansi_to_vga[8] = {0, 4, 2, 6, 1, 5, 3, 7};
    if (n == 0)
        attr = DEFAULT_ATTR;
    else if (n >= 30 && n <= 37)
        attr = (uint8_t)((attr & 0xF0) | ansi_to_vga[n - 30]);
    else if (n >= 90 && n <= 97)
        attr = (uint8_t)((attr & 0xF0) | (ansi_to_vga[n - 90] + 8));
    else if (n >= 40 && n <= 47)
        attr = (uint8_t)((attr & 0x0F) | (ansi_to_vga[n - 40] << 4));
}

static void screen_putc(char ch)
{
    uint8_t c = (uint8_t)ch;
    if (esc == 1) {
        esc = (c == '[') ? 2 : 0;
        esc_num = esc_n = 0;
        return;
    }
    if (esc == 2) {
        if (c >= '0' && c <= '9') {
            esc_num = esc_num * 10 + (c - '0');
            return;
        }
        if (esc_n < 4)
            esc_args[esc_n++] = esc_num;
        esc_num = 0;
        if (c == ';')
            return;
        esc = 0;
        if (c == 'm') {
            for (int i = 0; i < esc_n; i++)
                sgr(esc_args[i]);
        } else if (c == 'J') {
            clear();
        } else if (c == 'H') {
            row = col = 0;
        }
        return;
    }
    switch (c) {
    case 0x1B:
        esc = 1;
        return;
    case '\n':
        newline();
        return;
    case '\r':
        col = 0;
        return;
    case '\b':
        if (col > 0)
            col--;
        return;
    case '\t':
        do
            screen_putc(' ');
        while (col % 8);
        return;
    case '\f':
        clear();
        return;
    default:
        cell_put(row, col, (uint16_t)(attr << 8 | c));
        if (++col == TEXT_COLS)
            newline();
    }
}

void console_putc(char c)
{
    cursor_show(0);
    screen_putc(c);
    cursor_show(1);
    if (c == '\n')
        uart_putc('\r');
    uart_putc(c);
}

void console_write(const char *s, size_t n)
{
    cursor_show(0);
    for (size_t i = 0; i < n; i++) {
        screen_putc(s[i]);
        if (s[i] == '\n')
            uart_putc('\r');
        uart_putc(s[i]);
    }
    cursor_show(1);
}

void console_init(void)
{
    visible = 1;
    clear();
}

static void kput(char c, void *arg)
{
    (void)arg;
    console_putc(c);
}

void kprintf(const char *fmt, ...)
{
    va_list ap;
    va_start(ap, fmt);
    vformat(kput, 0, fmt, ap);
    va_end(ap);
}
