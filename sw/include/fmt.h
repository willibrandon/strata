/* printf-style formatting: %d %i %u %x %X %o %c %s %p %%, with flags '-' '0',
 * a field width, and the 'l'/'ll' length modifiers. */
#ifndef STRATA_FMT_H
#define STRATA_FMT_H
#include <stdarg.h>
#include <stddef.h>

int vsnprintf(char *buf, size_t size, const char *fmt, va_list ap);
int snprintf(char *buf, size_t size, const char *fmt, ...) __attribute__((format(printf, 3, 4)));

/* Formats through a character sink; returns the number of characters. */
int vformat(void (*put)(char c, void *arg), void *arg, const char *fmt, va_list ap);
#endif
