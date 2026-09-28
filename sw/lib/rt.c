/*
 * Integer arithmetic helpers that clang calls on RV32I, which has no multiply
 * or divide instructions. Written with shifts, adds and compares only, so
 * that the compiler cannot turn them back into calls to themselves. Division
 * by zero follows the RISC-V M-extension convention (quotient all ones,
 * remainder = dividend) so results are well defined.
 */
#include <stdint.h>

uint32_t __mulsi3(uint32_t a, uint32_t b);
uint64_t __muldi3(uint64_t a, uint64_t b);
uint32_t __udivsi3(uint32_t n, uint32_t d);
uint32_t __umodsi3(uint32_t n, uint32_t d);
int32_t  __divsi3(int32_t n, int32_t d);
int32_t  __modsi3(int32_t n, int32_t d);
uint64_t __udivdi3(uint64_t n, uint64_t d);
uint64_t __umoddi3(uint64_t n, uint64_t d);
int64_t  __divdi3(int64_t n, int64_t d);
int64_t  __moddi3(int64_t n, int64_t d);

uint32_t __mulsi3(uint32_t a, uint32_t b)
{
    uint32_t r = 0;
    while (b) {
        if (b & 1)
            r += a;
        a <<= 1;
        b >>= 1;
    }
    return r;
}

uint64_t __muldi3(uint64_t a, uint64_t b)
{
    uint64_t r = 0;
    while (b) {
        if (b & 1)
            r += a;
        a <<= 1;
        b >>= 1;
    }
    return r;
}

static uint32_t udivmod32(uint32_t n, uint32_t d, uint32_t *rem)
{
    if (d == 0) {
        *rem = n;
        return 0xFFFFFFFFu;
    }
    uint32_t q = 0, r = 0;
    for (int i = 31; i >= 0; i--) {
        r = (r << 1) | ((n >> i) & 1);
        if (r >= d) {
            r -= d;
            q |= 1u << i;
        }
    }
    *rem = r;
    return q;
}

static uint64_t udivmod64(uint64_t n, uint64_t d, uint64_t *rem)
{
    if (d == 0) {
        *rem = n;
        return ~0ull;
    }
    if ((n >> 32) == 0 && (d >> 32) == 0) {
        uint32_t r32;
        uint32_t q32 = udivmod32((uint32_t)n, (uint32_t)d, &r32);
        *rem = r32;
        return q32;
    }
    uint64_t q = 0, r = 0;
    for (int i = 63; i >= 0; i--) {
        r = (r << 1) | ((n >> i) & 1);
        if (r >= d) {
            r -= d;
            q |= 1ull << i;
        }
    }
    *rem = r;
    return q;
}

uint32_t __udivsi3(uint32_t n, uint32_t d)
{
    uint32_t r;
    return udivmod32(n, d, &r);
}

uint32_t __umodsi3(uint32_t n, uint32_t d)
{
    uint32_t r;
    udivmod32(n, d, &r);
    return r;
}

int32_t __divsi3(int32_t n, int32_t d)
{
    if (d == 0)
        return -1;
    int neg = (n < 0) != (d < 0);
    uint32_t r, q = udivmod32(n < 0 ? -(uint32_t)n : (uint32_t)n, d < 0 ? -(uint32_t)d : (uint32_t)d, &r);
    return neg ? -(int32_t)q : (int32_t)q;
}

int32_t __modsi3(int32_t n, int32_t d)
{
    if (d == 0)
        return n;
    uint32_t r;
    udivmod32(n < 0 ? -(uint32_t)n : (uint32_t)n, d < 0 ? -(uint32_t)d : (uint32_t)d, &r);
    return n < 0 ? -(int32_t)r : (int32_t)r;
}

uint64_t __udivdi3(uint64_t n, uint64_t d)
{
    uint64_t r;
    return udivmod64(n, d, &r);
}

uint64_t __umoddi3(uint64_t n, uint64_t d)
{
    uint64_t r;
    udivmod64(n, d, &r);
    return r;
}

int64_t __divdi3(int64_t n, int64_t d)
{
    if (d == 0)
        return -1;
    int neg = (n < 0) != (d < 0);
    uint64_t r, q = udivmod64(n < 0 ? -(uint64_t)n : (uint64_t)n, d < 0 ? -(uint64_t)d : (uint64_t)d, &r);
    return neg ? -(int64_t)q : (int64_t)q;
}

int64_t __moddi3(int64_t n, int64_t d)
{
    if (d == 0)
        return n;
    uint64_t r;
    udivmod64(n < 0 ? -(uint64_t)n : (uint64_t)n, d < 0 ? -(uint64_t)d : (uint64_t)d, &r);
    return n < 0 ? -(int64_t)r : (int64_t)r;
}
