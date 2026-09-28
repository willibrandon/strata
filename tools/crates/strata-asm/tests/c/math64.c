/* Differential-test program: 64-bit arithmetic, CRC-32, large constants,
 * initialized 64-bit data and pointers into other objects. */
#include <stdint.h>

uint64_t big_table[6] = {0, 1, 0x123456789abcdef0ull, ~0ull, 0x8000000000000000ull, 1000000007ull};
int64_t signed_table[4] = {-1, -9223372036854775807ll - 1, 42, -123456789012ll};
uint32_t *const big_low_word = (uint32_t *)&big_table[2];
extern const char *const color_names[];
const char *const *palette_ref = &color_names[3];

uint32_t crc32_update(uint32_t crc, const uint8_t *p, unsigned n) {
    crc = ~crc;
    while (n--) {
        crc ^= *p++;
        for (int k = 0; k < 8; k++)
            crc = (crc >> 1) ^ (0xEDB88320u & -(crc & 1));
    }
    return ~crc;
}

uint64_t mix64(uint64_t a, uint64_t b) {
    uint64_t x = a * 0x9E3779B97F4A7C15ull + b;
    x ^= x >> 31;
    x *= 0xBF58476D1CE4E5B9ull;
    return x ^ (x << 17) ^ (b >> 3);
}

int64_t sdiv64(int64_t a, int64_t b) { return b ? a / b : 0; }
uint64_t urem64(uint64_t a, uint64_t b) { return b ? a % b : a; }

uint32_t isqrt(uint32_t v) {
    uint32_t r = 0, bit = 1u << 30;
    while (bit > v)
        bit >>= 2;
    while (bit) {
        if (v >= r + bit) {
            v -= r + bit;
            r = (r >> 1) + bit;
        } else {
            r >>= 1;
        }
        bit >>= 2;
    }
    return r;
}

static const uint8_t message[] = "The quick brown fox jumps over the lazy dog";

uint32_t math_main(void) {
    uint64_t acc = 0;
    for (int i = 0; i < 6; i++)
        acc = mix64(acc, big_table[i]) + (uint64_t)signed_table[i & 3];
    acc += (uint64_t)sdiv64(signed_table[3], 7) + urem64(acc, 1000003);
    uint32_t crc = crc32_update(0, message, sizeof message - 1);
    return crc ^ (uint32_t)acc ^ (uint32_t)(acc >> 32) ^ isqrt(crc) ^ *big_low_word ^ (uint32_t)(uintptr_t)*palette_ref;
}
