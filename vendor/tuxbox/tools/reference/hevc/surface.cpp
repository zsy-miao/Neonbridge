#include "surface.h"
#include <cstring>

// The Xbox player requests the texture and synchronization commands from this buffer.
// Allocate an independent texture for each output. The GPU upload completes before a
// software buffer can be returned to the decoder; users of the sample own the texture.
class Surface final : public Classic<IMFMediaBuffer, IMFDXGIBuffer, IMFD3D12SynchronizationObjectCommands> {
  public:
    ComPtr<IMFMediaBuffer> cpu;
    ComPtr<ID3D12Resource> texture;
    ComPtr<ID3D12Fence> ready;
    ComPtr<IMFAttributes> attributes;
    STDMETHODIMP Lock(BYTE** p, DWORD* max, DWORD* size) override { return cpu->Lock(p, max, size); }
    STDMETHODIMP Unlock() override { return cpu->Unlock(); }
    STDMETHODIMP GetCurrentLength(DWORD* n) override { return cpu->GetCurrentLength(n); }
    STDMETHODIMP SetCurrentLength(DWORD n) override { return cpu->SetCurrentLength(n); }
    STDMETHODIMP GetMaxLength(DWORD* n) override { return cpu->GetMaxLength(n); }
    STDMETHODIMP GetResource(REFIID iid, void** out) override { return texture.CopyTo(iid, out); }
    STDMETHODIMP GetSubresourceIndex(UINT* sub) override {
        if (!sub)
            return E_POINTER;
        *sub = 0;
        return S_OK;
    }
    STDMETHODIMP GetUnknown(REFIID key, REFIID iid, void** out) override {
        if (key == sync_key)
            return QueryInterface(iid, out);
        return attributes->GetUnknown(key, iid, out);
    }
    STDMETHODIMP SetUnknown(REFIID key, IUnknown* obj) override {
        if (key == sync_key)
            return E_ACCESSDENIED;
        return obj ? attributes->SetUnknown(key, obj) : attributes->DeleteItem(key);
    }
    STDMETHODIMP EnqueueResourceReady(ID3D12CommandQueue* queue) override {
        return queue ? queue->Signal(ready.Get(), 1) : E_POINTER;
    }
    STDMETHODIMP EnqueueResourceReadyWait(ID3D12CommandQueue* queue) override {
        return queue ? queue->Wait(ready.Get(), 1) : E_POINTER;
    }
    STDMETHODIMP SignalEventOnResourceReady(HANDLE event) override {
        return ready->SetEventOnCompletion(1, event);
    }
    STDMETHODIMP EnqueueResourceRelease(ID3D12CommandQueue* queue) override {
        // D3D12 command lists do not retain their referenced resources. Keep the
        // texture alive through the last queued consumer. Waiting here also keeps
        // teardown correct if the application immediately releases its MF sample.
        if (!queue)
            return E_POINTER;
        ComPtr<ID3D12Device> owner;
        CHECK_MEDIA(texture->GetDevice(IID_PPV_ARGS(&owner)));
        ComPtr<ID3D12Fence> released;
        CHECK_MEDIA(owner->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&released)));
        CHECK_MEDIA(queue->Signal(released.Get(), 1));
        return released->SetEventOnCompletion(1, nullptr);
    }
};

HRESULT decode_queue(IMFDXGIDeviceManager* manager, ID3D12CommandQueue** out) {
    if (!manager || !out)
        return E_POINTER;
    *out = nullptr;
    HANDLE handle{};
    CHECK_MEDIA(manager->OpenDeviceHandle(&handle));
    ComPtr<ID3D11Device> media;
    HRESULT hr = manager->GetVideoService(handle, IID_PPV_ARGS(&media));
    manager->CloseDeviceHandle(handle);
    CHECK_MEDIA(hr);
    ComPtr<IUnknown> attached;
    UINT size = sizeof(void*);
    CHECK_MEDIA(media->GetPrivateData(queue_lifetime, &size, attached.GetAddressOf()));
    ComPtr<ID3D12CommandQueue> associated;
    CHECK_MEDIA(attached.As(&associated));
    ComPtr<ID3D12Device> owner;
    CHECK_MEDIA(associated->GetDevice(IID_PPV_ARGS(&owner)));
    D3D12_COMMAND_QUEUE_DESC desc{};
    return owner->CreateCommandQueue(&desc, IID_PPV_ARGS(out));
}

void Nv12TextureBatch::reset() {
    const std::lock_guard<std::mutex> lock(mutex);
    heap.Reset();
    owner.Reset();
}

// Small frames share a heap, with a fresh range for every texture. A native
// texture can outlive its media sample, so it retains its heap directly.
HRESULT Nv12TextureBatch::create(ID3D12Device* device, const D3D12_HEAP_PROPERTIES& properties,
                                 const D3D12_RESOURCE_DESC& desc, ComPtr<ID3D12Resource>& texture) {
    constexpr UINT64 max_batch_bytes = 8 * 1024 * 1024;
    constexpr UINT64 frames_per_heap = 8;
    const auto info = device->GetResourceAllocationInfo(0, 1, &desc);
    if (info.Alignment == D3D12_DEFAULT_RESOURCE_PLACEMENT_ALIGNMENT && info.SizeInBytes &&
        info.SizeInBytes <= max_batch_bytes / frames_per_heap && !(info.SizeInBytes % info.Alignment)) {
        const std::lock_guard<std::mutex> lock(mutex);
        if (owner.Get() != device || !heap || info.SizeInBytes > remaining) {
            D3D12_HEAP_DESC hd{};
            hd.SizeInBytes = frames_per_heap * info.SizeInBytes;
            hd.Properties = properties;
            hd.Flags = D3D12_HEAP_FLAG_ALLOW_ONLY_NON_RT_DS_TEXTURES;
            heap.Reset();
            owner.Reset();
            if (SUCCEEDED(device->CreateHeap(&hd, IID_PPV_ARGS(&heap)))) {
                owner = device;
                offset = 0;
                remaining = hd.SizeInBytes;
            }
        }
        if (heap && SUCCEEDED(device->CreatePlacedResource(heap.Get(), offset, &desc, D3D12_RESOURCE_STATE_COPY_DEST,
                                                           nullptr, IID_PPV_ARGS(&texture)))) {
            constexpr GUID frame_heap{0xe92a78a1, 0xf10c, 0x452f, {0xb7, 0xe6, 0x3b, 0x6a, 0xfa, 0xc1, 0x18, 0x1f}};
            CHECK_MEDIA(texture->SetPrivateDataInterface(frame_heap, heap.Get()));
            offset += info.SizeInBytes;
            remaining -= info.SizeInBytes;
            return S_OK;
        }
    }
    return device->CreateCommittedResource(&properties, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COPY_DEST,
                                           nullptr, IID_PPV_ARGS(&texture));
}

