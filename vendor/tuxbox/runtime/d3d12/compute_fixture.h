// Shared resources for GPU-verified Xbox compute guests.
#pragma once
#include "frame_abi.h"
#include <cstring>

namespace compute_fixture {
using frame_abi::ComPtr;
using frame_abi::check;

inline ComPtr<ID3D12Resource> buffer(ID3D12Device* device, D3D12_HEAP_TYPE type, UINT64 bytes,
                              D3D12_RESOURCE_STATES state, D3D12_RESOURCE_FLAGS flags = D3D12_RESOURCE_FLAG_NONE) {
    D3D12_HEAP_PROPERTIES heap{};
    heap.Type = type;
    heap.CreationNodeMask = heap.VisibleNodeMask = 1;
    D3D12_RESOURCE_DESC desc{};
    desc.Dimension = D3D12_RESOURCE_DIMENSION_BUFFER;
    desc.Width = bytes;
    desc.Height = 1;
    desc.DepthOrArraySize = 1;
    desc.MipLevels = 1;
    desc.SampleDesc.Count = 1;
    desc.Layout = D3D12_TEXTURE_LAYOUT_ROW_MAJOR;
    desc.Flags = flags;
    ComPtr<ID3D12Resource> resource;
    check(device->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, state, nullptr,
                                          IID_PPV_ARGS(&resource)), "create buffer");
    return resource;
}

inline void upload(ID3D12Resource* resource, const void* bytes, size_t size) {
    D3D12_RANGE empty{}, written{0, size};
    void* mapped = nullptr;
    check(resource->Map(0, &empty, &mapped), "map upload");
    std::memcpy(mapped, bytes, size);
    resource->Unmap(0, &written);
}

inline void transition(ID3D12GraphicsCommandList* list, ID3D12Resource* resource,
                D3D12_RESOURCE_STATES before, D3D12_RESOURCE_STATES after) {
    D3D12_RESOURCE_BARRIER barrier{};
    barrier.Transition = {resource, D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES, before, after};
    list->ResourceBarrier(1, &barrier);
}

inline void wait_for_fence(ID3D12Fence* fence, UINT64 value) {
    HANDLE event = CreateEventW(nullptr, FALSE, FALSE, nullptr);
    if (!event) throw std::runtime_error("create completion event");
    const auto hr = fence->SetEventOnCompletion(value, event);
    const auto waited = SUCCEEDED(hr) ? WaitForSingleObject(event, 15000) : WAIT_FAILED;
    CloseHandle(event);
    check(hr, "arm completion event");
    if (waited != WAIT_OBJECT_0 || fence->GetCompletedValue() != value)
        throw std::runtime_error("GPU fence did not complete");
}
}
