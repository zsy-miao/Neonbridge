// Verify a transfer on a COPY queue, ordered by a fence from a DIRECT queue.
#include "compute_fixture.h"
#include "frame_cli.h"
#include <cstring>

namespace {
using namespace frame_abi;
constexpr UINT bytes = 64;

std::string run(const wchar_t* requested, bool) {
    DXGI_ADAPTER_DESC1 selected{};
    auto device = create_device(requested, selected);
    ComPtr<ID3D12CommandQueue> direct, copy, compute;
    D3D12_COMMAND_QUEUE_DESC queue{};
    queue.Type = D3D12_COMMAND_LIST_TYPE_DIRECT;
    check(device->CreateCommandQueue(&queue, IID_PPV_ARGS(&direct)), "direct queue");
    queue.Type = D3D12_COMMAND_LIST_TYPE_COPY;
    queue.Flags = D3D12_COMMAND_QUEUE_FLAG_DISABLE_GPU_TIMEOUT;
    queue.NodeMask = 1;
    check(device->CreateCommandQueue(&queue, IID_PPV_ARGS(&copy)), "copy queue");
    queue.Type = D3D12_COMMAND_LIST_TYPE_COMPUTE;
    queue.Flags = D3D12_COMMAND_QUEUE_FLAG_NONE;
    check(device->CreateCommandQueue(&queue, IID_PPV_ARGS(&compute)), "compute queue");
    queue.Flags = static_cast<D3D12_COMMAND_QUEUE_FLAGS>(0x40000000);
    void* rejected = device.Get();
    if (device->CreateCommandQueue(&queue, __uuidof(ID3D12CommandQueue), &rejected) != E_NOTIMPL || rejected)
        throw std::runtime_error("unmapped queue flags accepted");

    auto upload = compute_fixture::buffer(device.Get(), D3D12_HEAP_TYPE_UPLOAD, bytes, D3D12_RESOURCE_STATE_GENERIC_READ);
    auto readback = compute_fixture::buffer(device.Get(), D3D12_HEAP_TYPE_READBACK, bytes, D3D12_RESOURCE_STATE_COPY_DEST);
    std::array<BYTE, bytes> expected{};
    for (UINT i = 0; i < bytes; ++i) expected[i] = static_cast<BYTE>(0xa5 ^ (7 * i));
    compute_fixture::upload(upload.Get(), expected.data(), bytes);

    ComPtr<ID3D12CommandAllocator> allocator;
    check(device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_COPY, IID_PPV_ARGS(&allocator)), "copy allocator");
    ComPtr<ID3D12GraphicsCommandList> list;
    const UINT desc[] = {D3D12_COMMAND_LIST_TYPE_COPY, 0, 1, 0};
    check(method<CreateList>(device.Get(), 73)(device.Get(), desc, allocator.Get(), nullptr,
          IID_PPV_ARGS(&list)), "copy list");
    check(list->Reset(allocator.Get(), nullptr), "open copy list");
    list->CopyBufferRegion(readback.Get(), 0, upload.Get(), 0, bytes);
    check(list->Close(), "close copy list");
    ComPtr<ID3D12Fence> gate, done;
    check(device->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&gate)), "gate fence");
    check(device->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&done)), "copy fence");
    if (copy->Wait(nullptr, 1) != E_INVALIDARG)
        throw std::runtime_error("null wait fence accepted");
    using SignalX = HRESULT (WINAPI*)(ID3D12CommandQueue*, ID3D12Fence*, UINT64, UINT);
    auto signal = [](ID3D12CommandQueue* q, ID3D12Fence* f, UINT64 value) {
        UINT type{};
        std::memcpy(&type, reinterpret_cast<const BYTE*>(q) + 0x10, sizeof(type));
        const UINT flags = type == D3D12_COMMAND_LIST_TYPE_COPY ? 0 : 0x08000000u;
        check(method<SignalX>(q, 23)(q, f, value, flags), "guest SignalX");
        return type;
    };
    if (method<SignalX>(copy.Get(), 23)(copy.Get(), done.Get(), 17, 0x4000) != E_NOTIMPL)
        throw std::runtime_error("unexpected SignalX flush flags accepted");
    check(copy->Wait(gate.Get(), 7), "copy queue wait");
    ID3D12CommandList* batch[]{list.Get()};
    copy->ExecuteCommandLists(1, batch);
    if (signal(copy.Get(), done.Get(), 9) != D3D12_COMMAND_LIST_TYPE_COPY) throw std::runtime_error("copy queue type");
    if (done->GetCompletedValue() != 0) throw std::runtime_error("copy completed before gate");
    if (signal(direct.Get(), gate.Get(), 7) != D3D12_COMMAND_LIST_TYPE_DIRECT) throw std::runtime_error("direct queue type");

    compute_fixture::wait_for_fence(done.Get(), 9);
    D3D12_RANGE range{0, bytes}, empty{};
    void* mapped{};
    check(readback->Map(0, &range, &mapped), "map readback");
    bool matched = std::memcmp(mapped, expected.data(), bytes) == 0;
    readback->Unmap(0, &empty);
    if (!matched) throw std::runtime_error("GPU copy byte mismatch");
    char receipt[100];
    std::snprintf(receipt, sizeof(receipt), "d3d12-queues PASS pci=%04x:%04x queues=3 bytes=64 wait=1 signal-x=2",
                  selected.VendorId, selected.DeviceId);
    return receipt;
}
}

int wmain(int argc, wchar_t** argv) {
    return tuxbox_test::frame_main(argc, argv, "d3d12-queues", false, run);
}
