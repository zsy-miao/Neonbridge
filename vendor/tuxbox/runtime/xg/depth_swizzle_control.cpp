// Differential control for the Xbox XG depth/stencil swizzle validator.
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <cstdint>
#include <cstdio>
#include <initializer_list>
using Validator = HRESULT (WINAPI*)(uint32_t, uint32_t, uint32_t);
int main() {
    HMODULE dll = LoadLibraryA("xg_subject.dll");
    if (!dll) return 1;
    auto validate = reinterpret_cast<Validator>(GetProcAddress(dll, "XGValidateDepthStencilSwizzleMode"));
    if (!validate) return 2;
    FILE* output = nullptr;
    if (fopen_s(&output, "depth-swizzle-results.bin", "wb") || !output) return 3;
    const uint32_t formats[] = {0,1,4,9,18,23,28,39,40,45,48,54,56,59,60,65,70,72,78,84,87,93,94,99,0xffffffffu};
    unsigned cases = 0;
    for (uint32_t format : formats) for (uint32_t samples : {0u,1u,2u,4u,8u,16u,0xffffffffu}) {
        for (uint32_t mode = 0; mode < 35; ++mode) {
            const uint32_t swizzle = mode < 32 ? mode : (mode == 32 ? 0x7fffffffu : mode == 33 ? 0x80000000u : 0xffffffffu);
            const HRESULT hr = validate(format, samples, swizzle);
            if (std::fwrite(&hr, sizeof(hr), 1, output) != 1) return 4;
            ++cases;
        }
    }
    if (std::fclose(output)) return 5;
    std::printf("xg-depth-swizzle PASS %u cases\n", cases);
    return 0;
}
