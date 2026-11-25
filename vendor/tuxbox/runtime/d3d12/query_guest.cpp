// The Xbox graphics list moves BeginQuery, EndQuery and ResolveQueryData to 28–30.
#include "frame_abi.h"
#include "frame_cli.h"
#include <algorithm>

namespace {
using namespace frame_abi;
using Record = void (WINAPI*)(ID3D12GraphicsCommandList*, ID3D12QueryHeap*, D3D12_QUERY_TYPE, UINT);
using Resolve = void (WINAPI*)(ID3D12GraphicsCommandList*, ID3D12QueryHeap*, D3D12_QUERY_TYPE,
                                UINT, UINT, ID3D12Resource*, UINT64);
using GpuHandle = UINT64 (WINAPI*)(ID3D12DescriptorHeap*);

void descriptor_heaps(ID3D12Device* device) {
    for (auto type : {D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER,
                      D3D12_DESCRIPTOR_HEAP_TYPE_RTV, D3D12_DESCRIPTOR_HEAP_TYPE_DSV}) {
        for (auto flags : {D3D12_DESCRIPTOR_HEAP_FLAG_NONE, D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE}) {
            D3D12_DESCRIPTOR_HEAP_DESC desc{type, 7, flags, 0};
            ComPtr<ID3D12DescriptorHeap> heap;
            HRESULT hr = device->CreateDescriptorHeap(&desc, IID_PPV_ARGS(&heap));
            bool visible = flags == D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE;
            if (visible && (type == D3D12_DESCRIPTOR_HEAP_TYPE_RTV || type == D3D12_DESCRIPTOR_HEAP_TYPE_DSV)) {
                if (hr != E_INVALIDARG || heap) throw std::runtime_error("invalid visible heap accepted");
                continue;
            }
            check(hr, "create descriptor heap");
            const auto actual = heap->GetDesc();
            if (actual.Type != desc.Type || actual.Flags != desc.Flags ||
                actual.NodeMask != desc.NodeMask || actual.NumDescriptors != desc.NumDescriptors)
                throw std::runtime_error("descriptor heap description mismatch");
            if (!device->GetDescriptorHandleIncrementSize(type) ||
                !method<CpuHandle>(heap.Get(), 9)(heap.Get()) ||
                (method<GpuHandle>(heap.Get(), 10)(heap.Get()) != 0) != visible)
                throw std::runtime_error("descriptor heap handle mismatch");
        }
    }
}

std::string run(const wchar_t* requested, bool) {
    DXGI_ADAPTER_DESC1 adapter{};
    auto device = create_device(requested, adapter);
    descriptor_heaps(device.Get());
    D3D12_COMMAND_QUEUE_DESC direct{};
    ComPtr<ID3D12CommandQueue> queue;
    check(device->CreateCommandQueue(&direct, IID_PPV_ARGS(&queue)), "direct queue");
    UINT64 frequency{}, gpu{}, cpu{};
    check(queue->GetTimestampFrequency(&frequency), "timestamp frequency");
    check(queue->GetClockCalibration(&gpu, &cpu), "clock calibration");
    if (!frequency || !gpu || !cpu) throw std::runtime_error("empty clock");
    ComPtr<ID3D12CommandAllocator> allocator;
    check(device->CreateCommandAllocator(direct.Type, IID_PPV_ARGS(&allocator)), "allocator");
    const UINT creation[]{D3D12_COMMAND_LIST_TYPE_DIRECT, 0, 1, 0};
    ComPtr<ID3D12GraphicsCommandList> list;
    check(method<CreateList>(device.Get(), 73)(device.Get(), creation, allocator.Get(), nullptr,
          IID_PPV_ARGS(&list)), "query list");
    D3D12_QUERY_HEAP_DESC desc{D3D12_QUERY_HEAP_TYPE_TIMESTAMP, 4, 0};
    ComPtr<ID3D12QueryHeap> timestamps, occlusion;
    check(device->CreateQueryHeap(&desc, IID_PPV_ARGS(&timestamps)), "timestamp heap");
    desc.Type = D3D12_QUERY_HEAP_TYPE_OCCLUSION;
    check(device->CreateQueryHeap(&desc, IID_PPV_ARGS(&occlusion)), "occlusion heap");

    D3D12_HEAP_PROPERTIES heap{};
    heap.Type = D3D12_HEAP_TYPE_READBACK;
    heap.CreationNodeMask = heap.VisibleNodeMask = 1;
    D3D12_RESOURCE_DESC buffer{};
    buffer.Dimension = D3D12_RESOURCE_DIMENSION_BUFFER;
    buffer.Width = 5 * sizeof(UINT64);
    buffer.Height = 1;
    buffer.DepthOrArraySize = 1;
    buffer.MipLevels = 1;
    buffer.SampleDesc.Count = 1;
    buffer.Layout = D3D12_TEXTURE_LAYOUT_ROW_MAJOR;
    ComPtr<ID3D12Resource> result;
    check(device->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &buffer, D3D12_RESOURCE_STATE_COPY_DEST,
          nullptr, IID_PPV_ARGS(&result)), "readback buffer");
    UINT64* words = nullptr;
    D3D12_RANGE none{}, all{0, static_cast<SIZE_T>(buffer.Width)};
    check(result->Map(0, &none, reinterpret_cast<void**>(&words)), "initialize readback");
    constexpr UINT64 guard = 0x427438fcdeaddeafull;
    std::fill_n(words, 5, guard);
    result->Unmap(0, &all);

