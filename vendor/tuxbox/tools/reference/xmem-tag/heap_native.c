/* Exercise the hash-pinned XMemAllocDefault/FreeDefault heap dispatch. The
 * launcher builds a temporary PE image and substitutes host heap imports. */
#include <stdint.h>
#include <stdio.h>
#include <sys/mman.h>

typedef void *(__attribute__((ms_abi)) *alloc_fn)(size_t, uint64_t);
typedef void (__attribute__((ms_abi)) *free_fn)(void *, uint64_t);
static char heap_sentinel[8], storage[32];
static unsigned gets, allocations, frees, null_frees;
static __attribute__((ms_abi)) void *partition_heap(int partition)
{
    if (partition != 1) return NULL;
    ++gets;
    return heap_sentinel;
}
static __attribute__((ms_abi)) void *heap_alloc(void *heap, uint32_t flags, size_t size)
{
    if (heap != heap_sentinel || flags || size != 17) return NULL;
    ++allocations;
    return storage;
}
static __attribute__((ms_abi)) int heap_free(void *heap, uint32_t flags, void *memory)
{
    if (heap != heap_sentinel || flags) return 0;
    if (!memory) { ++null_frees; return 0; }
    if (memory != storage) return 0;
    ++frees;
    return 1;
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
    *(void **)(image + 0x9478) = (void *)partition_heap;
    *(void **)(image + 0x9190) = (void *)heap_alloc;
    *(void **)(image + 0x9198) = (void *)heap_free;
    alloc_fn allocate = (alloc_fn)(image + 0x5870);
    free_fn release = (free_fn)(image + 0x5ac0);
    const uint64_t metadata[] = {0, 1, 0x9e, 0x3fff, 0x20000, 0x800000, 0x823fff, 0x200000000ULL};
    unsigned count = 0;
    for (unsigned type = 0; type < 5; ++type) {
        for (unsigned j = 0; j < 8; ++j) {
            uint64_t attr = ((uint64_t)type << 24) | metadata[j];
            if (allocate(17, attr) != storage) return 6;
            release(storage, attr);
            ++count;
        }
    }
    release(NULL, 0);
    if (count != 40 || allocations != 40 || frees != 40 || gets != 81 || null_frees != 1) return 7;
    puts("xmem-native-heap PASS attributes=40 alloc=40 free=40 null-free=1");
    return 0;
}
