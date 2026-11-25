/* Run the hash-pinned XMemGetTitleHeap and simple heap-creation paths with
 * controlled partition heap callbacks; no console binary is stored. */
#include <stdint.h>
#include <stdio.h>
#include <sys/mman.h>

typedef void *(__attribute__((ms_abi)) *title_fn)(void);
typedef void *(__attribute__((ms_abi)) *create_fn)(uint64_t, void *);
static char title_heap[8], private_heap[8];
static unsigned get_calls, create_calls, expected_flags, last_error;
static __attribute__((ms_abi)) void set_error(unsigned error) { last_error = error; }
static __attribute__((ms_abi)) void *get_heap(int partition)
{
    if (partition != 1) return NULL;
    ++get_calls;
    return title_heap;
}
static __attribute__((ms_abi)) void *create_heap(unsigned partition, unsigned flags, void *base, unsigned value)
{
    if (partition != 1 || flags != expected_flags || base || value != 1) return NULL;
    ++create_calls;
    return private_heap;
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
    *(void **)(image + 0x9478) = (void *)get_heap;
    *(void **)(image + 0x9488) = (void *)create_heap;
    *(void **)(image + 0x9158) = (void *)set_error;
    image[0x8180] = 0xc3; /* Suppress console-only diagnostic logging. */
    title_fn title = (title_fn)(image + 0x3b40);
    create_fn create = (create_fn)(image + 0x3740);
    if (title() != title_heap || get_calls != 1) return 6;
    expected_flags = 3;
    if (create(0, NULL) != private_heap || create(0, (void *)0x10000) != private_heap ||
        create(0x8000, NULL) != private_heap) return 7;
    expected_flags = 2;
    if (create(4, NULL) != private_heap || create(0x8004, (void *)0x10000) != private_heap ||
        create_calls != 5) return 8;
    if (create(2, NULL) || last_error != 87) return 9;
    last_error = 0;
    if (create(2, (void *)0x10001) || last_error != 87) return 10;
    last_error = 0;
    if (create(6, (void *)0x10000) || last_error != 87) return 11;
    last_error = 0;
    if (create(8, NULL) || last_error != 87) return 12;
    puts("xmem-native-heaps PASS title=1 create=5 flags=3/2 invalid=4");
    return 0;
}
