#pragma once
#include "frame_abi.h"
#include <cstring>

namespace compute_buffers {
using frame_abi::check;
using frame_abi::ComPtr;
static ComPtr<ID3D12Resource> buffer(ID3D12Device* device, UINT size, D3D12_HEAP_TYPE type, D3D12_RESOURCE_STATES state,
                                     D3D12_RESOURCE_FLAGS flags = D3D12_RESOURCE_FLAG_NONE) {
    D3D12_HEAP_PROPERTIES props{};
    props.Type = type;
    props.VisibleNodeMask = props.CreationNodeMask = 1;
    D3D12_RESOURCE_DESC desc{};
    desc.Dimension = D3D12_RESOURCE_DIMENSION_BUFFER;
    desc.Width = size;
    desc.Height = desc.MipLevels = desc.DepthOrArraySize = 1;
    desc.SampleDesc.Count = 1;
    desc.Layout = D3D12_TEXTURE_LAYOUT_ROW_MAJOR;
    desc.Flags = flags;
    ComPtr<ID3D12Resource> result;
    check(device->CreateCommittedResource(&props, D3D12_HEAP_FLAG_NONE, &desc, state, nullptr, IID_PPV_ARGS(&result)),
          "CreateCommittedResource");
    return result;
}

static void transition(ID3D12GraphicsCommandList* list, ID3D12Resource* buffer, D3D12_RESOURCE_STATES before,
                       D3D12_RESOURCE_STATES after) {
    D3D12_RESOURCE_BARRIER barrier{};
    barrier.Type = D3D12_RESOURCE_BARRIER_TYPE_TRANSITION;
    barrier.Transition = {buffer, D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES, before, after};
    list->ResourceBarrier(1, &barrier);
}

static void upload(ID3D12Resource* resource, const void* values, SIZE_T bytes) {
    void* mapped{};
    D3D12_RANGE empty{}, written{0, bytes};
    check(resource->Map(0, &empty, &mapped), "Map upload");
    std::memcpy(mapped, values, bytes);
    resource->Unmap(0, &written);
}

static void submit_and_wait(ID3D12Device* device, ID3D12CommandQueue* queue, ID3D12GraphicsCommandList* list) {
    check(list->Close(), "Close");
    ID3D12CommandList* lists[] = {list};
    ComPtr<ID3D12Fence> fence;
    check(device->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&fence)), "CreateFence");
    queue->ExecuteCommandLists(1, lists);
    check(queue->Signal(fence.Get(), 1), "Signal fence");
    HANDLE event = CreateEventW(nullptr, FALSE, FALSE, nullptr);
    if (!event)
        throw std::runtime_error("CreateEventW failed");
    HRESULT hr = fence->SetEventOnCompletion(1, event);
    DWORD wait = SUCCEEDED(hr) ? WaitForSingleObject(event, 15000) : WAIT_FAILED;
    CloseHandle(event);
    check(hr, "SetEventOnCompletion");
    if (wait != WAIT_OBJECT_0 || fence->GetCompletedValue() != 1)
        throw std::runtime_error("GPU fence failed");
}
} // namespace compute_buffers
