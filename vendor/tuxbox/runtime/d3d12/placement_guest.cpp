// Read back title-selected virtual addresses after late commitment and page replacement.
#include "compute_fixture.h"
#include "frame_cli.h"
#include <vector>

namespace {
using namespace frame_abi;
constexpr std::array<UINT, 18> release_profile{
    0x20004, 0, UINT_MAX, 0, 0, 0, UINT_MAX, UINT_MAX, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0};
constexpr UINT page_bytes = 65536, backing_bytes = 5 * page_bytes;
constexpr auto initial = D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE;
using Place = HRESULT (WINAPI*)(ID3D12Device*, UINT64, const D3D12_RESOURCE_DESC*, D3D12_RESOURCE_STATES,
                                const D3D12_CLEAR_VALUE*, REFIID, void**);

struct Sample {
    UINT offset, size, width, height;
    UINT16 layers, mips;
    DXGI_FORMAT format;
    UINT block, element_bytes;
    UINT seeds[2];
    UINT64 hashes[2];

    D3D12_RESOURCE_DESC describe() const {
        D3D12_RESOURCE_DESC desc{};
        desc.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
        desc.Width = width;
        desc.Height = height;
        desc.DepthOrArraySize = layers;
        desc.MipLevels = mips;
        desc.SampleDesc.Count = 1;
        desc.Format = format;
        desc.Layout = static_cast<D3D12_TEXTURE_LAYOUT>(0x105);
        return desc;
    }
};

// FNV-1a of every visible element in D3D subresource/row order. Each element's
// address was independently queried from Frosty's XG computer (original DLL
// SHA-256 fee85ef80b6fe47edaa083270933c08a37f9e27e7541f86f731daecf7ead1493).
constexpr Sample first{0, 4096, 32, 32, 1, 6, DXGI_FORMAT_BC1_TYPELESS, 4, 8,
                       {1, 3}, {0xf8aa7bc9d919b674ull, 0xf3431474335ee77full}};
constexpr Sample adjacent{4096, 4096, 32, 32, 1, 6, DXGI_FORMAT_BC1_TYPELESS, 4, 8,
                          {2, 4}, {0xf4e4d0e886920dd4ull, 0xaacde181af78d9b4ull}};
constexpr Sample cube{page_bytes, 73728, 128, 128, 6, 8, DXGI_FORMAT_BC1_TYPELESS, 4, 8,
                      {5, 6}, {0x50842e5f1eb952ffull, 0x43bda11f695f5a1full}};
constexpr Sample r8{3 * page_bytes, 131072, 512, 256, 1, 1, DXGI_FORMAT_R8_UNORM, 1, 1,
                    {7, 8}, {0xae2105e023ca1ab9ull, 0xdf15c9311484ecf6ull}};

struct Backing {
    BYTE* base = nullptr;
    explicit Backing(UINT64 start = 0, UINT64 limit = 0) {
        if (!start) {
            base = static_cast<BYTE*>(VirtualAlloc(nullptr, backing_bytes, MEM_RESERVE, PAGE_READWRITE));
            return;
        }
        MEMORY_BASIC_INFORMATION region{};
        while (start < limit && VirtualQuery(reinterpret_cast<void*>(start), &region, sizeof(region))) {
            const UINT64 next = reinterpret_cast<UINT64>(region.BaseAddress) + region.RegionSize;
            if (next <= start) break;
            if (region.State == MEM_FREE && backing_bytes <= limit - start && backing_bytes <= next - start) {
                base = static_cast<BYTE*>(VirtualAlloc(reinterpret_cast<void*>(start), backing_bytes, MEM_RESERVE, PAGE_READWRITE));
                if (base) break;
            }
            start = (next + 65535) & ~65535ull;
        }
    }
    ~Backing() { if (base) VirtualFree(base, 0, MEM_RELEASE); }

    void fill(UINT pass) {
        if (pass && !VirtualFree(base, backing_bytes, MEM_DECOMMIT)) throw std::runtime_error("decommit guest VA");
        if (!VirtualAlloc(base, backing_bytes, MEM_COMMIT, PAGE_READWRITE)) throw std::runtime_error("commit guest VA");
        for (const Sample* sample : {&first, &adjacent, &cube, &r8}) {
            UINT state = sample->seeds[pass];
            for (UINT j = 0; j < sample->size; ++j) {
                state = state * 1664525u + 1013904223u;
                base[sample->offset + j] = static_cast<BYTE>(state >> 24);
            }
        }
    }
};

struct Texture {
    const Sample& sample;
    ComPtr<ID3D12Resource> texture, buffer;
    std::vector<D3D12_PLACED_SUBRESOURCE_FOOTPRINT> layouts;
    std::vector<UINT> rows;
    std::vector<UINT64> row_bytes;
    UINT64 size{};