    auto* cmd = list.Get();
    auto begin = method<Record>(cmd, 28);
    auto end = method<Record>(cmd, 29);
    auto resolve = method<Resolve>(cmd, 30);
    check(cmd->Reset(allocator.Get(), nullptr), "reset query list");
    end(cmd, timestamps.Get(), D3D12_QUERY_TYPE_TIMESTAMP, 1);
    begin(cmd, occlusion.Get(), D3D12_QUERY_TYPE_OCCLUSION, 0);
    end(cmd, occlusion.Get(), D3D12_QUERY_TYPE_OCCLUSION, 0);
    end(cmd, timestamps.Get(), D3D12_QUERY_TYPE_TIMESTAMP, 2);
    resolve(cmd, timestamps.Get(), D3D12_QUERY_TYPE_TIMESTAMP, 1, 2, result.Get(), 8);
    resolve(cmd, occlusion.Get(), D3D12_QUERY_TYPE_OCCLUSION, 0, 1, result.Get(), 24);
    check(cmd->Close(), "close query list");
    ID3D12CommandList* batch[]{cmd};
    queue->ExecuteCommandLists(1, batch);
    ComPtr<ID3D12Fence> fence;
    check(device->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&fence)), "query fence");
    check(queue->Signal(fence.Get(), 1), "signal query fence");
    HANDLE done = CreateEventW(nullptr, FALSE, FALSE, nullptr);
    if (!done) throw std::runtime_error("query event");
    HRESULT armed = fence->SetEventOnCompletion(1, done);
    DWORD waited = SUCCEEDED(armed) ? WaitForSingleObject(done, 15000) : WAIT_FAILED;
    CloseHandle(done);
    if (waited != WAIT_OBJECT_0 || fence->GetCompletedValue() < 1) throw std::runtime_error("query fence timeout");
    check(result->Map(0, &all, reinterpret_cast<void**>(&words)), "read query results");
    bool correct = words[0] == guard && words[4] == guard && words[3] == 0 &&
                   words[1] && words[1] != guard && words[2] != guard && words[2] >= words[1];
    result->Unmap(0, &none);
    if (!correct) throw std::runtime_error("invalid timestamps, occlusion count or guards");
    check(cmd->Reset(allocator.Get(), nullptr), "reset for invalid range");
    resolve(cmd, timestamps.Get(), D3D12_QUERY_TYPE_TIMESTAMP, 3, 2, result.Get(), 0);
    if (cmd->Close() != E_INVALIDARG) throw std::runtime_error("out-of-range query accepted");

    char receipt[140];
    std::snprintf(receipt, sizeof(receipt), "d3d12-queries PASS pci=%04x:%04x timestamps=2 occlusion=0 guards=2 heaps=6 rejected=3",
                  adapter.VendorId, adapter.DeviceId);
    return receipt;
}
}
int wmain(int argc, wchar_t** argv) {
    return tuxbox_test::frame_main(argc, argv, "d3d12-queries", false, run);
}
