/*
 * Keyboard and button input. Bytes from the terminal and button presses are
 * turned into key codes (ASCII or KEY_*) and queued. ANSI cursor-key
 * sequences (ESC [ A..D) become KEY_UP..KEY_LEFT. Ctrl-C is not queued: it
 * is recorded and handled by task_interrupt() at the end of the trap.
 */
#include "kernel.h"

#define QSIZE 64

static uint16_t queue[QSIZE];
static unsigned head, tail;
static int esc_state;   /* 0 idle, 1 after ESC, 2 after ESC [ */
static int interrupt_pending;

static void put(int key)
{
    unsigned next = (head + 1) % QSIZE;
    if (next != tail) {
        queue[head] = (uint16_t)key;
        head = next;
    }
}

void input_uart_byte(int c)
{
    switch (esc_state) {
    case 1:
        esc_state = (c == '[') ? 2 : 0;
        if (!esc_state) {
            put(0x1B);
            put(c);
        }
        return;
    case 2:
        esc_state = 0;
        switch (c) {
        case 'A': put(KEY_UP); return;
        case 'B': put(KEY_DOWN); return;
        case 'C': put(KEY_RIGHT); return;
        case 'D': put(KEY_LEFT); return;
        default: return;
        }
    default:
        if (c == 0x1B)
            esc_state = 1;
        else if (c == 0x03)
            interrupt_pending = 1;
        else
            put(c == '\r' ? '\n' : c);
    }
}

void input_buttons(uint32_t pressed)
{
    static const int keys[7] = {KEY_UP, KEY_DOWN, KEY_LEFT, KEY_RIGHT, KEY_A, KEY_B, KEY_START};
    for (int i = 0; i < 7; i++)
        if (pressed & (1u << i))
            put(keys[i]);
}

/* Moves any bytes waiting in the UART into the queue. */
void input_poll(void)
{
    int c;
    while ((c = uart_getc()) >= 0)
        input_uart_byte(c);
}

void input_put(int key)
{
    put(key);
}

/* Whether Ctrl-C arrived since the last call. */
int input_take_interrupt(void)
{
    int p = interrupt_pending;
    interrupt_pending = 0;
    return p;
}

int input_available(void)
{
    return head != tail;
}

int input_get(void)
{
    if (head == tail)
        return -1;
    int k = queue[tail];
    tail = (tail + 1) % QSIZE;
    return k;
}