    Texture(const Sample& test, UINT64 base, ID3D12Device* device)
        : sample(test), layouts(UINT(test.layers) * test.mips), rows(layouts.size()), row_bytes(layouts.size()) {
        auto desc = sample.describe();
        auto va = base + sample.offset;
        auto place = method<Place>(device, 44);
        check(place(device, va, &desc, initial, nullptr, IID_PPV_ARGS(&texture)), "place reserved texture");
        const auto actual = texture->GetDesc();
        if (actual.Dimension != desc.Dimension || actual.Width != desc.Width || actual.Height != desc.Height ||
            actual.DepthOrArraySize != desc.DepthOrArraySize || actual.MipLevels != desc.MipLevels ||
            actual.Format != desc.Format || actual.Layout != desc.Layout || texture->GetGPUVirtualAddress() != va)
            throw std::runtime_error("placed guest texture identity changed");

        desc.Layout = D3D12_TEXTURE_LAYOUT_UNKNOWN;
        device->GetCopyableFootprints(&desc, 0, static_cast<UINT>(layouts.size()), 0, layouts.data(),
                                      rows.data(), row_bytes.data(), &size);
        if (!size || size > 4 * 1024 * 1024) throw std::runtime_error("invalid texture readback size");
        for (UINT i = 0; i < layouts.size(); ++i) {
            const UINT mip = i % sample.mips;
            const UINT width = (sample.width >> mip) ? sample.width >> mip : 1;
            const UINT height = (sample.height >> mip) ? sample.height >> mip : 1;
            const UINT count = (height + sample.block - 1) / sample.block;
            const UINT bytes = ((width + sample.block - 1) / sample.block) * sample.element_bytes;
            const auto& layout = layouts[i];
            if (rows[i] != count || row_bytes[i] != bytes || layout.Offset >= size ||
                layout.Footprint.RowPitch < bytes || UINT64(count - 1) * layout.Footprint.RowPitch + bytes > size - layout.Offset)
                throw std::runtime_error("invalid texture readback footprint");
        }
        buffer = compute_fixture::buffer(device, D3D12_HEAP_TYPE_READBACK, size, D3D12_RESOURCE_STATE_COPY_DEST);
    }

    void record(ID3D12GraphicsCommandList* list) const {
        compute_fixture::transition(list, texture.Get(), initial, D3D12_RESOURCE_STATE_COPY_SOURCE);
        D3D12_TEXTURE_COPY_LOCATION source{}, dest{};
        source.Type = D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
        source.pResource = texture.Get();
        dest.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
        dest.pResource = buffer.Get();
        for (UINT i = 0; i < layouts.size(); ++i) {
            source.SubresourceIndex = i;
            dest.PlacedFootprint = layouts[i];
            method<Copy>(list, 83)(list, &dest, 0, 0, 0, &source, nullptr, 0);
        }
        compute_fixture::transition(list, texture.Get(), D3D12_RESOURCE_STATE_COPY_SOURCE, initial);
    }