HRESULT upload_nv12(Nv12TextureBatch& textures, ID3D12CommandQueue* queue, UINT width, UINT height, IMFSample* frame) {
    if (!width || !height || width % 2 || height % 2)
        return MF_E_INVALIDMEDIATYPE;
    ComPtr<ID3D12Device> device;
    CHECK_MEDIA(queue->GetDevice(IID_PPV_ARGS(&device)));
    auto surface = Make<Surface>();
    if (!surface)
        return E_OUTOFMEMORY;
    CHECK_MEDIA(MFCreateAttributes(&surface->attributes, 1));
    CHECK_MEDIA(frame->ConvertToContiguousBuffer(&surface->cpu));
    D3D12_HEAP_PROPERTIES hp{};
    hp.Type = D3D12_HEAP_TYPE_DEFAULT;
    hp.CreationNodeMask = hp.VisibleNodeMask = 1;
    D3D12_RESOURCE_DESC desc{};
    desc.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
    desc.Width = width;
    desc.Height = height;
    desc.Format = DXGI_FORMAT_NV12;
    desc.DepthOrArraySize = desc.MipLevels = desc.SampleDesc.Count = 1;
    CHECK_MEDIA(textures.create(device.Get(), hp, desc, surface->texture));
    D3D12_PLACED_SUBRESOURCE_FOOTPRINT footprints[2]{};
    UINT rows[2]{};
    UINT64 rowBytes[2]{}, capacity{};
    device->GetCopyableFootprints(&desc, 0, 2, 0, footprints, rows, rowBytes, &capacity);
    if (rows[0] != height || rows[1] != height / 2 || rowBytes[0] != width || rowBytes[1] != width)
        return E_UNEXPECTED;
    hp.Type = D3D12_HEAP_TYPE_UPLOAD;
    desc.Dimension = D3D12_RESOURCE_DIMENSION_BUFFER;
    desc.Format = DXGI_FORMAT_UNKNOWN;
    desc.Width = capacity;
    desc.Height = 1;
    desc.Layout = D3D12_TEXTURE_LAYOUT_ROW_MAJOR;
    ComPtr<ID3D12Resource> staging;
    CHECK_MEDIA(device->CreateCommittedResource(
        &hp, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_GENERIC_READ, nullptr, IID_PPV_ARGS(&staging)));
    BYTE* cpu{};
    DWORD length{};
    CHECK_MEDIA(surface->cpu->Lock(&cpu, nullptr, &length));
    void* mapped{};
    D3D12_RANGE empty{};
    HRESULT hr = staging->Map(0, &empty, &mapped);
    if (SUCCEEDED(hr)) {
        if (UINT64(width) * height * 3 / 2 != length)
            hr = MF_E_INVALIDMEDIATYPE;
        else
            for (UINT plane = 0; plane < 2; plane++)
                for (UINT y = 0; y < rows[plane]; y++) {
                    std::memcpy(static_cast<BYTE*>(mapped) + footprints[plane].Offset +
                                    UINT64(y) * footprints[plane].Footprint.RowPitch,
                                cpu, width);
                    cpu += width;
                }
        staging->Unmap(0, nullptr);
    }
    surface->cpu->Unlock();
    CHECK_MEDIA(hr);
    ComPtr<ID3D12CommandAllocator> alloc;
    ComPtr<ID3D12GraphicsCommandList> list;
    CHECK_MEDIA(device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&alloc)));
    CHECK_MEDIA(
        device->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, alloc.Get(), nullptr, IID_PPV_ARGS(&list)));
    for (UINT plane = 0; plane < 2; plane++) {
        D3D12_TEXTURE_COPY_LOCATION dst{}, src{};
        dst.pResource = surface->texture.Get();
        dst.SubresourceIndex = plane;
        src.pResource = staging.Get();
        src.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
        src.PlacedFootprint = footprints[plane];
        list->CopyTextureRegion(&dst, 0, 0, 0, &src, nullptr);
    }
    D3D12_RESOURCE_BARRIER barrier{};
    barrier.Transition = {surface->texture.Get(), D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                          D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COMMON};
    list->ResourceBarrier(1, &barrier);
    CHECK_MEDIA(list->Close());
    CHECK_MEDIA(device->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&surface->ready)));
    ID3D12CommandList* lists[] = {list.Get()};
    queue->ExecuteCommandLists(1, lists);
    CHECK_MEDIA(surface->EnqueueResourceReady(queue));
    CHECK_MEDIA(surface->ready->SetEventOnCompletion(1, nullptr));
    CHECK_MEDIA(frame->RemoveAllBuffers());
    return frame->AddBuffer(surface.Get());
}
