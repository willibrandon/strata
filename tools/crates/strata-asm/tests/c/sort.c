/* Differential-test program: structs, recursion, function pointers,
 * string tables, jump-table switches and bit-fields. */
#include <stddef.h>
#include <stdint.h>

struct record {
    const char *name;
    int32_t key;
    uint16_t flags : 5;
    uint16_t level : 11;
    uint8_t tag;
};

typedef int (*cmp_fn)(const struct record *, const struct record *);

static struct record table[] = {
    {"delta", 40, 3, 100, 'd'},   {"alpha", -7, 1, 2047, 'a'}, {"echo", 1000, 31, 0, 'e'},
    {"charlie", 12, 0, 5, 'c'},   {"bravo", -7, 2, 17, 'b'},   {"foxtrot", 0, 7, 1, 'f'},
    {"golf", 123456, 9, 99, 'g'}, {"hotel", -42, 4, 3, 'h'},
};

const char *const color_names[] = {"red", "green", "blue", "cyan", "magenta", "yellow", "white", "black"};

extern void uart_putc(char c);
extern void trace(int event, int value);

static int by_key(const struct record *a, const struct record *b) {
    return a->key < b->key ? -1 : a->key > b->key;
}

static int by_name(const struct record *a, const struct record *b) {
    const char *p = a->name, *q = b->name;
    while (*p && *p == *q) {
        p++;
        q++;
    }
    return (unsigned char)*p - (unsigned char)*q;
}

static int by_level(const struct record *a, const struct record *b) {
    return (int)a->level - (int)b->level;
}

cmp_fn comparators[] = {by_key, by_name, by_level};

static void swap(struct record *a, struct record *b) {
    struct record t = *a;
    *a = *b;
    *b = t;
}

void quicksort(struct record *v, int lo, int hi, cmp_fn cmp) {
    if (lo >= hi)
        return;
    struct record *pivot = &v[(lo + hi) / 2];
    struct record p = *pivot;
    int i = lo, j = hi;
    while (i <= j) {
        while (cmp(&v[i], &p) < 0)
            i++;
        while (cmp(&v[j], &p) > 0)
            j--;
        if (i <= j) {
            swap(&v[i], &v[j]);
            i++;
            j--;
        }
    }
    quicksort(v, lo, j, cmp);
    quicksort(v, i, hi, cmp);
}

void puts_(const char *s) {
    while (*s)
        uart_putc(*s++);
}

/* Dense switch with distinct side effects: compiled to a jump table. */
int dispatch(int op, int a, int b) {
    switch (op) {
    case 0: trace(0, a); return a + b;
    case 1: trace(1, b); return a - b;
    case 2: puts_("two"); return a * 3;
    case 3: trace(3, a ^ b); return a & b;
    case 4: puts_(color_names[a & 7]); return a | b;
    case 5: trace(5, a << 2); return a >> 1;
    case 6: puts_("six"); return -a;
    case 7: trace(7, b); return b ? a / b : 0;
    case 8: trace(8, a); return (unsigned)a % 10u;
    default: puts_("bad op"); return -1;
    }
}

int ackermann(int m, int n) {
    if (m == 0)
        return n + 1;
    if (n == 0)
        return ackermann(m - 1, 1);
    return ackermann(m - 1, ackermann(m, n - 1));
}

int sort_main(void) {
    int total = 0;
    for (size_t c = 0; c < sizeof comparators / sizeof comparators[0]; c++) {
        quicksort(table, 0, (int)(sizeof table / sizeof table[0]) - 1, comparators[c]);
        for (size_t i = 0; i < sizeof table / sizeof table[0]; i++) {
            puts_(table[i].name);
            total += table[i].key * (int)(i + 1) + table[i].flags + table[i].tag;
        }
    }
    for (int op = -1; op < 10; op++)
        total += dispatch(op, total, op);
    return total + ackermann(2, 3);
}
