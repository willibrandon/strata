/* printf and friends for user programs, buffered into SYS_WRITE calls. */
#include <fmt.h>
#include <stdarg.h>
#include <string.h>
#include <strata/user.h>

struct outbuf {
    char buf[64];
    size_t n;
};

static void flush(struct outbuf *o)
{
    if (o->n)
        sys_write(o->buf, o->n);
    o->n = 0;
}

static void put(char c, void *arg)
{
    struct outbuf *o = arg;
    o->buf[o->n++] = c;
    if (o->n == sizeof o->buf)
        flush(o);
}

int printf(const char *fmt, ...)
{
    struct outbuf o = {.n = 0};
    va_list ap;
    va_start(ap, fmt);
    int n = vformat(put, &o, fmt, ap);
    va_end(ap);
    flush(&o);
    return n;
}

void putchar_(int c)
{
    char ch = (char)c;
    sys_write(&ch, 1);
}

void puts_(const char *s)
{
    sys_write(s, strlen(s));
}
