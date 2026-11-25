// Compare the supplied Xbox XG implementation and the TuxBox provider using
// identical descriptors. The host compares the complete output files.
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <cstdint>
#include <cstdio>
#include <cstring>

struct Desc { uint32_t size, reserved04, flags, reserved0c, options; };
using Compute = HRESULT (WINAPI*)(const Desc*, void*);
struct Result { int32_t hr; uint8_t before[32], layout[0x1b40], after[32]; };
static_assert(sizeof(Desc) == 20 && sizeof(Result) == 0x1b84);

int main() {
    HMODULE dll = LoadLibraryA("xg_subject.dll");
    if (!dll) { std::fprintf(stderr, "LoadLibrary failed: %lu\n", GetLastError()); return 1; }
    auto compute = reinterpret_cast<Compute>(GetProcAddress(dll, "XGComputeBufferLayout"));
    if (!compute) return 2;
    FILE* output = nullptr;
    if (fopen_s(&output, "buffer-results.bin", "wb") || !output) return 3;
    unsigned cases = 0;
    auto run = [&](const Desc& d) {
        Result result;
        std::memset(&result, 0xa5, sizeof(result));
        result.hr = compute(&d, result.layout);
        if (std::fwrite(&result, sizeof(result), 1, output) != 1) return false;
        for (uint8_t byte : result.before) if (byte != 0xa5) return false;
        for (uint8_t byte : result.after) if (byte != 0xa5) return false;
        if (result.hr < 0) for (uint8_t byte : result.layout) if (byte != 0xa5) return false;
        ++cases;
        return true;
    };
    const uint32_t sizes[] = {0, 1, 33, 65535, 65536, 65537, 0xffffffffu};
    for (uint32_t size : sizes) {
        Desc d{size, 0, 0, 0, 0};
        if (!run(d)) return 4;
        for (unsigned bit = 0; bit < 32; ++bit) {
            d.flags = 1u << bit;
            if (!run(d)) return 5;
            d.flags = 0; d.options = 1u << bit;
            if (!run(d)) return 6;
            d.options = 0;
        }
    }
    uint32_t rng = 0x925a1f31;
    for (unsigned i = 0; i < 512; ++i) {
        auto next = [&] { rng ^= rng << 13; rng ^= rng >> 17; rng ^= rng << 5; return rng; };
        Desc d{sizes[next() % 7], next(), next() & 0x6ffu, next(), next() & 0x0c7a92f5u};
        if (!run(d)) return 7;
    }
    Desc valid{1, 0, 0, 0, 0};
    Result untouched;
    std::memset(&untouched, 0xa5, sizeof(untouched));
    HRESULT no_desc = compute(nullptr, untouched.layout);
    HRESULT no_output = compute(&valid, nullptr);
    std::fclose(output);
    if (no_desc != E_INVALIDARG || no_output != E_INVALIDARG) return 8;
    for (uint8_t byte : untouched.layout) if (byte != 0xa5) return 9;
    std::printf("xg-buffer PASS %u descriptors, null arguments, complete layouts and guards\n", cases);
    return 0;
}
