/* Native comparison for the isolated, fingerprinted XMemMakeTag routine.
 * The checked routine makes no calls and uses only its argument and stack.
 * The caller verifies the complete package DLL hash before launching this. */
#include <sys/mman.h>
#include <stdint.h>
#include <stdio.h>

typedef uint64_t (__attribute__((ms_abi)) *tag_fn)(uint32_t);

static uint64_t add_result(uint64_t hash, uint64_t value)
{
    for (unsigned i = 0; i < 8; ++i) {
        hash ^= (value >> (i * 8)) & 255;
        hash *= 1099511628211ULL;
    }
    return hash;
}

int main(int argc, char **argv)
{
    if (argc != 2) return 2;
    FILE *file = fopen(argv[1], "rb");
    if (!file) return 3;
    void *code = mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (code == MAP_FAILED) return 4;
    /* The exact reference build's ordinal-20 RVA 0x7ab0 maps to file offset
       0x6eb0, with its return instruction at the end of this 0x103-byte span. */
    if (fseek(file, 0x6eb0, SEEK_SET) || fread(code, 1, 0x103, file) != 0x103 ||
        mprotect(code, 4096, PROT_READ | PROT_EXEC)) return 5;
    fclose(file);
    tag_fn tag = (tag_fn)code;
    uint64_t hash = 14695981039346656037ULL;
    unsigned count = 0;
    for (unsigned shift = 0; shift < 32; shift += 8) {
        for (unsigned c = 0; c < 256; ++c) {
            uint32_t value = (0x41414141u & ~(0xffu << shift)) | (c << shift);
            hash = add_result(hash, tag(value));
            ++count;
        }
    }
    uint32_t seed = 0x6d2b79f5;
    for (unsigned i = 0; i < 4096; ++i) {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        hash = add_result(hash, tag(seed));
        ++count;
    }
    printf("zero=%016llx sample=%016llx count=%u hash=%016llx\n",
           (unsigned long long)tag(0), (unsigned long long)tag(0x41414141), count,
           (unsigned long long)hash);
    munmap(code, 4096);
    return 0;
}
