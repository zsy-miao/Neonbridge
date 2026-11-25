// Verify the observed D3D12.x whole-resource copy through the Xbox device and command list.
#include "compute_fixture.h"
#include "frame_cli.h"
#include <algorithm>
#include <future>
#include <chrono>
#include <vector>

namespace {
using namespace frame_abi;
using namespace compute_fixture;
using CopyResourceX = void (WINAPI*)(ID3D12GraphicsCommandList*, ID3D12Resource*, ID3D12Resource*, UINT);
constexpr UINT bufferBytes = 1028, bufferOffset = 64, textureBase = 1536;
constexpr UINT mipCount = 4, layers = 2, subresources = mipCount * layers;
constexpr BYTE guard = 0xa7;
using Place = HRESULT (WINAPI*)(ID3D12Device*, UINT64, const D3D12_RESOURCE_DESC*, D3D12_RESOURCE_STATES,
                                const D3D12_CLEAR_VALUE*, REFIID, void**);

struct TiledBacking {
    // All eight small RGBA8 subresources fit in a 64 KiB reservation.
    static constexpr size_t size = 65536;
    void* address = VirtualAlloc(nullptr, size, MEM_RESERVE, PAGE_READWRITE);
    ~TiledBacking() { if (address) VirtualFree(address, 0, MEM_RELEASE); }
};
constexpr std::array<UINT, 18> releaseProfile{0x20004, 0, ~0u, 0, 0, 0, ~0u, ~0u, 0, 1};

struct Layout {
    std::array<D3D12_PLACED_SUBRESOURCE_FOOTPRINT, subresources> tiles{};
    std::array<UINT, subresources> rows{};
    std::array<UINT64, subresources> bytesPerRow{};
    size_t length;
    D3D12_RESOURCE_DESC desc{};

    explicit Layout(ID3D12Device* device) {
        desc.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
        desc.Width = 19;
        desc.Height = 13;
        desc.MipLevels = mipCount;
        desc.DepthOrArraySize = layers;
        desc.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
        desc.SampleDesc.Count = 1;
        UINT64 bytes{};
        device->GetCopyableFootprints(&desc, 0, subresources, 0, tiles.data(), rows.data(), bytesPerRow.data(), &bytes);
        if (bytes > 1024 * 1024) throw std::runtime_error("unexpected texture footprint");
        length = static_cast<size_t>(textureBase + bytes + 512);
        for (UINT sub = 0; sub < subresources; ++sub) {
            tiles[sub].Offset += textureBase;
            if (rows[sub] != std::max(1u, desc.Height >> (sub % mipCount)) ||
                bytesPerRow[sub] != 4 * std::max<UINT64>(1, desc.Width >> (sub % mipCount)) ||
                tiles[sub].Offset + (rows[sub] - 1) * tiles[sub].Footprint.RowPitch + bytesPerRow[sub] > length - 512)
                throw std::runtime_error("invalid subresource dimensions");
        }
    }

    template <class Visit>
    void visit(Visit row) const {
        row(bufferOffset, bufferBytes, 0u);
        for (UINT sub = 0; sub < subresources; ++sub)
            for (UINT y = 0; y < rows[sub]; ++y)
                row(tiles[sub].Offset + y * tiles[sub].Footprint.RowPitch, bytesPerRow[sub], 1 + sub * 37 + y);
    }
};

struct Resources {
    ComPtr<ID3D12Resource> data, texture, staging, readback;
    Resources(ID3D12Device* device, const Layout& layout, const std::vector<BYTE>& bytes) {
        data = buffer(device, D3D12_HEAP_TYPE_DEFAULT, bufferBytes, D3D12_RESOURCE_STATE_COPY_DEST);
        D3D12_HEAP_PROPERTIES heap{};
        heap.Type = D3D12_HEAP_TYPE_DEFAULT;
        heap.CreationNodeMask = heap.VisibleNodeMask = 1;
        check(device->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &layout.desc, D3D12_RESOURCE_STATE_COPY_DEST,
                                              nullptr, IID_PPV_ARGS(&texture)), "create texture");
        staging = buffer(device, D3D12_HEAP_TYPE_UPLOAD, bytes.size(), D3D12_RESOURCE_STATE_GENERIC_READ);
        readback = buffer(device, D3D12_HEAP_TYPE_READBACK, bytes.size(), D3D12_RESOURCE_STATE_COPY_DEST);
        upload(staging.Get(), bytes.data(), bytes.size());
    }

