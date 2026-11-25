/* Execute only fingerprinted XMemAlloc, XMemFree and XMemSetAllocationHooks
 * against controlled callbacks. The launcher constructs a temporary PE image,
 * validates the source DLL hash, and does not copy vendor bytes to the repo. */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/mman.h>

typedef void (__attribute__((ms_abi)) *set_fn)(void *, void *);
typedef void *(__attribute__((ms_abi)) *alloc_fn)(size_t, uint64_t);
typedef void (__attribute__((ms_abi)) *free_fn)(void *, uint64_t);
static __attribute__((ms_abi)) long lock_noop(void *p) { (void)p; return 0; }
__attribute__((naked)) static void guard_dispatch(void) { __asm__("jmp *%rax"); }
static unsigned allocations, frees;
static size_t bytes_seen;
static uint64_t alloc_attributes, free_attributes;
static void *memory_seen;
static char storage[16];
static set_fn native_set;
static __attribute__((ms_abi)) void *alloc_hook(size_t bytes, uint64_t attributes)
{
    ++allocations; bytes_seen = bytes; alloc_attributes = attributes; return storage;
}
static __attribute__((ms_abi)) void free_hook(void *memory, uint64_t attributes)
{
    ++frees; memory_seen = memory; free_attributes = attributes;
}
static __attribute__((ms_abi)) void *reentrant_hook(size_t bytes, uint64_t attributes)
{
    (void)bytes; (void)attributes;
    native_set(NULL, NULL);
    return storage;
}
int main(int argc, char **argv)
{
    if (argc != 2) return 2;
    FILE *file = fopen(argv[1], "rb");
    if (!file || fseek(file, 0, SEEK_END)) return 3;
    long length = ftell(file);
    if (length < 0x10000 || length > 0x100000 || fseek(file, 0, SEEK_SET)) return 4;
    unsigned char *image = mmap(NULL, length, PROT_READ|PROT_WRITE|PROT_EXEC,
                                MAP_PRIVATE|MAP_ANONYMOUS, -1, 0);
    if (image == MAP_FAILED || fread(image, 1, length, file) != (size_t)length) return 5;
    fclose(file);
    /* Replace only synchronization and CFG dispatch to use local callbacks.
       The callbacks and all tested calls run in this single native thread. */
    *(void **)(image + 0x9400) = (void *)lock_noop;
    *(void **)(image + 0x93d0) = (void *)lock_noop;
    *(void **)(image + 0x94a8) = (void *)guard_dispatch;
    set_fn set = (set_fn)(image + 0x3d90);
    native_set = set;
    alloc_fn allocate = (alloc_fn)(image + 0x3c00);
    free_fn release = (free_fn)(image + 0x3ce0);
    set((void *)alloc_hook, (void *)free_hook);
    if (allocate(0, 0x1234567887654321ULL) != storage || allocations != 1 || bytes_seen != 0 ||
        alloc_attributes != 0x1234567887654321ULL) return 6;
    release(NULL, 0x1020304050607080ULL);
    if (frees != 1 || memory_seen || free_attributes != 0x1020304050607080ULL) return 7;
    if (allocate(17, 0x9988776655443322ULL) != storage || allocations != 2 || bytes_seen != 17 ||
        alloc_attributes != 0x9988776655443322ULL) return 8;
    release(storage, 0x8877665544332211ULL);
    if (frees != 2 || memory_seen != storage || free_attributes != 0x8877665544332211ULL) return 9;
    set(NULL, (void *)free_hook);
    if (*(void **)(image + 0xe870) != image + 0x5870 || *(void **)(image + 0xe878) != image + 0x5ac0) return 10;
    set((void *)alloc_hook, NULL);
    if (*(void **)(image + 0xe870) != (void *)alloc_hook || *(void **)(image + 0xe878) != NULL) return 11;
    set((void *)reentrant_hook, (void *)free_hook);
    if (allocate(1, 0) != storage || *(void **)(image + 0xe870) != image + 0x5870 ||
        *(void **)(image + 0xe878) != image + 0x5ac0) return 12;
    puts("xmem-native-hooks PASS calls=2/2 zero-size=1 null-free=1 restore=1 partial-pair=1 reentrant=1");
    return 0;
}
