/*
 * The program store: a read-only directory of user programs in the boot
 * flash (format in strata/layout.h, built by scripts/mkflash.py). Programs
 * are copied from flash into their slot each time they are started, so
 * every run begins from the program's original data.
 */
#include <string.h>

#include "kernel.h"

static struct store_entry dir[STORE_MAX];
static int count;

int store_init(void)
{
    uint32_t hdr[2];
    flash_read(STORE_OFFSET, hdr, sizeof hdr);
    if (hdr[0] != STORE_MAGIC || hdr[1] > STORE_MAX) {
        count = 0;
        return E_NOENT;
    }
    count = (int)hdr[1];
    flash_read(STORE_OFFSET + 8, dir, (uint32_t)count * sizeof dir[0]);
    return count;
}

int store_count(void)
{
    return count;
}

const struct store_entry *store_entry(int i)
{
    return (i >= 0 && i < count) ? &dir[i] : 0;
}

const struct store_entry *store_find(const char *name)
{
    for (int i = 0; i < count; i++)
        if (strncmp(dir[i].name, name, sizeof dir[i].name) == 0)
            return &dir[i];
    return 0;
}

int store_load(const struct store_entry *e)
{
    void *dst = (void *)e->load_addr;
    flash_read(STORE_OFFSET + e->offset, dst, e->size);
    if (crc32(0, dst, e->size) != e->crc32) {
        kprintf("[kernel] %s: CRC mismatch in program store\n", e->name);
        return E_INVAL;
    }
    return 0;
}
