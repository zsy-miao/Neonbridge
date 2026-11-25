// Independent guest for the Scarlett address computer. Frosty calls slot 2
// and copies each complete 0x6c8-byte plane before releasing via slot 1.
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <dxgiformat.h>
#include <array>
#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <cwchar>

namespace {
struct Desc {
    UINT dimension;
    UINT64 alignment, width;
    UINT height;
    UINT16 slices, mips;
    DXGI_FORMAT format;
    UINT samples, quality, layout, flags;
};
static_assert(sizeof(Desc) == 56 && offsetof(Desc, width) == 16 && offsetof(Desc, layout) == 44);

struct Layout {
    UINT64 bytes, alignment;
    UINT mips, planes;
    BYTE plane[4][0x6c8];
    BYTE tail[8];
};
static_assert(sizeof(Layout) == 0x1b40 && offsetof(Layout, plane) == 0x18);

struct Computer;
struct Methods {
    ULONG (WINAPI* add_ref)(Computer*);
    ULONG (WINAPI* release)(Computer*);
    HRESULT (WINAPI* get_layout)(Computer*, Layout*);
    void* other[3];
    UINT64 (WINAPI* element_offset)(Computer*, UINT, UINT, UINT64, UINT, UINT, UINT, UINT*);
};
struct Computer { const Methods* methods; };
extern "C" __declspec(dllimport) HRESULT WINAPI XGCreateTextureComputer(const Desc*, Computer**);

struct Sample {
    UINT width, height;
    UINT16 mips;
    DXGI_FORMAT format;
    UINT layout;
    UINT64 bytes, alignment, plane_hash;
};
// Reference outputs from Frosty's native XG module, SHA-256
// fee85ef80b6fe47edaa083270933c08a37f9e27e7541f86f731daecf7ead1493.
// Layouts 256 and 261 produce distinct footprints for the same RGBA texture.
constexpr Sample samples[]{
    {37, 13, 1, DXGI_FORMAT_R8G8B8A8_UNORM, 256, 3328, 256, 0xfc4498b176d2d5d7},
    {37, 13, 1, DXGI_FORMAT_R8G8B8A8_UNORM, 261, 8192, 4096, 0x4622024bbf0f7ee3},
    {128, 128, 5, DXGI_FORMAT_R8G8B8A8_UNORM, 261, 90112, 4096, 0xdb330eae8a07b417},
    {65, 33, 3, DXGI_FORMAT_BC7_UNORM, 261, 16384, 4096, 0x28e99d8a6e487a93},
};

struct Guarded {
    std::array<BYTE, 32> before;
    Layout layout;
    std::array<BYTE, 32> after;
};
bool guards_intact(const Guarded& output) {
    for (BYTE value : output.before) if (value != 0xa7) return false;
    for (BYTE value : output.after) if (value != 0xa7) return false;
    return true;
}

bool verify(const Sample& sample) {
    Desc desc{};
    desc.dimension = 3; // Texture2D
    desc.width = sample.width;
    desc.height = sample.height;
    desc.slices = 1;
    desc.samples = 1;
    desc.mips = sample.mips;
    desc.format = sample.format;
    desc.layout = sample.layout;
    Computer* computer = nullptr;
    HRESULT hr = XGCreateTextureComputer(&desc, &computer);
    if (FAILED(hr) || !computer) {
        std::fprintf(stderr, "XGCreateTextureComputer failed, HRESULT %#lx\n", static_cast<unsigned long>(hr));
        return false;
    }
    Guarded first, second;
    std::memset(&first, 0xa7, sizeof(first));
    std::memset(&second, 0xa7, sizeof(second));
    std::memset(&second.layout, 0x35, sizeof(Layout));
    hr = computer->methods->get_layout(computer, &first.layout);
    if (SUCCEEDED(hr)) hr = computer->methods->get_layout(computer, &second.layout);
    const ULONG remaining = computer->methods->release(computer);
    const auto& result = first.layout;
    UINT64 hash = 14695981039346656037ull;
    for (BYTE b : result.plane[0]) hash = (hash ^ b) * 1099511628211ull;
    if (FAILED(hr) || remaining || !guards_intact(first) || !guards_intact(second) ||
        std::memcmp(&result, &second.layout, sizeof(Layout)) || result.planes != 1 ||
        result.mips != sample.mips || result.bytes != sample.bytes ||
        result.alignment != sample.alignment || hash != sample.plane_hash) {
        std::fprintf(stderr, "XG layout mismatch for %ux%u, format %u, layout %u (hr %#lx, bytes %llu, hash %#llx)\n",
                     sample.width, sample.height, static_cast<unsigned>(sample.format), sample.layout,
                     static_cast<unsigned long>(hr), result.bytes, hash);
        return false;
    }
    return true;
}

// Hash the complete layout and every returned element address in mip/row order.
// Goldens were captured from the same native XG module as the samples above.
struct AddressCase {
    UINT width, height;
    UINT16 mips;
    DXGI_FORMAT format;
    UINT layout;
    UINT64 layout_hash, address_hash;
    UINT16 slices = 1;
};
constexpr AddressCase pixel_tiled{261, 183, 5, DXGI_FORMAT_B8G8R8A8_UNORM, 261, 0x245daf2aa8688834, 0xac277f5a5410c3ea};
constexpr AddressCase block_linear{128, 128, 5, DXGI_FORMAT_BC7_UNORM, 256, 0xcb6161b60b4aaca3, 0x86e3e5ec57eee485};
constexpr AddressCase block_tiled{65, 33, 5, DXGI_FORMAT_BC7_UNORM, 261, 0x8a0097d72947a2da, 0x6e056f26dbe773ba};
constexpr AddressCase pixel_linear{513, 257, 5, DXGI_FORMAT_R8G8B8A8_UNORM, 256, 0xa21aa988236a8804, 0x0fb6d256e228228b};
constexpr AddressCase bc1_linear{259, 139, 8, DXGI_FORMAT_BC1_TYPELESS, 256, 0xadcd43e0eec01d8a, 0x468434553d0df774};
constexpr AddressCase bc1_tiled{259, 139, 8, DXGI_FORMAT_BC1_TYPELESS, 261, 0xb1909691e9e1790b, 0x2a9d1b908bb36ae0};
constexpr AddressCase byte_linear{259, 139, 8, DXGI_FORMAT_R8_UNORM, 256, 0x4649f7d0fc454209, 0x24d3c1b3f7842cac};
constexpr AddressCase byte_tiled{259, 139, 8, DXGI_FORMAT_R8_UNORM, 261, 0xa3ced028ae416558, 0x3c722d34b146d88e};
constexpr AddressCase rg8_linear{259, 139, 8, DXGI_FORMAT_R8G8_UNORM, 256, 0xacbb77500086091e, 0xa13eed57cdb1e079};
constexpr AddressCase rg8_tiled{259, 139, 8, DXGI_FORMAT_R8G8_UNORM, 261, 0xad87d6cf75e6e0e1, 0x6b28338ea87c09e0};
constexpr AddressCase rgba16_linear{259, 139, 8, DXGI_FORMAT_R16G16B16A16_FLOAT, 256, 0xa832214633c68ee1, 0x27eb3630322e38fa};
constexpr AddressCase rgba16_tiled{259, 139, 8, DXGI_FORMAT_R16G16B16A16_FLOAT, 261, 0x20d9c8de8dcdff9a, 0x7d463f89365bf693};
constexpr AddressCase rgba32_linear{259, 139, 8, DXGI_FORMAT_R32G32B32A32_TYPELESS, 256, 0x90dd37a05cd9c0df, 0xaa3dfe79e51a39ff};
constexpr AddressCase rgba32_tiled{259, 139, 8, DXGI_FORMAT_R32G32B32A32_TYPELESS, 261, 0x4f010bdc22c4f087, 0x11ce489fbabf2a1b};
constexpr AddressCase address_cases[]{
    {136, 37, 7, DXGI_FORMAT_BC1_TYPELESS, 256, 0x1227553f1b613930, 0x89f46509c8221452, 3},
    {1, 29, 5, DXGI_FORMAT_R8G8B8A8_UNORM, 261, 0x5fb8b9b2e388ec41, 0xfed8254ff0c5da20},
    pixel_tiled, block_linear, block_tiled, pixel_linear,
    // Shape fields recorded during Frosty's startup (one sample with quality zero).
    {128, 128, 8, DXGI_FORMAT_BC1_TYPELESS, 261, 0x85898430b4e3a3c4, 0x1d1faa841e5cb615, 6},
    {512, 256, 1, DXGI_FORMAT_R8_UNORM, 261, 0x8c1810f5e071776d, 0xbc0482703c2efb25},
    {256, 1792, 1, DXGI_FORMAT_R16G16B16A16_FLOAT, 261, 0x26c5cba621281816, 0x7cd1e3b5f8f5e725},
    {32, 32, 6, DXGI_FORMAT_BC1_TYPELESS, 261, 0xddcbfa1c56d6b488, 0x248b06defc15ebde},
    {32, 32, 6, DXGI_FORMAT_B8G8R8A8_TYPELESS, 261, 0xba76c744255a51ae, 0xf62c0a92129835b4},
    {843, 5, 1, DXGI_FORMAT_B8G8R8A8_TYPELESS, 261, 0x3d34d55e54e1d625, 0x7f4f57f937403266},
    {512, 512, 10, DXGI_FORMAT_BC3_TYPELESS, 261, 0x57dcba197e218104, 0x43df594108211cfe},
    {256, 256, 9, DXGI_FORMAT_BC5_UNORM, 261, 0x733a03937023d402, 0xeaccc2ed7a2c7afe},
};
UINT64 hash_bytes(UINT64 hash, const void* bytes, size_t size) {
    auto p = static_cast<const BYTE*>(bytes);
    for (size_t i = 0; i < size; ++i) hash = (hash ^ p[i]) * 1099511628211ull;
    return hash;
}
bool verify_addresses(const AddressCase& test, UINT& checked) {
    Desc desc{};
    desc.dimension = 3;
    desc.width = test.width;
    desc.height = test.height;
    desc.mips = test.mips;
    desc.slices = test.slices;
    desc.samples = 1;
    desc.format = test.format;
    desc.layout = test.layout;
    Computer* c = nullptr;
    if (FAILED(XGCreateTextureComputer(&desc, &c)) || !c) return false;
    bool ok = c->methods->add_ref(c) == 2;
    ok = (c->methods->release(c) == 1) && ok;
    Guarded output;
    std::memset(&output, 0xa7, sizeof(output));
    constexpr UINT64 seed = 14695981039346656037ull;
    ok = SUCCEEDED(c->methods->get_layout(c, &output.layout)) && guards_intact(output) &&
         hash_bytes(seed, &output.layout, sizeof(output.layout)) == test.layout_hash && ok;
    UINT64 hash = seed;
    const bool compressed = (test.format >= DXGI_FORMAT_BC1_TYPELESS && test.format <= DXGI_FORMAT_BC5_SNORM) ||
                            (test.format >= DXGI_FORMAT_BC6H_TYPELESS && test.format <= DXGI_FORMAT_BC7_UNORM_SRGB);
    const UINT block = compressed ? 4 : 1;
    for (UINT mip = 0; mip < desc.mips; ++mip) {
        UINT w = test.width >> mip, h = test.height >> mip;
        w = ((w ? w : 1) + block - 1) / block;
        h = ((h ? h : 1) + block - 1) / block;
        for (UINT slice = 0; slice < test.slices; ++slice) {
            for (UINT y = 0; y < h; ++y) for (UINT x = 0; x < w; ++x) {
                UINT bit = 0xffff;
                const auto offset = c->methods->element_offset(c, 0, mip, x, y, slice, 0, &bit);
                ok = bit == 0 && offset < output.layout.bytes && ok;
                hash = hash_bytes(hash, &offset, sizeof(offset));
                ++checked;
            }
        }
    }
    ok = (c->methods->release(c) == 0) && hash == test.address_hash && ok;
    if (!ok) std::fprintf(stderr, "XG address mismatch for %ux%u, format %u, layout %u\n",
                          test.width, test.height, static_cast<unsigned>(test.format), test.layout);
    return ok;
}
// Exercise both layouts for each color interpretation against independently captured layouts
// and element addresses for the corresponding storage size.
bool verify_formats(UINT& formats, UINT& addresses) {
    constexpr DXGI_FORMAT pixels[]{
        DXGI_FORMAT_R10G10B10A2_TYPELESS, DXGI_FORMAT_R10G10B10A2_UNORM, DXGI_FORMAT_R10G10B10A2_UINT,
        DXGI_FORMAT_R11G11B10_FLOAT, DXGI_FORMAT_R8G8B8A8_TYPELESS, DXGI_FORMAT_R8G8B8A8_UNORM,
        DXGI_FORMAT_R8G8B8A8_UNORM_SRGB, DXGI_FORMAT_R8G8B8A8_UINT, DXGI_FORMAT_R8G8B8A8_SNORM,
        DXGI_FORMAT_R8G8B8A8_SINT, DXGI_FORMAT_R16G16_TYPELESS, DXGI_FORMAT_R16G16_FLOAT,
        DXGI_FORMAT_R16G16_UNORM, DXGI_FORMAT_R16G16_UINT, DXGI_FORMAT_R16G16_SNORM, DXGI_FORMAT_R16G16_SINT,
        DXGI_FORMAT_R32_TYPELESS, DXGI_FORMAT_R32_FLOAT, DXGI_FORMAT_R32_UINT, DXGI_FORMAT_R32_SINT,
        DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_B8G8R8X8_UNORM, DXGI_FORMAT_B8G8R8A8_TYPELESS,
        DXGI_FORMAT_B8G8R8A8_UNORM_SRGB, DXGI_FORMAT_B8G8R8X8_TYPELESS, DXGI_FORMAT_B8G8R8X8_UNORM_SRGB,
    };
    constexpr DXGI_FORMAT small_blocks[]{
        DXGI_FORMAT_BC1_TYPELESS, DXGI_FORMAT_BC1_UNORM, DXGI_FORMAT_BC1_UNORM_SRGB,
        DXGI_FORMAT_BC4_TYPELESS, DXGI_FORMAT_BC4_UNORM, DXGI_FORMAT_BC4_SNORM,
    };
    constexpr DXGI_FORMAT blocks[]{
        DXGI_FORMAT_BC2_TYPELESS, DXGI_FORMAT_BC2_UNORM, DXGI_FORMAT_BC2_UNORM_SRGB,
        DXGI_FORMAT_BC3_TYPELESS, DXGI_FORMAT_BC3_UNORM, DXGI_FORMAT_BC3_UNORM_SRGB,
        DXGI_FORMAT_BC5_TYPELESS, DXGI_FORMAT_BC5_UNORM, DXGI_FORMAT_BC5_SNORM,
        DXGI_FORMAT_BC6H_TYPELESS, DXGI_FORMAT_BC6H_UF16, DXGI_FORMAT_BC6H_SF16,
        DXGI_FORMAT_BC7_TYPELESS, DXGI_FORMAT_BC7_UNORM, DXGI_FORMAT_BC7_UNORM_SRGB,
    };
    auto check_pair = [&](DXGI_FORMAT format, AddressCase linear, AddressCase tiled) {
        linear.format = tiled.format = format;
        ++formats;
        return verify_addresses(linear, addresses) && verify_addresses(tiled, addresses);
    };
    struct Family { DXGI_FORMAT first, last; AddressCase linear, tiled; };
    constexpr Family families[]{
        {DXGI_FORMAT_R32G32B32A32_TYPELESS, DXGI_FORMAT_R32G32B32A32_SINT, rgba32_linear, rgba32_tiled},
        {DXGI_FORMAT_R16G16B16A16_TYPELESS, DXGI_FORMAT_R32G32_SINT, rgba16_linear, rgba16_tiled},
        {DXGI_FORMAT_R8G8_TYPELESS, DXGI_FORMAT_R16_FLOAT, rg8_linear, rg8_tiled},
        {DXGI_FORMAT_R16_UNORM, DXGI_FORMAT_R16_SINT, rg8_linear, rg8_tiled},
        {DXGI_FORMAT_R8_TYPELESS, DXGI_FORMAT_A8_UNORM, byte_linear, byte_tiled},
    };
    for (const auto& family : families) {
        for (int format = family.first; format <= family.last; ++format)
            if (!check_pair(static_cast<DXGI_FORMAT>(format), family.linear, family.tiled)) return false;
    }
    for (auto format : pixels) if (!check_pair(format, pixel_linear, pixel_tiled)) return false;
    for (auto format : small_blocks) if (!check_pair(format, bc1_linear, bc1_tiled)) return false;
    for (auto format : blocks) if (!check_pair(format, block_linear, block_tiled)) return false;
    return true;
}

// Time the same checked workload inside the guest after one warm-up. Process
// startup and report I/O stay outside the timed region.
bool benchmark_formats(FILE* output) {
    UINT formats = 0, addresses = 0;
    if (!verify_formats(formats, addresses)) return false;
    LARGE_INTEGER hz;
    if (!QueryPerformanceFrequency(&hz)) return false;
    for (UINT iteration = 0; iteration < 7; ++iteration) {
        formats = addresses = 0;
        LARGE_INTEGER start, end;
        if (!QueryPerformanceCounter(&start) || !verify_formats(formats, addresses) ||
            !QueryPerformanceCounter(&end)) return false;
        if (std::fprintf(output, "formats=%u addresses=%u ticks=%lld frequency=%lld\n",
                         formats, addresses, end.QuadPart - start.QuadPart, hz.QuadPart) < 0) return false;
    }
    return true;
}

}