    void verify(UINT pass) const {
        D3D12_RANGE range{0, static_cast<SIZE_T>(size)}, empty{};
        void* mapped{};
        check(buffer->Map(0, &range, &mapped), "read back placed texture");
        UINT64 hash = 14695981039346656037ull;
        for (UINT i = 0; i < layouts.size(); ++i) {
            const auto& layout = layouts[i];
            for (UINT y = 0; y < rows[i]; ++y) {
                const auto row = static_cast<const BYTE*>(mapped) + layout.Offset + y * layout.Footprint.RowPitch;
                for (UINT x = 0; x < row_bytes[i]; ++x) hash = (hash ^ row[x]) * 1099511628211ull;
            }
        }
        buffer->Unmap(0, &empty);
        if (hash != sample.hashes[pass])
            throw std::runtime_error("placed texture data differs for seed " + std::to_string(sample.seeds[pass]));
    }
};

std::string run(const wchar_t* requested, bool) {
    DXGI_ADAPTER_DESC1 selected{};
    struct Config { UINT flags, count, preserve, padding; UINT64 start, end; } config{};
    if (GetEnvironmentVariableW(L"TUXBOX_TEST_CONFIGURED_VM", nullptr, 0)) {
        config.count = 1;
        using Configure = HRESULT (WINAPI*)(Config*);
        auto configure = reinterpret_cast<Configure>(GetProcAddress(GetModuleHandleW(L"d3d12_xs.dll"), "D3D12ConfigureVirtualMemoryX"));
        if (!configure || configure(&config) != 0x10000000 || config.start != 0x80000000ull || config.end != 0x7f8000000ull)
            throw std::runtime_error("configure guest GPU address range");
    }
    auto device = create_device(requested, selected, release_profile.data());
    const auto place = method<Place>(device.Get(), 44);
    Backing memory(config.start, config.end);
    if (!memory.base) throw std::runtime_error("reserve guest VA");
    const auto base = reinterpret_cast<UINT64>(memory.base);
    auto desc = first.describe();
    auto reject = [&](UINT64 va) {
        void* out = device.Get();
        if (SUCCEEDED(place(device.Get(), va, &desc, initial, nullptr, __uuidof(ID3D12Resource), &out)) || out)
            throw std::runtime_error("invalid guest VA placement accepted");
    };
    if (config.end) reject(config.end);
    reject(0);
    reject(base + 1);
    desc.Layout = static_cast<D3D12_TEXTURE_LAYOUT>(0xffffffffu);
    reject(base);

    using Enable = HRESULT (WINAPI*)(UINT64*, SIZE_T, UINT64*);
    auto enable_profiling = reinterpret_cast<Enable>(GetProcAddress(GetModuleHandleW(L"d3d12_xs.dll"),
                                                                    "D3D12XboxEnableMemoryProfiling"));
    UINT64 cursor = 1, samples[]{7, 8, 9};
    if (!enable_profiling) throw std::runtime_error("resolve memory profiler");
    check(enable_profiling(&cursor, 2, samples), "register profiling buffer");
    { Texture one(first, base, device.Get()), alias(first, base, device.Get()); }
    { Texture two(adjacent, base, device.Get()); }
    check(enable_profiling(nullptr, 0, nullptr), "unregister profiling buffer");
    { Texture three(first, base, device.Get()); }
    if (cursor != 3 || samples[0] != 7 || samples[1] == 8 || samples[2] != 9)
        throw std::runtime_error("profiling cursor, sample or guard");

    std::vector<Texture> textures;
    for (const Sample* test : {&first, &adjacent, &first, &cube, &r8})
        textures.emplace_back(*test, base, device.Get());

    D3D12_COMMAND_QUEUE_DESC queue_desc{};
    ComPtr<ID3D12CommandQueue> queue;
    ComPtr<ID3D12Fence> fence;
    check(device->CreateCommandQueue(&queue_desc, IID_PPV_ARGS(&queue)), "create direct queue");
    check(device->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&fence)), "create completion fence");
    for (UINT pass = 0; pass < 2; ++pass) {
        ComPtr<ID3D12CommandAllocator> allocator;
        ComPtr<ID3D12GraphicsCommandList> list;
        check(device->CreateCommandAllocator(queue_desc.Type, IID_PPV_ARGS(&allocator)), "create allocator");
        const UINT list_desc[]{D3D12_COMMAND_LIST_TYPE_DIRECT, 0, 1, 0};
        check(method<CreateList>(device.Get(), 73)(device.Get(), list_desc, allocator.Get(), nullptr, IID_PPV_ARGS(&list)),
              "create copy command list");
        for (const auto& texture : textures) texture.record(list.Get());
        check(list->Close(), "close texture copies");
        // Both initial commitment and recommit/write occur after the lists were recorded.
        memory.fill(pass);
        if (pass == 1) {
            using Flush = void (WINAPI*)();
            auto flush = reinterpret_cast<Flush>(GetProcAddress(GetModuleHandleW(L"d3d12_xs.dll"),
                                                                 "D3DFlushEntireCpuCache"));
            if (!flush) throw std::runtime_error("resolve CPU cache flush");
            flush();
            // The GPU must still see the second pattern after the guest
            // withdraws its CPU backing. Submit cannot upload those pages.
            if (!VirtualFree(memory.base, backing_bytes, MEM_DECOMMIT))
                throw std::runtime_error("decommit flushed guest pages");
        }
        ID3D12CommandList* batch[] = {list.Get()};
        queue->ExecuteCommandLists(1, batch);
        check(queue->Signal(fence.Get(), pass + 1), "signal texture readback");
        compute_fixture::wait_for_fence(fence.Get(), pass + 1);
        for (const auto& texture : textures) texture.verify(pass);
    }
    char receipt[180];
    std::snprintf(receipt, sizeof(receipt), "d3d12-placement PASS pci=%04x:%04x slot=44 VA-offsets=0,4096,0 "
                  "passes=2 BC1-blocks=522 cube-blocks=16404 R8-texels=262144 rejected=%d configured_vm=%d",
                  selected.VendorId, selected.DeviceId, config.end ? 4 : 3, config.end ? 1 : 0);
    return receipt;
}
}

int wmain(int argc, wchar_t** argv) {
    return tuxbox_test::frame_main(argc, argv, "d3d12-placement", false, run);
}
