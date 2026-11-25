// Place two buffers in one Frosty-style heap and copy distinct GPU results back.
#include "compute_fixture.h"
#include "frame_cli.h"
#include <vector>

namespace {
using namespace frame_abi;
constexpr std::array<UINT, 18> release_profile{
    0x20004, 0, UINT_MAX, 0, 0, 0, UINT_MAX, UINT_MAX, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0};
constexpr UINT64 alignment = D3D12_DEFAULT_RESOURCE_PLACEMENT_ALIGNMENT;
constexpr size_t words = 128;

struct VolumeCopy {
    static constexpr UINT width = 5, height = 3, depth = 3;
    ComPtr<ID3D12Resource> texture, upload, readback;
    D3D12_PLACED_SUBRESOURCE_FOOTPRINT layout{};
    UINT64 bytes{};

    VolumeCopy(ID3D12Device* device, const D3D12_HEAP_PROPERTIES& heap, ID3D12GraphicsCommandList* list) {
        D3D12_RESOURCE_DESC desc{};
        desc.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE3D;
        desc.Width = width;
        desc.Height = height;
        desc.DepthOrArraySize = depth;
        desc.MipLevels = 1;
        desc.SampleDesc.Count = 1;
        desc.Format = DXGI_FORMAT_R32_UINT;
        check(device->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COPY_DEST,
                                              nullptr, IID_PPV_ARGS(&texture)), "create volume texture");
        UINT rows{};
        UINT64 row_bytes{};
        device->GetCopyableFootprints(&desc, 0, 1, 0, &layout, &rows, &row_bytes, &bytes);
        if (rows != height || row_bytes != width * sizeof(UINT) || layout.Footprint.Depth != depth ||
            layout.Offset != 0 || layout.Footprint.RowPitch < row_bytes || layout.Footprint.RowPitch > 65536 ||
            bytes < index(width - 1, height - 1, depth - 1) + sizeof(UINT) || bytes > 65536)
            throw std::runtime_error("unexpected volume footprint");
        upload = compute_fixture::buffer(device, D3D12_HEAP_TYPE_UPLOAD, bytes, D3D12_RESOURCE_STATE_GENERIC_READ);
        readback = compute_fixture::buffer(device, D3D12_HEAP_TYPE_READBACK, bytes, D3D12_RESOURCE_STATE_COPY_DEST);
        std::vector<BYTE> values(static_cast<size_t>(bytes), 0x7a);
        for (UINT z = 0; z < depth; ++z)
            for (UINT y = 0; y < height; ++y)
                for (UINT x = 0; x < width; ++x) {
                    const UINT value = 0x10000 * (z + 1) + 0x100 * y + x;
                    std::memcpy(values.data() + index(x, y, z), &value, sizeof(value));
                }
        compute_fixture::upload(upload.Get(), values.data(), values.size());
        D3D12_TEXTURE_COPY_LOCATION memory{}, image{};
        memory.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
        memory.pResource = upload.Get();
        memory.PlacedFootprint = layout;
        image.Type = D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
        image.pResource = texture.Get();
        const auto copy = method<Copy>(list, 83);
        for (UINT z = 0; z < depth; ++z) {
            const D3D12_BOX slice{0, 0, z, width, height, z + 1};
            copy(list, &image, 0, 0, depth - 1 - z, &memory, &slice, 0);
        }
        compute_fixture::transition(list, texture.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE);
        memory.pResource = readback.Get();
        copy(list, &memory, 0, 0, 0, &image, nullptr, 0);
    }

    size_t index(UINT x, UINT y, UINT z) const {
        return static_cast<size_t>(layout.Offset) + (z * height + y) * layout.Footprint.RowPitch + x * sizeof(UINT);
    }

    void verify() const {
        D3D12_RANGE range{0, static_cast<SIZE_T>(bytes)}, empty{};
        void* mapped{};
        check(readback->Map(0, &range, &mapped), "read volume texture slices");
        bool matches = true;
        for (UINT z = 0; z < depth; ++z)
            for (UINT y = 0; y < height; ++y)
                for (UINT x = 0; x < width; ++x) {
                    UINT value{};
                    std::memcpy(&value, static_cast<const BYTE*>(mapped) + index(x, y, z), sizeof(value));
                    matches &= value == 0x10000 * (depth - z) + 0x100 * y + x;
                }
        readback->Unmap(0, &empty);
        if (!matches) throw std::runtime_error("volume readback lost or reordered texels");
    }
};

