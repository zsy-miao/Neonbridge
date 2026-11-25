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
    Desc2D sample{3,0,4,4,1,1,83,1,0,0,0};
    Comp* sentinel = reinterpret_cast<Comp*>(static_cast<uintptr_t>(0x1234));
    if (argc == 2 && !std::strcmp(argv[1], "unsupported")) {
        sample.flags = 2;
        if (create(&sample, &sentinel) != E_NOTIMPL || sentinel) return 4;
        std::puts("xg-64kmips unsupported descriptor rejected"); return 0;
    }
    const uint32_t formats[] = {1,2,3,4,9,10,11,12,13,14,15,16,17,18,23,24,25,26,27,28,29,30,31,32,33,34,35,36,37,38,39,41,42,43,48,49,50,51,52,53,54,56,57,58,59,60,61,62,63,64,65,70,71,72,73,74,75,76,77,78,79,80,81,82,83,84,87,88,90,91,92,93,94,95,96,97,98,99};
    FILE* file = nullptr; if (fopen_s(&file, "texture-64kmips-results.bin", "wb") || !file) return 6;
    struct Shape { uint32_t width, height; };
    constexpr Shape shapes[] = {{256,256}, {512,512}, {1024,1024}};
    unsigned cases = 0, addresses = 0;
    for (uint32_t format : formats) for (uint32_t flag : {1u,5u,0x100001u,0x100005u,0x200001u,0x200005u,0x300001u,0x300005u})
    for (Shape shape : shapes) for (uint32_t levels : {2u,3u,5u,8u,10u}) for (uint32_t slices : {1u}) {
        if (levels > (shape.width == 256 ? 9u : shape.width == 512 ? 10u : 11u)) continue;
        const unsigned bytes = format <= 4 ? 16 : format <= 18 ? 8 : (format >= 48 && format <= 59) ? 2 : (format >= 60 && format <= 65) ? 1 : 4;
        if ((format >= 70 && format <= 84) || (format >= 94 && format <= 99)) continue;
        const unsigned tw = bytes <= 2 ? 256 : bytes <= 8 ? 128 : 64;
        const unsigned th = bytes == 1 ? 256 : bytes <= 4 ? 128 : 64;
        unsigned first = levels; for (unsigned i = 0; i < levels; ++i) if ((shape.width >> i) <= tw/2 && (shape.height >> i) <= th) { first = i; break; }
        const unsigned max_tail = bytes == 16 ? 5 : bytes >= 4 ? 6 : 7;
        if (levels - first > max_tail) continue;
        Desc2D desc{3,0,shape.width,shape.height,static_cast<uint16_t>(slices),static_cast<uint16_t>(levels),format,1,0,0,flag};
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
            const auto at = [](uint32_t n, uint32_t v) { return v < n ? v : n-1; };
            const uint32_t xs[] = {0, at(w,1), at(w,7), at(w,8), at(w,15), at(w,16), at(w,63), at(w,64), w-1};
            const uint32_t ys[] = {0, at(h,1), at(h,7), at(h,8), at(h,15), at(h,16), at(h,63), at(h,64), h-1};
            for (uint32_t x : xs) for (uint32_t y : ys) {
                uint32_t bit = 0xffffffffu;
                uint64_t off = computer->methods->offset(computer, 0, mip, x, y, slice, 0, &bit);
                if (off == UINT64_MAX || bit || std::fwrite(&off, sizeof(off), 1, file) != 1 ||
                    std::fwrite(&bit, sizeof(bit), 1, file) != 1) return 10;
                ++addresses;
            }
        }
        if (computer->methods->release(computer) != 0) return 11;
        ++cases;
    }
    if (std::fclose(file)) return 12;
    std::printf("xg-64kmips PASS %u layouts %u element addresses\n", cases, addresses);
    return 0;
}
