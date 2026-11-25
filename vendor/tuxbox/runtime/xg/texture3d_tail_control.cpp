// Differential guest for the Xbox-specific 3D layout and computer exports.
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <cstdint>
#include <cstdio>
#include <initializer_list>
#include <cstring>

struct Desc3D { uint32_t width, height, depth, mips, format, usage, bind_flags, cpu_access, misc_flags, swizzle, pitch; };
struct ResourceDesc { uint32_t dimension; uint64_t alignment, width; uint32_t height; uint16_t depth, mips;
                      uint32_t format, samples, quality, layout, flags; };
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
using Create = HRESULT (WINAPI*)(const Desc3D*, Comp**);
using Compute = HRESULT (WINAPI*)(const Desc3D*, void*);
using CreateGeneric = HRESULT (WINAPI*)(const ResourceDesc*, Comp**);
static_assert(sizeof(Desc3D) == 44);
static_assert(sizeof(ResourceDesc) == 56);

int main(int argc, char** argv) {
    HMODULE dll = LoadLibraryA("xg_subject.dll");
    if (!dll) return 1;
    auto create = reinterpret_cast<Create>(GetProcAddress(dll, "XGCreateTexture3DComputer"));
    auto compute = reinterpret_cast<Compute>(GetProcAddress(dll, "XGComputeTexture3DLayout"));
    auto generic = reinterpret_cast<CreateGeneric>(GetProcAddress(dll, "XGCreateTextureComputer"));
    if (!create || !compute || !generic) return 2;
    Desc3D sample{37, 13, 3, 3, 28, 3, 8, 0, 0, 0, 0};
    Layout unchanged; std::memset(&unchanged, 0xa5, sizeof(unchanged));
    Comp* sentinel = reinterpret_cast<Comp*>(static_cast<uintptr_t>(0x1234));
    if (compute(nullptr, unchanged.body) != E_INVALIDARG || compute(&sample, nullptr) != E_INVALIDARG ||
        create(nullptr, &sentinel) != E_INVALIDARG || sentinel != reinterpret_cast<Comp*>(static_cast<uintptr_t>(0x1234)) ||
        create(&sample, nullptr) != E_INVALIDARG) return 3;
    if (argc == 2 && !std::strcmp(argv[1], "unsupported")) {
        // This usage is outside TuxBox's verified subset.
        sample.usage = 2; sample.swizzle = 0;
        if (compute(&sample, unchanged.body) != E_NOTIMPL || create(&sample, &sentinel) != E_NOTIMPL || sentinel) return 4;
        sample.usage = 0; sample.swizzle = 5; sample.width = 13; // Packed mip tails are not yet supported.
        if (compute(&sample, unchanged.body) != E_NOTIMPL || create(&sample, &sentinel) != E_NOTIMPL || sentinel) return 4;
        ResourceDesc unsupported{4,0,13,13,3,3,28,1,0,261,0};
        if (generic(&unsupported, &sentinel) != E_NOTIMPL || sentinel) return 4;
        sample.width = 8; sample.height = 1; sample.format = 61; // Three packed tail levels.
        if (compute(&sample, unchanged.body) != E_NOTIMPL || create(&sample, &sentinel) != E_NOTIMPL || sentinel) return 4;
        for (uint8_t b : unchanged.body) if (b != 0xa5) return 5;
        std::puts("xg-3d unsupported descriptor rejected"); return 0;
    }
    const uint32_t formats[] = {1,2,3,4,9,10,11,12,13,14,15,16,17,18,23,24,25,26,27,28,29,30,31,32,33,34,35,36,37,38,39,41,42,43,48,49,50,51,52,53,54,56,57,58,59,60,61,62,63,64,65,87,88,90,91,92,93};
    FILE* file = nullptr; if (fopen_s(&file, "texture3d-tail-results.bin", "wb") || !file) return 6;
    struct Shape { uint32_t width, height; };
    constexpr Shape shapes[] = {{8,1}, {8,2}, {8,3}, {8,4}, {16,1}, {16,2}, {16,3}, {16,4}, {32,1}, {32,2}, {32,3}, {32,4}, {64,1}, {64,2}, {64,3}, {64,4}, {128,1}, {128,2}, {128,3}, {128,4}};
    unsigned cases = 0, addresses = 0;
    for (uint32_t format : formats) for (uint32_t variant : {3u,4u})
    for (Shape shape : shapes) for (uint32_t levels : {2u,3u,4u}) for (uint32_t depth : {1u,8u,17u,128u,2048u}) {
        if (shape.width == 1 && shape.height == 1 && levels != 1) continue;
        const uint32_t tw = format <= 4 ? 4 : ((format >= 60 && format <= 65) ? 16 : 8);
        unsigned tails = 0; for (uint32_t i = 0; i < levels; ++i) if ((shape.width >> i) <= tw) ++tails;
        if (!tails || tails > 2) continue;
        const bool tiled = variant >= 3;

        const uint32_t usage = variant == 2 ? 3u : variant % 3;
        Desc3D desc{shape.width,shape.height,depth,levels,format,usage,8,0,0,tiled ? 5u : 0u,0};
        Layout direct, from_computer, from_generic;
        std::memset(&direct, 0xa5, sizeof(direct)); std::memset(&from_computer, 0xa5, sizeof(from_computer));
        std::memset(&from_generic, 0xa5, sizeof(from_generic));
        Comp* computer = nullptr; Comp* generic_computer = nullptr;
        ResourceDesc resource{4,0,shape.width,shape.height,static_cast<uint16_t>(depth),
                              static_cast<uint16_t>(levels),format,1,0,tiled ? 261u : 256u,0};
        HRESULT a = compute(&desc, direct.body), b = create(&desc, &computer);
        HRESULT c = generic(&resource, &generic_computer);
        if (a != S_OK || b != S_OK || c != S_OK || !computer || !generic_computer ||
            computer->methods->layout(computer, from_computer.body) != S_OK ||
            generic_computer->methods->layout(generic_computer, from_generic.body) != S_OK ||
            std::memcmp(direct.body, from_computer.body, sizeof(direct.body)) ||
            std::memcmp(direct.body, from_generic.body, sizeof(direct.body))) return 7;
        for (auto value : direct.before) if (value != 0xa5) return 8;
        for (auto value : direct.after) if (value != 0xa5) return 8;
        for (auto value : from_computer.before) if (value != 0xa5) return 8;
        for (auto value : from_computer.after) if (value != 0xa5) return 8;
        for (auto value : from_generic.before) if (value != 0xa5) return 8;
        for (auto value : from_generic.after) if (value != 0xa5) return 8;
        if (std::fwrite(direct.body, sizeof(direct.body), 1, file) != 1) return 9;
        for (uint32_t mip = 0; mip < levels; ++mip) {
            uint32_t w = shape.width >> mip, h = shape.height >> mip, d = depth >> mip;
            if (!w) w = 1;
            if (!h) h = 1;
            if (!d) d = 1;
            if (tiled) {
                const auto at = [](uint32_t n, uint32_t v) { return v < n ? v : n-1; };
                const uint32_t xs[] = {0, at(w, 1), at(w, 4), at(w, 8), w-1};
                const uint32_t ys[] = {0, at(h, 1), at(h, 4), at(h, 8), h-1};
                const uint32_t zs[] = {0, at(d, 1), at(d, 4), at(d, 8), d-1};
                for (uint32_t x : xs) for (uint32_t y : ys) for (uint32_t z : zs) {
                    uint32_t bit = 0xffffffffu;
                    uint64_t off = computer->methods->offset(computer, 0, mip, x, y, z, 0, &bit);
                    uint32_t generic_bit = 0xffffffffu;
                    uint64_t generic_off = generic_computer->methods->offset(generic_computer, 0, mip, x, y, z, 0, &generic_bit);
                    if (off == UINT64_MAX || bit || generic_off != off || generic_bit != bit || std::fwrite(&off, sizeof(off), 1, file) != 1 ||
                        std::fwrite(&bit, sizeof(bit), 1, file) != 1) return 10;
                    ++addresses;
                }
                uint32_t bit = 0xffffffffu;
                uint64_t off = computer->methods->offset(computer, 0, mip, 0, 0, d, 0, &bit);
                uint32_t generic_bit = 0xffffffffu;
                uint64_t generic_off = generic_computer->methods->offset(generic_computer, 0, mip, 0, 0, d, 0, &generic_bit);
                if (off != UINT64_MAX || bit || generic_off != off || generic_bit != bit || std::fwrite(&off, sizeof(off), 1, file) != 1 ||
                    std::fwrite(&bit, sizeof(bit), 1, file) != 1) return 10;
                ++addresses;
                continue;
            }
            for (uint32_t z : {0u, d/2, d-1, d}) {
                uint32_t bit = 0xffffffffu;
                uint64_t off = computer->methods->offset(computer, 0, mip, w-1, h-1, z, 0, &bit);
                uint32_t generic_bit = 0xffffffffu;
                uint64_t generic_off = generic_computer->methods->offset(generic_computer, 0, mip, w-1, h-1, z, 0, &generic_bit);
                if ((z < d) != (off != UINT64_MAX) || bit || generic_off != off || generic_bit != bit ||
                    std::fwrite(&off, sizeof(off), 1, file) != 1 ||
                    std::fwrite(&bit, sizeof(bit), 1, file) != 1) return 10;
                ++addresses;
            }
        }
        if (computer->methods->release(computer) != 0 || generic_computer->methods->release(generic_computer) != 0) return 11;
        ++cases;
    }
    if (std::fclose(file)) return 12;
    std::printf("xg-3d-tail PASS %u layouts %u element addresses\n", cases, addresses);
    return 0;
}
