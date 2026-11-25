/* Check the supplied xmem scatter entry point against a local NT stub. The
 * Python launcher verifies the exact console DLL fingerprint before use. */
#include <stdint.h>
#include <stdio.h>
#include <sys/mman.h>

typedef int (__attribute__((ms_abi)) *scatter_fn)(void *const *, size_t, const uint64_t *);
static unsigned calls, error_seen;
static void *const *expected_addresses;
static const uint64_t *expected_tokens;
static int should_fail;

static __attribute__((ms_abi)) int nt_scatter(void *const *addresses, size_t count, const uint64_t *tokens)
{
    ++calls;
    if (addresses != expected_addresses || count != 3 || tokens != expected_tokens) return 0xc000000d;
    return should_fail ? 0xc000000d : 0;
}
static __attribute__((ms_abi)) unsigned translate_status(unsigned status)
{
    return status == 0xc000000d || status == 0xc00000ef ? 87 : 1;
}
static __attribute__((ms_abi)) void set_error(unsigned error) { error_seen = error; }

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
    *(void **)(image + 0x93f0) = (void *)nt_scatter;
    *(void **)(image + 0x93c0) = (void *)translate_status;
    *(void **)(image + 0x9158) = (void *)set_error;
    image[0x8180] = 0xc3; /* Suppress console-only error logging. */
    scatter_fn scatter = (scatter_fn)(image + 0x2a50);
    void *addresses[] = {(void *)0x10000, (void *)0x40000, (void *)0x70000};
    uint64_t tokens[] = {1, 2, 3};
    expected_addresses = addresses;
    expected_tokens = tokens;
    if (!scatter(addresses, 3, tokens) || calls != 1) return 6;
    expected_tokens = NULL;
    if (!scatter(addresses, 3, NULL) || calls != 2) return 7;
    if (!scatter(addresses, 0, tokens) || calls != 2) return 8;
    if (scatter(NULL, 0, tokens) || calls != 2 || error_seen != 87) return 9;
    expected_tokens = tokens;
    should_fail = 1;
    error_seen = 0;
    if (scatter(addresses, 3, tokens) || calls != 3 || error_seen != 87) return 10;
    puts("xmem-native-scatter PASS args=3 unmap=1 zero=1 null=1 error=1");
    return 0;
}
