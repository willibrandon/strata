#include <fmt.h>
#include <stdint.h>

static int emit_num(void (*put)(char, void *), void *arg, unsigned long long v, unsigned base,
                    int upper, int neg, int width, int zero, int left)
{
    char tmp[24];
    int n = 0, count = 0;
    const char *digits = upper ? "0123456789ABCDEF" : "0123456789abcdef";
    /* 32-bit division is much cheaper on RV32I; use it whenever possible. */
    while (v >> 32) {
        tmp[n++] = digits[v % base];
        v /= base;
    }
    uint32_t v32 = (uint32_t)v;
    do {
        tmp[n++] = digits[v32 % base];
        v32 /= base;
    } while (v32);
    int len = n + neg;
    if (neg && zero) {
        put('-', arg);
        count++;
    }
    if (!left) {
        for (; width > len; width--, count++)
            put(zero ? '0' : ' ', arg);
    }
    if (neg && !zero) {
        put('-', arg);
        count++;
    }
    while (n) {
        put(tmp[--n], arg);
        count++;
    }
    if (left) {
        for (; width > len; width--, count++)
            put(' ', arg);
    }
    return count;
}

__attribute__((noinline)) int vformat(void (*put)(char c, void *arg), void *arg, const char *fmt, va_list ap)
{
    int count = 0;
    for (; *fmt; fmt++) {
        if (*fmt != '%') {
            put(*fmt, arg);
            count++;
            continue;
        }
        fmt++;
        int zero = 0, left = 0, width = 0, lng = 0;
        for (;; fmt++) {
            if (*fmt == '0')
                zero = 1;
            else if (*fmt == '-')
                left = 1;
            else
                break;
        }
        while (*fmt >= '0' && *fmt <= '9')
            width = width * 10 + (*fmt++ - '0');
        while (*fmt == 'l') {
            lng++;
            fmt++;
        }
        if (left)
            zero = 0;
        switch (*fmt) {
        case 'd':
        case 'i': {
            long long v = lng >= 2 ? va_arg(ap, long long) : va_arg(ap, int);
            int neg = v < 0;
            count += emit_num(put, arg, neg ? -(unsigned long long)v : (unsigned long long)v, 10, 0, neg,
                              width, zero, left);
            break;
        }
        case 'u':
        case 'x':
        case 'X':
        case 'o': {
            unsigned long long v = lng >= 2 ? va_arg(ap, unsigned long long) : va_arg(ap, unsigned);
            unsigned base = *fmt == 'u' ? 10 : *fmt == 'o' ? 8 : 16;
            count += emit_num(put, arg, v, base, *fmt == 'X', 0, width, zero, left);
            break;
        }
        case 'p':
            put('0', arg);
            put('x', arg);
            count += 2 + emit_num(put, arg, (uintptr_t)va_arg(ap, void *), 16, 0, 0, 8, 1, 0);
            break;
        case 'c': {
            char c = (char)va_arg(ap, int);
            if (!left)
                for (; width > 1; width--, count++)
                    put(' ', arg);
            put(c, arg);
            count++;
            for (; width > 1; width--, count++)
                put(' ', arg);
            break;
        }
        case 's': {
            const char *s = va_arg(ap, const char *);
            if (!s)
                s = "(null)";
            int len = 0;
            while (s[len])
                len++;
            if (!left)
                for (; width > len; width--, count++)
                    put(' ', arg);
            for (int i = 0; i < len; i++)
                put(s[i], arg);
            count += len;
            for (; width > len; width--, count++)
                put(' ', arg);
            break;
        }
        case '%':
            put('%', arg);
            count++;
            break;
        case 0:
            return count;
        default:
            put('%', arg);
            put(*fmt, arg);
            count += 2;
            break;
        }
    }
    return count;
}

struct bufsink {
    char *buf;
    size_t size, pos;
};

static void buf_put(char c, void *arg)
{
    struct bufsink *b = arg;
    if (b->pos + 1 < b->size)
        b->buf[b->pos] = c;
    b->pos++;
}

int vsnprintf(char *buf, size_t size, const char *fmt, va_list ap)
{
    struct bufsink b = {buf, size, 0};
    int n = vformat(buf_put, &b, fmt, ap);
    if (size)
        buf[b.pos < size ? b.pos : size - 1] = 0;
    return n;
}

int snprintf(char *buf, size_t size, const char *fmt, ...)
{
    va_list ap;
    va_start(ap, fmt);
    int n = vsnprintf(buf, size, fmt, ap);
    va_end(ap);
    return n;
}
