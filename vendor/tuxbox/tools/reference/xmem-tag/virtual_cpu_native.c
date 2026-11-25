/* Validate the console XMemVirtualAlloc ordinary CPU flags without initializing
 * the console runtime. The launcher verifies the supplied PE hash; the NT
 * allocation entry point is replaced with a controlled single-thread stub. */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>

typedef void *(__attribute__((ms_abi)) *alloc_fn)(void *, size_t, uint32_t, uint64_t, uint32_t);
static uint32_t flags_seen, protection_seen;
static uint64_t bytes_seen;
static unsigned calls;

static __attribute__((ms_abi)) void *fill(void *p, int value, size_t bytes)
{
    return memset(p, value, bytes);
}
static __attribute__((ms_abi)) int nt_alloc(void *process, void **base, size_t *size,
                                           uint32_t flags, uint32_t protection,
                                           void *params, uint32_t count)
{
    (void)process; (void)params; (void)count;
    ++calls; flags_seen = flags; protection_seen = protection; bytes_seen = *size;
    *base = (void *)0x12340000;
    return 0;
}
int main(int argc, char **argv)
{
    if (argc != 2) return 2;
    FILE *file = fopen(argv[1], "rb");
    if (!file || fseek(file, 0, SEEK_END)) return 3;
    long length = ftell(file);
    if (length < 0x10000 || length > 0x100000 || fseek(file, 0, SEEK_SET)) return 4;
    unsigned char *image = mmap(NULL, length, PROT_READ | PROT_WRITE | PROT_EXEC,
                                MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (image == MAP_FAILED || fread(image, 1, length, file) != (size_t)length) return 5;
    fclose(file);
    *(void **)(image + 0x93e0) = (void *)nt_alloc;
    *(void **)(image + 0x9368) = (void *)fill;
    /* Force the standard NT allocation mode and disable console-only feature
       detection, which cannot be queried in this isolated process. */
    *(uint32_t *)(image + 0xe0d8) = 0;
    alloc_fn allocate = (alloc_fn)(image + 0x2140);
    const uint32_t flags[] = {0x1000, 0x2000, 0x3000};
    const uint32_t protection[] = {1, 2, 4, 0x10, 0x20, 0x40, 0x102, 0x104, 0x110, 0x120, 0x140};
    unsigned expected = 0;
    for (unsigned i = 0; i < 3; ++i) for (unsigned j = 0; j < 11; ++j) for (unsigned k = 0; k < 2; ++k) {
        uint32_t f = flags[i] | (k ? 0x100000 : 0);
        void *result = allocate(NULL, 16384, f, 1, protection[j]);
        ++expected;
        if (result != (void *)0x12340000 || calls != expected || flags_seen != f ||
            protection_seen != protection[j] || bytes_seen != 16384) return 6;
    }
    puts("xmem-native-virtual-cpu PASS profiles=66 flags=preserved");
    return 0;
}