int wmain(int argc, wchar_t** argv) {
    if (argc == 3 && !std::wcscmp(argv[1], L"--bench-formats") && *argv[2]) {
        FILE* file;
        if (_wfopen_s(&file, argv[2], L"wb")) return 1;
        const bool ok = benchmark_formats(file);
        return std::fclose(file) == 0 && ok ? 0 : 1;
    }
    const bool addresses = argc > 1 && std::wcscmp(argv[1], L"--addresses") == 0;
    const bool formats = argc > 1 && std::wcscmp(argv[1], L"--formats") == 0;
    const int next = (addresses || formats) ? 2 : 1;
    if (argc != next && (argc != next+2 || std::wcscmp(argv[next], L"--report") || !*argv[next+1])) {
        std::fputs("usage: xg-texture.exe [--addresses|--formats] [--report FILE] or --bench-formats FILE\n", stderr);
        return 2;
    }
    for (const auto& sample : samples) if (!verify(sample)) return 1;
    const char* receipt = "xg-texture PASS descriptors=4 layout-queries=8 plane-records=4 releases=4";
    char extended[96];
    if (addresses) {
        UINT checked = 0;
        for (const auto& test : address_cases) if (!verify_addresses(test, checked)) return 1;
        std::snprintf(extended, sizeof(extended), "xg-texture PASS layouts=18 traced-descriptors=8 element-addresses=%u", checked);
        receipt = extended;
    }
    if (formats) {
        UINT count = 0, offsets = 0;
        if (!verify_formats(count, offsets)) return 1;
        std::snprintf(extended, sizeof(extended), "xg-formats PASS formats=%u layouts=%u element-addresses=%u", count, count*2, offsets);
        receipt = extended;
    }
    if (argc == next+2) {
        FILE* file;
        if (_wfopen_s(&file, argv[next+1], L"wb")) return 1;
        const int written = std::fprintf(file, "%s\n", receipt);
        if (std::fclose(file) || written < 0) return 1;
    }
    std::puts(receipt);
    return 0;
}
