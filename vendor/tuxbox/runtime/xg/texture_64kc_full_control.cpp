// Differential guest for the Xbox-specific 2D layout and computer exports.
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <cstdint>
#include <cstdio>
#include <initializer_list>
#include <cstring>

struct Desc2D { uint32_t dimension; uint64_t alignment, width; uint32_t height; uint16_t slices, mips; uint32_t format, samples, quality, layout, flags; };
struct Comp;
struct Methods {
    ULONG (WINAPI* add_ref)(Comp*);
    ULONG (WINAPI* release)(Comp*);
    HRESULT (WINAPI* layout)(Comp*, void*);
    void* other[3];
    uint64_t (WINAPI* offset)(Comp*, uint32_t, uint32_t, uint64_t, uint32_t, uint32_t, uint32_t, uint32_t*);
};
struct Comp { const Methods* methods; };
struct Layout { uint8_t before[32], body[0x1b40], after[32]; };
using Create = HRESULT (WINAPI*)(const Desc2D*, Comp**);
using Compute = HRESULT (WINAPI*)(const Desc2D*, void*);
static_assert(sizeof(Desc2D) == 56);

int main(int argc, char** argv) {
    HMODULE dll = LoadLibraryA("xg_subject.dll");
    if (!dll) return 1;
    auto create = reinterpret_cast<Create>(GetProcAddress(dll, "XGCreateTextureComputer"));
    if (!create) return 2;
    Desc2D sample{3,0,512,512,1,1,90,1,0,0,0x300001};
    Comp* sentinel = reinterpret_cast<Comp*>(static_cast<uintptr_t>(0x1234));
    if (argc == 2 && !std::strcmp(argv[1], "unsupported")) {
        sample.flags = 2;
        if (create(&sample, &sentinel) != E_NOTIMPL || sentinel) return 4;
        std::puts("xg-64kc-full unsupported descriptor rejected"); return 0;
    }
    const uint32_t formats[] = {2,10,34,49,61};
    FILE* file = nullptr; if (fopen_s(&file, "texture-64kc-full-results.bin", "wb") || !file) return 6;
    struct Shape { uint32_t width, height; };
    constexpr Shape shapes[] = {{1024,1024}};
    unsigned cases = 0, addresses = 0;
    for (uint32_t format : formats)
    for (Shape shape : shapes) for (uint32_t levels : {1u}) for (uint32_t slices : {1u}) {
        if (shape.width == 1 && shape.height == 1 && levels != 1) continue;
        Desc2D desc{3,0,shape.width,shape.height,static_cast<uint16_t>(slices),static_cast<uint16_t>(levels),format,1,0,0,0xc00001};
        Layout direct, from_computer;
        std::memset(&direct, 0xa5, sizeof(direct)); std::memset(&from_computer, 0xa5, sizeof(from_computer));
        Comp* computer = nullptr;
        HRESULT b = create(&desc, &computer);
        if (b != S_OK || !computer || computer->methods->layout(computer, from_computer.body) != S_OK) return 7;
        std::memcpy(direct.body, from_computer.body, sizeof(direct.body));
        for (auto value : direct.before) if (value != 0xa5) return 8;
        for (auto value : direct.after) if (value != 0xa5) return 8;
        for (auto value : from_computer.before) if (value != 0xa5) return 8;
        for (auto value : from_computer.after) if (value != 0xa5) return 8;
        if (std::fwrite(direct.body, sizeof(direct.body), 1, file) != 1) return 9;
        const bool compressed = (format >= 70 && format <= 84) || (format >= 94 && format <= 99);
        for (uint32_t mip = 0; mip < levels; ++mip) for (uint32_t slice = 0; slice < slices; ++slice) {
            uint32_t w = shape.width >> mip, h = shape.height >> mip;
            if (!w) w = 1;
            if (!h) h = 1;
            if (compressed) { w = (w + 3) / 4; h = (h + 3) / 4; }
            for (uint32_t plane = 0; plane < 2; ++plane) for (uint32_t x = 0; x < w; ++x) for (uint32_t y = 0; y < h; ++y) {
                uint32_t bit = 0xffffffffu;
                uint64_t off = computer->methods->offset(computer, plane, mip, x, y, slice, 0, &bit);
                if (off == UINT64_MAX || std::fwrite(&off, sizeof(off), 1, file) != 1 ||
                    std::fwrite(&bit, sizeof(bit), 1, file) != 1) return 10;
                ++addresses;
            }
        }
        if (computer->methods->release(computer) != 0) return 11;
        ++cases;
    }
    if (std::fclose(file)) return 12;
    std::printf("xg-64kc-full PASS %u layouts %u element addresses\n", cases, addresses);
    return 0;
}