    void transfer(ID3D12GraphicsCommandList* list, const Layout& layout, bool read,
                  ID3D12Resource* imageOverride = nullptr) const {
        auto linear = read ? readback.Get() : staging.Get();
        if (read) list->CopyBufferRegion(linear, bufferOffset, data.Get(), 0, bufferBytes);
        else list->CopyBufferRegion(data.Get(), 0, linear, bufferOffset, bufferBytes);
        for (UINT sub = 0; sub < subresources; ++sub) {
            D3D12_TEXTURE_COPY_LOCATION image{}, footprint{};
            image.pResource = imageOverride ? imageOverride : texture.Get();
            image.Type = D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
            image.SubresourceIndex = sub;
            footprint.pResource = linear;
            footprint.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
            footprint.PlacedFootprint = layout.tiles[sub];
            method<Copy>(list, 83)(list, read ? &footprint : &image, 0, 0, 0, read ? &image : &footprint, nullptr, 0);
        }
    }

    void make_copy_source(ID3D12GraphicsCommandList* list) const {
        for (auto resource : {data.Get(), texture.Get()})
            transition(list, resource, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE);
    }

    void verify(const Layout& layout, const std::vector<BYTE>& expected) const {
        D3D12_RANGE range{0, layout.length}, empty{};
        void* mapping{};
        check(readback->Map(0, &range, &mapping), "map readback");
        auto bytes = static_cast<const BYTE*>(mapping);
        bool matches = !std::memcmp(bytes, expected.data(), bufferOffset) &&
                       !std::memcmp(bytes + layout.length - 512, expected.data() + layout.length - 512, 512);
        layout.visit([&](size_t at, size_t size, UINT) {
            if (std::memcmp(bytes + at, expected.data() + at, size)) matches = false;
        });
        readback->Unmap(0, &empty);
        if (!matches) throw std::runtime_error("GPU readback differs from the expected resource or guard bytes");
    }
};