std::string run(const wchar_t* requested, bool) {
    DXGI_ADAPTER_DESC1 selected{};
    auto device = create_device(requested, selected, release_profile.data());
    D3D12_HEAP_DESC layout{};
    layout.SizeInBytes = 32 * 1024 * 1024;
    layout.Properties.Type = D3D12_HEAP_TYPE_DEFAULT;
    layout.Properties.CreationNodeMask = layout.Properties.VisibleNodeMask = 1;
    layout.Flags = D3D12_HEAP_FLAG_ALLOW_ONLY_BUFFERS;
    ComPtr<ID3D12Heap> heap;
    check(device->CreateHeap(&layout, IID_PPV_ARGS(&heap)), "create GPU memory heap");
    ComPtr<ID3D12Device> owner;
    check(heap->GetDevice(IID_PPV_ARGS(&owner)), "memory heap parent");
    const auto actual = heap->GetDesc();
    if (owner.Get() != device.Get() || actual.SizeInBytes < layout.SizeInBytes ||
        actual.Properties.Type != layout.Properties.Type || actual.Flags != layout.Flags)
        throw std::runtime_error("memory heap lost its parent or description");

    D3D12_RESOURCE_DESC desc{};
    desc.Dimension = D3D12_RESOURCE_DIMENSION_BUFFER;
    desc.Alignment = alignment;
    desc.Width = sizeof(UINT) * words;
    desc.Height = 1;
    desc.DepthOrArraySize = 1;
    desc.MipLevels = 1;
    desc.SampleDesc.Count = 1;
    desc.Layout = D3D12_TEXTURE_LAYOUT_ROW_MAJOR;
    desc.Flags = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
    constexpr auto initial = D3D12_RESOURCE_STATE_UNORDERED_ACCESS;
    DXGI_ADAPTER_DESC1 second_adapter{};
    auto second = create_device(requested, second_adapter, release_profile.data());
    auto reject = [&](ID3D12Device* target, UINT64 offset) {
        void* output = heap.Get();
        const HRESULT hr = target->CreatePlacedResource(heap.Get(), offset, &desc, initial, nullptr,
                                                        __uuidof(ID3D12Resource), &output);
        if (hr != E_INVALIDARG || output) throw std::runtime_error("invalid placement succeeded");
    };
    reject(second.Get(), 0);
    reject(device.Get(), 1);
    reject(device.Get(), actual.SizeInBytes);
    reject(device.Get(), UINT64_MAX & ~(alignment - 1));
    ComPtr<ID3D12Resource> blocks[2];
    for (UINT i = 0; i < 2; ++i) {
        check(device->CreatePlacedResource(heap.Get(), i * alignment, &desc, initial, nullptr,
                                          IID_PPV_ARGS(&blocks[i])), "place GPU buffer");
        ComPtr<ID3D12Device> parent;
        check(blocks[i]->GetDevice(IID_PPV_ARGS(&parent)), "placed buffer parent");
        if (parent.Get() != device.Get()) throw std::runtime_error("placed buffer has the wrong device");
    }

    std::array<UINT, 2 * words> expected;
    for (UINT i = 0; i < expected.size(); ++i) expected[i] = i * 131071u + 17;
    auto upload = compute_fixture::buffer(device.Get(), D3D12_HEAP_TYPE_UPLOAD, sizeof(expected),
                                          D3D12_RESOURCE_STATE_GENERIC_READ);
    auto readback = compute_fixture::buffer(device.Get(), D3D12_HEAP_TYPE_READBACK, sizeof(expected),
                                            D3D12_RESOURCE_STATE_COPY_DEST);
    D3D12_RESOURCE_DESC scanout{};
    scanout.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
    scanout.Width = 3840;
    scanout.Height = 2160;
    scanout.DepthOrArraySize = 1;
    scanout.MipLevels = 1;
    scanout.SampleDesc.Count = 1;
    scanout.Format = DXGI_FORMAT_R10G10B10A2_UNORM;
    scanout.Flags = D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET;
    ComPtr<ID3D12Resource> back;
    check(device->CreateCommittedResource(&layout.Properties, D3D12_HEAP_FLAG_ALLOW_DISPLAY, &scanout,
                                          D3D12_RESOURCE_STATE_PRESENT, nullptr, IID_PPV_ARGS(&back)),
          "create 10-bit display surface");
    D3D12_HEAP_PROPERTIES props{};
    D3D12_HEAP_FLAGS flags{};
    check(back->GetHeapProperties(&props, &flags), "query display heap properties");
    const auto shape = back->GetDesc();
    if (shape.Width != scanout.Width || shape.Height != scanout.Height || shape.Format != scanout.Format ||
        props.Type != D3D12_HEAP_TYPE_DEFAULT || (flags & D3D12_HEAP_FLAG_ALLOW_DISPLAY) == 0)
        throw std::runtime_error("display surface lost dimensions, format or heap properties");
    D3D12_DESCRIPTOR_HEAP_DESC view{D3D12_DESCRIPTOR_HEAP_TYPE_RTV, 1, D3D12_DESCRIPTOR_HEAP_FLAG_NONE, 0};
    ComPtr<ID3D12DescriptorHeap> views;
    check(device->CreateDescriptorHeap(&view, IID_PPV_ARGS(&views)), "display descriptor heap");
    ComPtr<ID3D12Device5> residency;
    check(device.As(&residency), "Xbox Device5 for residency priority");
    using SetPriority = HRESULT (WINAPI*)(ID3D12Device5*, UINT, ID3D12Pageable* const*,
                                          const D3D12_RESIDENCY_PRIORITY*);
    const auto set_priority = method<SetPriority>(residency.Get(), 85);
    D3D12_QUERY_HEAP_DESC query_desc{D3D12_QUERY_HEAP_TYPE_TIMESTAMP, 1, 0};
    ComPtr<ID3D12QueryHeap> queries;
    check(device->CreateQueryHeap(&query_desc, IID_PPV_ARGS(&queries)), "residency query heap");
    ID3D12Pageable* pageables[]{heap.Get(), back.Get(), views.Get(), queries.Get()};
    const D3D12_RESIDENCY_PRIORITY priorities[]{D3D12_RESIDENCY_PRIORITY_HIGH,
        D3D12_RESIDENCY_PRIORITY_NORMAL, D3D12_RESIDENCY_PRIORITY_HIGH, D3D12_RESIDENCY_PRIORITY_LOW};
    check(set_priority(residency.Get(), 4, pageables, priorities), "four pageable priorities");
    ComPtr<ID3D12Device5> other_residency;
    check(second.As(&other_residency), "second Xbox Device5");
    ID3D12Pageable* null_pageable = nullptr;
    if (set_priority(residency.Get(), 1, &null_pageable, priorities) != E_INVALIDARG ||
        set_priority(other_residency.Get(), 1, pageables, priorities) != E_INVALIDARG ||
        set_priority(residency.Get(), 1, nullptr, priorities) != E_INVALIDARG ||
        set_priority(residency.Get(), 1, pageables, nullptr) != E_INVALIDARG)
        throw std::runtime_error("residency priority accepted an invalid argument");

    const D3D12_CPU_DESCRIPTOR_HANDLE rtv{method<CpuHandle>(views.Get(), 9)(views.Get())};
    device->CreateRenderTargetView(back.Get(), nullptr, rtv);
    auto pixel = compute_fixture::buffer(device.Get(), D3D12_HEAP_TYPE_READBACK, 256, D3D12_RESOURCE_STATE_COPY_DEST);
    compute_fixture::upload(upload.Get(), expected.data(), sizeof(expected));
    D3D12_COMMAND_QUEUE_DESC queue_desc{};
    ComPtr<ID3D12CommandQueue> queue;
    ComPtr<ID3D12CommandAllocator> allocator;
    ComPtr<ID3D12GraphicsCommandList> list;
    check(device->CreateCommandQueue(&queue_desc, IID_PPV_ARGS(&queue)), "create queue");
    check(device->CreateCommandAllocator(queue_desc.Type, IID_PPV_ARGS(&allocator)), "create allocator");
    const UINT list_desc[] = {D3D12_COMMAND_LIST_TYPE_DIRECT, 0, 1, 0};
    check(method<CreateList>(device.Get(), 73)(device.Get(), list_desc, allocator.Get(), nullptr, IID_PPV_ARGS(&list)),
          "create list");
    VolumeCopy volume(device.Get(), layout.Properties, list.Get());
    ComPtr<ID3D12Resource> alias;
    check(device->CreatePlacedResource(heap.Get(), 0, &desc, D3D12_RESOURCE_STATE_COPY_DEST, nullptr,
                                       IID_PPV_ARGS(&alias)), "place alias at first buffer offset");
    auto alias_read = compute_fixture::buffer(device.Get(), D3D12_HEAP_TYPE_READBACK, sizeof(UINT) * words,
                                              D3D12_RESOURCE_STATE_COPY_DEST);
    D3D12_RESOURCE_BARRIER activate{};
    activate.Type = D3D12_RESOURCE_BARRIER_TYPE_ALIASING;
    activate.Aliasing.pResourceAfter = blocks[0].Get();
    list->ResourceBarrier(1, &activate);
    for (UINT i = 0; i < 2; ++i) {
        compute_fixture::transition(list.Get(), blocks[i].Get(), initial, D3D12_RESOURCE_STATE_COPY_DEST);
        list->CopyBufferRegion(blocks[i].Get(), 0, upload.Get(), desc.Width * i, desc.Width);
        compute_fixture::transition(list.Get(), blocks[i].Get(), D3D12_RESOURCE_STATE_COPY_DEST,
                                    D3D12_RESOURCE_STATE_COPY_SOURCE);
    }
    // Read in reverse order after both writes to expose overlapping placements.
    for (UINT i : {1u, 0u}) list->CopyBufferRegion(readback.Get(), desc.Width * i, blocks[i].Get(), 0, desc.Width);
    activate.Aliasing.pResourceBefore = blocks[0].Get();
    activate.Aliasing.pResourceAfter = alias.Get();
    list->ResourceBarrier(1, &activate);
    list->CopyBufferRegion(alias.Get(), 0, upload.Get(), sizeof(UINT) * words, sizeof(UINT) * words);
    compute_fixture::transition(list.Get(), alias.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE);
    list->CopyBufferRegion(alias_read.Get(), 0, alias.Get(), 0, sizeof(UINT) * words);
    compute_fixture::transition(list.Get(), back.Get(), D3D12_RESOURCE_STATE_PRESENT, D3D12_RESOURCE_STATE_RENDER_TARGET);
    constexpr float green[]{0, 1, 0, 1};
    method<Clear>(list.Get(), 52)(list.Get(), rtv, green, 0, nullptr);
    compute_fixture::transition(list.Get(), back.Get(), D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_COPY_SOURCE);
    D3D12_TEXTURE_COPY_LOCATION src{}, dst{};
    src.pResource = back.Get();
    src.Type = D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
    dst.pResource = pixel.Get();
    dst.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
    dst.PlacedFootprint.Footprint = {scanout.Format, 1, 1, 1, 256};
    const D3D12_BOX box{128, 64, 0, 129, 65, 1};
    method<Copy>(list.Get(), 83)(list.Get(), &dst, 0, 0, 0, &src, &box, 0);
    compute_fixture::transition(list.Get(), back.Get(), D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_PRESENT);
    check(list->Close(), "close placed buffer copies");
    ComPtr<ID3D12Fence> fence;
    check(device->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&fence)), "create fence");
    ID3D12CommandList* lists[] = {list.Get()};
    queue->ExecuteCommandLists(1, lists);
    check(queue->Signal(fence.Get(), 1), "signal after copies");
    compute_fixture::wait_for_fence(fence.Get(), 1);
    volume.verify();
    D3D12_RANGE read{0, sizeof(expected)}, empty{};
    void* mapped{};
    check(readback->Map(0, &read, &mapped), "read placed buffers");
    const bool matches = std::memcmp(mapped, expected.data(), sizeof(expected)) == 0;
    readback->Unmap(0, &empty);
    if (!matches) throw std::runtime_error("GPU readback differs from both placed buffer patterns");
    D3D12_RANGE alias_range{0, sizeof(UINT) * words};
    check(alias_read->Map(0, &alias_range, &mapped), "read reactivated heap interval");
    const bool alias_matches = std::memcmp(mapped, expected.data() + words, alias_range.End) == 0;
    alias_read->Unmap(0, &empty);
    if (!alias_matches) throw std::runtime_error("reactivated heap interval differs");
    D3D12_RANGE pixel_read{0, sizeof(UINT)};
    check(pixel->Map(0, &pixel_read, &mapped), "read display pixel");
    UINT packed = 0;
    std::memcpy(&packed, mapped, sizeof(packed));
    pixel->Unmap(0, &empty);
    if (packed != 0xc00ffc00u) throw std::runtime_error("incorrect R10G10B10A2 surface readback");
    char receipt[208];
    std::snprintf(receipt, sizeof(receipt), "d3d12-heaps PASS pci=%04x:%04x heap=32MiB offsets=0,65536 "
                  "words=256/256 invalid=4 display=3840x2160/RGB10A2 pixel=c00ffc00 volume=45/45 priorities=4/4 invalid_priority_calls=4",
                  selected.VendorId, selected.DeviceId);
    return receipt;
}
}

int wmain(int argc, wchar_t** argv) {
    return tuxbox_test::frame_main(argc, argv, "d3d12-heaps", false, run);
}
