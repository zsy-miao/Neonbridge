#pragma once
#include "surface.h"
#include <cstring>
#include <vector>
using frame_abi::check;
inline std::vector<BYTE> readback_texture(ID3D12Resource* tex, IMFD3D12SynchronizationObjectCommands* sync,
                                          UINT width, UINT height) {
    auto desc = tex->GetDesc();
    if (desc.Format != DXGI_FORMAT_NV12 || desc.Width != width || desc.Height != height)
        throw std::runtime_error("unexpected texture");
    ComPtr<ID3D12Device> d;
    check(tex->GetDevice(IID_PPV_ARGS(&d)), "render device");
    ComPtr<ID3D12CommandQueue> q;
    D3D12_COMMAND_QUEUE_DESC qdesc{};
    check(d->CreateCommandQueue(&qdesc, IID_PPV_ARGS(&q)), "consumer queue");
    if (sync)
        check(sync->EnqueueResourceReadyWait(q.Get()), "wait for decoder");
    D3D12_PLACED_SUBRESOURCE_FOOTPRINT footprint[2]{};
    UINT rows[2]{};
    UINT64 bytes[2]{}, capacity{};
    d->GetCopyableFootprints(&desc, 0, 2, 0, footprint, rows, bytes, &capacity);
    D3D12_HEAP_PROPERTIES hp{};
    hp.Type = D3D12_HEAP_TYPE_READBACK;
    hp.CreationNodeMask = hp.VisibleNodeMask = 1;
    D3D12_RESOURCE_DESC target{};
    target.Dimension = D3D12_RESOURCE_DIMENSION_BUFFER;
    target.Width = capacity;
    target.Height = 1;
    target.DepthOrArraySize = target.MipLevels = target.SampleDesc.Count = 1;
    target.Layout = D3D12_TEXTURE_LAYOUT_ROW_MAJOR;
    ComPtr<ID3D12Resource> rb;
    check(d->CreateCommittedResource(&hp, D3D12_HEAP_FLAG_NONE, &target, D3D12_RESOURCE_STATE_COPY_DEST, nullptr,
                                     IID_PPV_ARGS(&rb)),
          "create readback");
    ComPtr<ID3D12CommandAllocator> a;
    ComPtr<ID3D12GraphicsCommandList> l;
    check(d->CreateCommandAllocator(qdesc.Type, IID_PPV_ARGS(&a)), "create allocator");
    check(d->CreateCommandList(0, qdesc.Type, a.Get(), nullptr, IID_PPV_ARGS(&l)), "create list");
    for (UINT i = 0; i < 2; i++) {
        D3D12_TEXTURE_COPY_LOCATION dst{}, src{};
        src.pResource = tex;
        src.SubresourceIndex = i;
        dst.pResource = rb.Get();
        dst.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
        dst.PlacedFootprint = footprint[i];
        l->CopyTextureRegion(&dst, 0, 0, 0, &src, nullptr);
    }
    check(l->Close(), "close");
    ID3D12CommandList* ls[] = {l.Get()};
    q->ExecuteCommandLists(1, ls);
    if (sync)
        check(sync->EnqueueResourceRelease(q.Get()), "signal decoder after consumer");
    ComPtr<ID3D12Fence> fence;
    check(d->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&fence)), "readback fence");
    check(q->Signal(fence.Get(), 1), "signal");
    check(fence->SetEventOnCompletion(1, nullptr), "complete");
    void* mapped{};
    D3D12_RANGE range{0, static_cast<SIZE_T>(capacity)};
    check(rb->Map(0, &range, &mapped), "map");
    std::vector<BYTE> pixels;
    for (UINT i = 0; i < 2; i++)
        for (UINT y = 0; y < rows[i]; y++) {
            auto row =
                static_cast<BYTE*>(mapped) + footprint[i].Offset + UINT64(y) * footprint[i].Footprint.RowPitch;
            pixels.insert(pixels.end(), row, row + bytes[i]);
        }
    D3D12_RANGE none{};
    rb->Unmap(0, &none);
    return pixels;
}

inline std::vector<BYTE> readback(IMFSample* sample, UINT width, UINT height,
                                  ComPtr<ID3D12Resource>* retain = nullptr) {
    ComPtr<IMFMediaBuffer> buf;
    check(sample->GetBufferByIndex(0, &buf), "frame buffer");
    ComPtr<IMFDXGIBuffer> dxgi;
    check(buf.As(&dxgi), "DXGI interface");
    ComPtr<ID3D12Resource> tex;
    check(dxgi->GetResource(IID_PPV_ARGS(&tex)), "get D3D12 texture");
    UINT sub{};
    check(dxgi->GetSubresourceIndex(&sub), "get subresource");
    if (sub)
        throw std::runtime_error("unexpected array subresource");
    ComPtr<IMFD3D12SynchronizationObjectCommands> sync;
    check(dxgi->GetUnknown(sync_key, IID_PPV_ARGS(&sync)), "sample synchronization");
    if (retain)
        *retain = tex;
    return readback_texture(tex.Get(), sync.Get(), width, height);
}