std::string run(const wchar_t* requested, bool) {
    DXGI_ADAPTER_DESC1 adapter{};
    auto device = create_device(requested, adapter, releaseProfile.data());
    Layout layout(device.Get());
    std::vector<BYTE> expected(layout.length, guard), poison(layout.length, guard);
    layout.visit([&](size_t at, size_t size, UINT seed) {
        for (size_t i = 0; i < size; ++i) {
            expected[at + i] = static_cast<BYTE>(seed * 73 + i * 29 + (i >> 5));
            poison[at + i] = expected[at + i] ^ 0xff;
        }
    });
    Resources src(device.Get(), layout, expected), dst(device.Get(), layout, poison);
    ComPtr<ID3D12CommandQueue> queue;
    D3D12_COMMAND_QUEUE_DESC queueDesc{};
    check(device->CreateCommandQueue(&queueDesc, IID_PPV_ARGS(&queue)), "create Xbox queue");
    ComPtr<ID3D12CommandAllocator> allocator;
    ComPtr<ID3D12GraphicsCommandList> list;
    check(device->CreateCommandAllocator(queueDesc.Type, IID_PPV_ARGS(&allocator)), "create Xbox allocator");
    const UINT listDesc[] = {D3D12_COMMAND_LIST_TYPE_DIRECT, 0, 1, 0};
    check(method<CreateList>(device.Get(), 73)(device.Get(), listDesc, allocator.Get(), nullptr, IID_PPV_ARGS(&list)),
          "create Xbox command list");
    TiledBacking backing;
    if (!backing.address) throw std::runtime_error("reserve tiled source");
    auto tileDesc = layout.desc;
    tileDesc.Layout = static_cast<D3D12_TEXTURE_LAYOUT>(0x105);
    ComPtr<ID3D12Resource> placed;
    check(method<Place>(device.Get(), 44)(device.Get(), reinterpret_cast<UINT64>(backing.address), &tileDesc,
                                          D3D12_RESOURCE_STATE_COPY_SOURCE, nullptr, IID_PPV_ARGS(&placed)),
          "create guest-backed tiled source");
    auto copy = method<CopyResourceX>(list.Get(), 84);
    copy(list.Get(), dst.data.Get(), src.data.Get(), 1);
    if (list->Close() != E_NOTIMPL) throw std::runtime_error("unexpected Close result for nonzero option");
    check(list->Reset(allocator.Get(), nullptr), "reset after unsupported option");
    copy(list.Get(), src.data.Get(), src.data.Get(), 0);
    if (list->Close() != E_INVALIDARG) throw std::runtime_error("unexpected Close result for self-copy");
    check(list->Reset(allocator.Get(), nullptr), "reset after self-copy");
    for (const auto* resources : {&src, &dst}) {
        list->CopyBufferRegion(resources->readback.Get(), 0, dst.staging.Get(), 0, layout.length);
        resources->transfer(list.Get(), layout, false);
    }
    src.make_copy_source(list.Get());
    copy(list.Get(), dst.data.Get(), src.data.Get(), 0);
    copy(list.Get(), dst.texture.Get(), src.texture.Get(), 0);
    dst.make_copy_source(list.Get());
    for (const auto* resources : {&src, &dst}) resources->transfer(list.Get(), layout, true);
    check(list->Close(), "close copy command list");
    ID3D12CommandList* batch[] = {list.Get()};
    ComPtr<ID3D12Fence> fence;
    check(device->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&fence)), "create fence");
    UINT64 completion = 0;
    auto submit = [&] {
        queue->ExecuteCommandLists(1, batch);
        check(queue->Signal(fence.Get(), ++completion), "signal queue");
        wait_for_fence(fence.Get(), completion);
    };
    submit();
    src.verify(layout, expected);
    dst.verify(layout, expected);
    auto reset = [&] {
        check(allocator->Reset(), "reset completed allocator");
        check(list->Reset(allocator.Get(), nullptr), "reset command list");
    };
    auto readPlaced = [&](ID3D12Resource* image) {
        transition(list.Get(), dst.texture.Get(), D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_COPY_DEST);
        copy(list.Get(), dst.texture.Get(), image, 0);
        transition(list.Get(), dst.texture.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE);
        dst.transfer(list.Get(), layout, true);
    };
    auto filled = expected;
    for (BYTE fill : {BYTE{0x27}, BYTE{0xb8}}) {
        reset();
        readPlaced(placed.Get());
        check(list->Close(), "close tiled source copy");
        // Commit and populate the title's CPU backing after the list has closed.
        if (!VirtualAlloc(backing.address, backing.size, MEM_COMMIT, PAGE_READWRITE))
            throw std::runtime_error("commit tiled source pages");
        std::memset(backing.address, fill, backing.size);
        submit();
        layout.visit([&](size_t at, size_t size, UINT seed) {
            if (seed) std::memset(filled.data() + at, fill, size);
        });
        dst.verify(layout, filled);
        if (!VirtualFree(backing.address, backing.size, MEM_DECOMMIT)) throw std::runtime_error("decommit tiled source");
    }
    if (!VirtualAlloc(backing.address, backing.size, MEM_COMMIT, PAGE_READWRITE))
        throw std::runtime_error("commit writable guest backing");
    constexpr BYTE initial = 0x51, red = 0xe3;
    std::memset(backing.address, initial, backing.size);
    ComPtr<ID3D12Resource> alias;
    check(method<Place>(device.Get(), 44)(device.Get(), reinterpret_cast<UINT64>(backing.address), &tileDesc,
                                          D3D12_RESOURCE_STATE_COPY_SOURCE, nullptr, IID_PPV_ARGS(&alias)),
          "create second view of the same guest texture");
    const auto shaderRead = D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE | D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE;
    reset();
    transition(list.Get(), placed.Get(), D3D12_RESOURCE_STATE_COPY_SOURCE, shaderRead);
    transition(list.Get(), placed.Get(), shaderRead, D3D12_RESOURCE_STATE_COPY_DEST);
    copy(list.Get(), placed.Get(), src.texture.Get(), 0);
    transition(list.Get(), placed.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE);
    readPlaced(alias.Get());
    check(list->Close(), "close whole-resource write and alias read");

    // Hold the write on one queue. The second queue must be able to submit an
    // alias read before the CPU opens the gate, without uploading stale pages.
    ComPtr<ID3D12CommandQueue> readerQueue;
    ComPtr<ID3D12CommandAllocator> readerAllocator;
    ComPtr<ID3D12GraphicsCommandList> readerList;
    ComPtr<ID3D12Fence> gate, writerDone, readerDone;
    check(device->CreateCommandQueue(&queueDesc, IID_PPV_ARGS(&readerQueue)), "create second queue");
    check(device->CreateCommandAllocator(queueDesc.Type, IID_PPV_ARGS(&readerAllocator)), "create second allocator");
    check(method<CreateList>(device.Get(), 73)(device.Get(), listDesc, readerAllocator.Get(), nullptr,
                                              IID_PPV_ARGS(&readerList)), "create second command list");
    check(device->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&gate)), "create blocked-write gate");
    check(device->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&writerDone)), "create writer fence");
    check(device->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&readerDone)), "create reader fence");
    // Start with complementary bytes in this readback so an empty reader
    // command list cannot accidentally reuse a previous successful result.
    readerList->CopyBufferRegion(src.readback.Get(), 0, dst.staging.Get(), 0, layout.length);
    src.transfer(readerList.Get(), layout, true, alias.Get());
    check(readerList->Close(), "close pending alias read");
    HANDLE submitted = CreateEventW(nullptr, TRUE, FALSE, nullptr);
    if (!submitted) throw std::runtime_error("create submission handshake");
    auto release = std::async(std::launch::async, [&] {
        // The timeout releases a stalled driver before reporting a failure.
        const auto wait = WaitForSingleObject(submitted, 8000);
        check(gate->Signal(1), "open blocked-write gate");
        return wait == WAIT_OBJECT_0;
    });
    check(queue->Wait(gate.Get(), 1), "hold writer queue");
    queue->ExecuteCommandLists(1, batch);
    check(queue->Signal(writerDone.Get(), 1), "signal pending writer");
    check(readerQueue->Wait(writerDone.Get(), 1), "order alias read after pending writer");
    ID3D12CommandList* pendingBatch[] = {readerList.Get()};
    readerQueue->ExecuteCommandLists(1, pendingBatch);
    check(readerQueue->Signal(readerDone.Get(), 1), "signal pending reader");
    const bool pending = readerDone->GetCompletedValue() == 0;
    SetEvent(submitted);
    const bool timely = release.get();
    CloseHandle(submitted);
    wait_for_fence(readerDone.Get(), 1);
    if (!pending || !timely) throw std::runtime_error("pending alias read ran early or blocked submission");
    dst.verify(layout, expected);
    src.verify(layout, expected);

    // Independent retail XG texture address-computer offsets for all 630 texels
    // of this descriptor give a fingerprint of the complete tiled allocation,
    // including its padding. See the copy-resource evidence for the XG DLL hash.
    const auto verifyBacking = [&](UINT64 reference) {
        UINT64 hash = 14695981039346656037ull;
        for (size_t i = 0; i < backing.size; ++i)
            hash = (hash ^ static_cast<const BYTE*>(backing.address)[i]) * 1099511628211ull;
        if (hash != reference) throw std::runtime_error("CPU tiled data or its padding differs after the guest fence");
    };
    verifyBacking(0x6df8e35207948235ull);

    // A new command list reads the same virtual allocation through a second
    // guest handle. A partial CPU write later changes red in every RGBA texel.
    reset();
    readPlaced(alias.Get());
    check(list->Close(), "close aliased texture read");
    submit();
    dst.verify(layout, expected);
    for (size_t i = 0; i < backing.size; i += 4) static_cast<BYTE*>(backing.address)[i] = red;
    layout.visit([&](size_t at, size_t size, UINT seed) {
        if (!seed) return;
        for (size_t i = 0; i < size; i += 4) expected[at + i] = red;
    });

    reset();
    readPlaced(alias.Get());
    check(list->Close(), "close texture read with red-channel CPU edits");
    submit();
    dst.verify(layout, expected);

    // Restore a 2 x 1 rectangle of slice 1, mip 1 from the original image.
    // Every G/B/A byte and every other CPU-written red byte must persist.
    constexpr UINT sub = mipCount + 1, x = 2, y = 1;
    D3D12_TEXTURE_COPY_LOCATION regionDst{}, regionSrc{};
    regionDst.pResource = placed.Get();
    regionSrc.pResource = src.texture.Get();
    regionDst.Type = regionSrc.Type = D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
    regionDst.SubresourceIndex = regionSrc.SubresourceIndex = sub;
    const D3D12_BOX box{x, y, 0, x + 2, y + 1, 1};
    reset();
    transition(list.Get(), placed.Get(), D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_COPY_DEST);
    method<Copy>(list.Get(), 83)(list.Get(), &regionDst, x, y, 0, &regionSrc, &box, 0);
    transition(list.Get(), placed.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE);
    readPlaced(placed.Get());
    check(list->Close(), "close partial texture write");
    submit();
    const size_t offset = layout.tiles[sub].Offset + y * layout.tiles[sub].Footprint.RowPitch + 4 * x;
    // Poison holds the complement of the original source at every visible byte.
    for (size_t i = 0; i < 8; ++i) expected[offset + i] = poison[offset + i] ^ 0xff;
    dst.verify(layout, expected);

    verifyBacking(0xd5c14aecb150fb89ull);

    // The reference layout puts slice 1 at byte 8192. A separate one-slice
    // handle at that VA overlaps just the latter half of the original texture.
    auto subDesc = tileDesc;
    subDesc.DepthOrArraySize = 1;
    ComPtr<ID3D12Resource> overlap;
    check(method<Place>(device.Get(), 44)(device.Get(), reinterpret_cast<UINT64>(backing.address) + 8192, &subDesc,
                                          D3D12_RESOURCE_STATE_COPY_SOURCE, nullptr, IID_PPV_ARGS(&overlap)),
          "create overlapping guest slice");
    reset();
    list->CopyBufferRegion(dst.readback.Get(), 0, dst.staging.Get(), 0, layout.length);
    auto sliceExpected = poison;
    for (UINT level = 0; level < mipCount; ++level) {
        const UINT subresource = mipCount + level;
        D3D12_TEXTURE_COPY_LOCATION image{}, linear{};
        image.pResource = overlap.Get();
        image.Type = D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
        image.SubresourceIndex = level;
        linear.pResource = dst.readback.Get();
        linear.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
        linear.PlacedFootprint = layout.tiles[subresource];
        method<Copy>(list.Get(), 83)(list.Get(), &linear, 0, 0, 0, &image, nullptr, 0);
        for (UINT row = 0; row < layout.rows[subresource]; ++row) {
            const size_t at = linear.PlacedFootprint.Offset + row * linear.PlacedFootprint.Footprint.RowPitch;
            std::memcpy(sliceExpected.data() + at, expected.data() + at, layout.bytesPerRow[subresource]);
        }
    }
    check(list->Close(), "close overlapping slice readback");
    submit();
    dst.verify(layout, sliceExpected);

    char result[320];
    std::snprintf(result, sizeof(result), "d3d12-copy-resource PASS pci=%04x:%04x slot=84 buffer_bytes=%u "
                  "texture_subresources=%u nonzero_option=E_NOTIMPL self_copy=E_INVALIDARG "
                  "source_and_destination=verified placed_uploads=2 placed_dst=verified same_va_alias=verified "
                  "cpu_writeback=verified partial_copy_texels=2 overlap_mips=4 pending_reader=verified",
                  adapter.VendorId, adapter.DeviceId, bufferBytes, subresources);
    return result;
}
// Blocked writer: defer overlapping image refresh until publication.
std::string run_alias_deferred(const wchar_t* requested, bool) {
    DXGI_ADAPTER_DESC1 adapter{};
    auto device=create_device(requested,adapter,releaseProfile.data());
    Layout layout(device.Get());
    constexpr BYTE old_value=0x51,new_value=0xe3;
    std::vector<BYTE> bytes(layout.length,new_value);
    Resources src(device.Get(),layout,bytes);
    TiledBacking backing;
    if(!backing.address||!VirtualAlloc(backing.address,backing.size,MEM_COMMIT,PAGE_READWRITE))
        throw std::runtime_error("alias deferred backing");
    std::memset(backing.address,old_value,backing.size);
    auto make=[&](UINT64 address,UINT16 slices,D3D12_RESOURCE_STATES initial) {
        auto desc=layout.desc;
        desc.Layout=static_cast<D3D12_TEXTURE_LAYOUT>(0x105);
        desc.DepthOrArraySize=slices;
        ComPtr<ID3D12Resource> out;
        check(method<Place>(device.Get(),44)(device.Get(),address,&desc,initial,nullptr,IID_PPV_ARGS(&out)),"place alias-budget texture");
        return out;
    };
    auto va=reinterpret_cast<UINT64>(backing.address);
    auto writer=make(va,2,D3D12_RESOURCE_STATE_COPY_DEST);
    auto first=make(va,1,D3D12_RESOURCE_STATE_COPY_SOURCE);
    auto second=make(va+8192,1,D3D12_RESOURCE_STATE_COPY_SOURCE);
    D3D12_COMMAND_QUEUE_DESC q{};
    ComPtr<ID3D12CommandQueue> wq,rq;
    check(device->CreateCommandQueue(&q,IID_PPV_ARGS(&wq)),"alias writer queue");
    check(device->CreateCommandQueue(&q,IID_PPV_ARGS(&rq)),"alias reader queue");
    ComPtr<ID3D12Fence> gate,wd,rd;
    for(auto* f:{gate.GetAddressOf(),wd.GetAddressOf(),rd.GetAddressOf()})check(device->CreateFence(0,D3D12_FENCE_FLAG_NONE,IID_PPV_ARGS(f)),"alias fence");
    auto readback=buffer(device.Get(),D3D12_HEAP_TYPE_READBACK,layout.length,D3D12_RESOURCE_STATE_COPY_DEST);
    std::array<ComPtr<ID3D12CommandAllocator>,4> allocs;
    std::array<ComPtr<ID3D12GraphicsCommandList>,4> lists;
    auto list=[&](UINT n){
        check(device->CreateCommandAllocator(q.Type,IID_PPV_ARGS(&allocs[n])),"alias allocator");
        const UINT desc[]{D3D12_COMMAND_LIST_TYPE_DIRECT,0,1,0};
        check(method<CreateList>(device.Get(),73)(device.Get(),desc,allocs[n].Get(),nullptr,IID_PPV_ARGS(&lists[n])),"alias list");
        return lists[n].Get();
    };
    auto read=[&](UINT n){
        auto l=list(n);
        for(UINT slice=0;slice<2;++slice)for(UINT mip=0;mip<mipCount;++mip){
            D3D12_TEXTURE_COPY_LOCATION from{},to{};
            from.pResource=slice?second.Get():first.Get();
            from.Type=D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;from.SubresourceIndex=mip;
            to.pResource=readback.Get();to.Type=D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
            to.PlacedFootprint=layout.tiles[slice*mipCount+mip];
            method<Copy>(l,83)(l,&to,0,0,0,&from,nullptr,0);
        }
        check(l->Close(),"close alias read");
        return l;
    };
    auto verify=[&](BYTE expected){
        D3D12_RANGE range{0,layout.length},empty{};void* ptr{};
        check(readback->Map(0,&range,&ptr),"alias deferred readback");
        bool equal=true;
        for(UINT sub=0;sub<subresources;++sub)for(UINT y=0;y<layout.rows[sub];++y)
            for(UINT64 x=0;x<layout.bytesPerRow[sub];++x)
                equal &= static_cast<const BYTE*>(ptr)[layout.tiles[sub].Offset+y*layout.tiles[sub].Footprint.RowPitch+x]==expected;
        readback->Unmap(0,&empty);
        if(!equal)throw std::runtime_error("alias deferred readback differs");
    };
    auto execute=[](ID3D12CommandQueue* queue,ID3D12GraphicsCommandList* l){
        ID3D12CommandList* batch[]{l};queue->ExecuteCommandLists(1,batch);
    };
    execute(rq.Get(),read(0));
    check(rq->Signal(rd.Get(),1),"initial alias signal");wait_for_fence(rd.Get(),1);
    verify(old_value);
    auto wl=list(1);
    src.transfer(wl,layout,false);
    src.make_copy_source(wl);
    method<CopyResourceX>(wl,84)(wl,writer.Get(),src.texture.Get(),0);
    check(wl->Close(),"close blocked writer");
    auto blocked=read(2);
    check(wq->Wait(gate.Get(),1),"block alias writer");
    execute(wq.Get(),wl);
    check(wq->Signal(wd.Get(),1),"signal blocked writer");
    // Release the gate only after submit returns. The current provider must
    // defer the refresh rather than wait for this CPU-dependent writer.
    const auto start=std::chrono::steady_clock::now();
    execute(rq.Get(),blocked);
    const auto elapsed=std::chrono::duration_cast<std::chrono::milliseconds>(std::chrono::steady_clock::now()-start).count();
    std::printf("alias deferred observed ms=%lld\n",static_cast<long long>(elapsed));
    std::fflush(stdout);
    check(gate->Signal(1),"release blocked writer");
    check(rq->Signal(rd.Get(),2),"signal degraded reader");
    wait_for_fence(wd.Get(),1);wait_for_fence(rd.Get(),2);
    verify(old_value);
    execute(rq.Get(),read(3));
    check(rq->Signal(rd.Get(),3),"signal refreshed reader");
    wait_for_fence(rd.Get(),3);
    verify(new_value);
    if(elapsed>1000)throw std::runtime_error("alias refresh unexpectedly blocked submission");
    char receipt[180];
    std::snprintf(receipt,sizeof(receipt),"d3d12-alias-deferred PASS pci=%04x:%04x overlaps=2 old=verified refreshed=verified no_cpu_wait=1",adapter.VendorId,adapter.DeviceId);
    return receipt;
}

}

int wmain(int argc, wchar_t** argv) {
    if(argc>1 && !std::wcscmp(argv[1],L"--alias-deferred"))
        return tuxbox_test::frame_main(argc-1,argv+1,"d3d12-alias-deferred",false,run_alias_deferred);
    return tuxbox_test::frame_main(argc, argv, "copy-resource", false, run);
}
