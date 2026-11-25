/* Check the hash-pinned XMemGetWriteWatch thunk and its paused branch. The
 * launcher maps the supplied PE into a temporary file, never the repository. */
#include <stdint.h>
#include <stdio.h>
#include <sys/mman.h>

typedef uint32_t (__attribute__((ms_abi)) *query_fn)(
    uint32_t, const void *, size_t, void **, size_t *, uint32_t *);
typedef void (__attribute__((ms_abi)) *option_fn)(const void *);
typedef int (__attribute__((ms_abi)) *resume_fn)(void);
static unsigned calls;

static __attribute__((ms_abi)) uint32_t watched_memory(
    uint32_t flags, const void *base, size_t size, void **addresses, size_t *count, uint32_t *granularity)
{
    ++calls;
    if (flags != 1 || base != (void *)0x12340000 || size != 0x23000 ||
        !addresses || !count || !granularity) return 0xbad;
    addresses[0] = (void *)0x12344000;
    *count = 1;
    *granularity = 4096;
    return 0x12ab;
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
    /* Substitute the imported GetWriteWatch, keeping the original thunk. */
    *(void **)(image + 0x9208) = (void *)watched_memory;
    query_fn query = (query_fn)(image + 0x4000);
    option_fn option = (option_fn)(image + 0x3b50);
    resume_fn resume = (resume_fn)(image + 0x4020);
    void *addresses[2] = {0};
    size_t count = 2;
    uint32_t granularity = 0;
    *(uint32_t *)(image + 0xe850) = 0;
    if (query(1, (void *)0x12340000, 0x23000, addresses, &count, &granularity) != 0x12ab ||
        calls != 1 || count != 1 || granularity != 4096 || addresses[0] != (void *)0x12344000) return 6;
    *(uint32_t *)(image + 0xe850) = 1;
    addresses[0] = NULL;
    count = 2;
    granularity = 0;
    if (query(1, (void *)0x12340000, 0x23000, addresses, &count, &granularity) != 1003 ||
        calls != 1 || count != 2 || granularity != 0 || addresses[0]) return 7;
    struct { uint32_t id, pad, value, reserved; } setting = {0x10000, 0, 1, 0};
    *(uint32_t *)(image + 0xe850) = 0;
    *(uint32_t *)(image + 0xe8b0) = 0;
    if (resume() != 1) return 8;
    option(&setting);
    if (*(uint32_t *)(image + 0xe850) != 1 || *(uint32_t *)(image + 0xe8b0) != 1 || resume() != 0) return 9;
    setting.value = 0;
    setting.reserved = 1; /* Only the low 32 bits of the payload are read. */
    option(&setting);
    if (*(uint32_t *)(image + 0xe850) != 1 || *(uint32_t *)(image + 0xe8b0) != 0 || resume() != 1 ||
        *(uint32_t *)(image + 0xe850) != 0) return 10;
    puts("xmem-native-watch PASS forwarded=6 paused=1003 output=untouched option=2 resume=3");
    return 0;
}
