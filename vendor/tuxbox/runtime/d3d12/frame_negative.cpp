// Guest regression for rejecting invalid inputs before they reach a desktop queue.
#include "frame_abi.h"
#include "frame_cli.h"

namespace {
using namespace frame_abi;

void expect_hr(HRESULT actual, HRESULT expected, const char* operation) {
    if (actual == expected) return;
    char message[160];
    std::snprintf(message, sizeof(message), "%s returned %#lx, expected %#lx", operation,
                  static_cast<unsigned long>(actual), static_cast<unsigned long>(expected));
    throw std::runtime_error(message);
}

struct Context {
    DXGI_ADAPTER_DESC1 selected{};
    ComPtr<ID3D12Device> device;
    ComPtr<ID3D12CommandQueue> queue;
    ComPtr<ID3D12CommandAllocator> allocator;
    ComPtr<ID3D12GraphicsCommandList> list;
    ComPtr<ID3D12Fence> fence;

    explicit Context(const wchar_t* adapter) : device(create_device(adapter, selected)) {
        const auto actual = device->GetAdapterLuid();
        if (actual.LowPart != selected.AdapterLuid.LowPart || actual.HighPart != selected.AdapterLuid.HighPart)
            throw std::runtime_error("guest device adapter mismatch");
        D3D12_COMMAND_QUEUE_DESC desc{};
        desc.Type = D3D12_COMMAND_LIST_TYPE_DIRECT;
        check(device->CreateCommandQueue(&desc, IID_PPV_ARGS(&queue)), "create direct queue");
        check(device->CreateCommandAllocator(desc.Type, IID_PPV_ARGS(&allocator)), "create allocator");
        const UINT params[] = {D3D12_COMMAND_LIST_TYPE_DIRECT, 0, 1, 0};
        check(method<CreateList>(device.Get(), 73)(device.Get(), params, allocator.Get(), nullptr,
              IID_PPV_ARGS(&list)), "create list");
        check(device->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&fence)), "create fence");
    }
};

ComPtr<ID3D12Resource> make_texture(ID3D12Device* device, D3D12_RESOURCE_STATES state) {
    D3D12_HEAP_PROPERTIES heap{};
    heap.Type = D3D12_HEAP_TYPE_DEFAULT;
    heap.CreationNodeMask = heap.VisibleNodeMask = 1;
    D3D12_RESOURCE_DESC desc{};
    desc.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
    desc.Width = 1;
    desc.Height = 1;
    desc.DepthOrArraySize = 1;
    desc.MipLevels = 1;
    desc.SampleDesc.Count = 1;
    desc.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
    ComPtr<ID3D12Resource> texture;
    check(device->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, state, nullptr,
          IID_PPV_ARGS(&texture)), "create texture for copy controls");
    return texture;
}

void wait_for_recovered_list(ID3D12Fence* fence) {
    HANDLE event = CreateEventW(nullptr, FALSE, FALSE, nullptr);
    if (!event) throw std::runtime_error("could not create recovery fence event");
    HRESULT hr = fence->SetEventOnCompletion(1, event);
    auto status = SUCCEEDED(hr) ? WaitForSingleObject(event, 15000) : WAIT_FAILED;
    CloseHandle(event);
    check(hr, "arm recovery fence event");
    if (status != WAIT_OBJECT_0 || fence->GetCompletedValue() != 1)
        throw std::runtime_error("recovered list did not reach its fence");
}

void verify_factory_record(ID3D12Device* device) {
    void* out = device;
    // Only the first DWORD is defined for an unknown version. Place it at the
    // end of the committed page to detect any read of a full profile.
    SYSTEM_INFO info{};
    GetSystemInfo(&info);
    auto* memory = static_cast<BYTE*>(VirtualAlloc(nullptr, 2 * info.dwPageSize, MEM_RESERVE, PAGE_NOACCESS));
    if (!memory) throw std::runtime_error("could not reserve version probe");
    if (!VirtualAlloc(memory, info.dwPageSize, MEM_COMMIT, PAGE_READWRITE)) {
        VirtualFree(memory, 0, MEM_RELEASE);
        throw std::runtime_error("could not commit version probe");
    }
    auto* version = reinterpret_cast<UINT*>(memory + info.dwPageSize - sizeof(UINT));
    *version = 0xffffffff;
    const auto hr = D3D12XboxCreateDevice(nullptr, version, __uuidof(ID3D12Device), &out);
    VirtualFree(memory, 0, MEM_RELEASE);
    if (hr != E_NOTIMPL || out) throw std::runtime_error("unrecognized device-creation version accepted");

    out = device;
    auto changed = profile;
    changed.back() = 1;
    if (D3D12XboxCreateDevice(nullptr, changed.data(), __uuidof(ID3D12Device), &out) != E_NOTIMPL || out)
        throw std::runtime_error("unsupported creation record accepted");
}

std::string run(const wchar_t* adapter, bool) {
    Context first(adapter);
    verify_factory_record(first.device.Get());
    wchar_t pci[16];
    std::swprintf(pci, 16, L"%04x:%04x", first.selected.VendorId, first.selected.DeviceId);
    Context second(pci);
    if (first.selected.AdapterLuid.LowPart != second.selected.AdapterLuid.LowPart ||
        first.selected.AdapterLuid.HighPart != second.selected.AdapterLuid.HighPart)
        throw std::runtime_error("foreign-parent control needs two guest devices on one adapter");
    expect_hr(first.list->Reset(second.allocator.Get(), nullptr), E_INVALIDARG, "reset with foreign allocator");
    check(second.list->Close(), "close initial empty recording");
    expect_hr(second.list->Close(), E_FAIL, "double-close of initial recording");
    expect_hr(first.queue->Signal(second.fence.Get(), 1), E_INVALIDARG, "signal foreign fence");

    auto src = make_texture(first.device.Get(), D3D12_RESOURCE_STATE_COPY_SOURCE);
    auto dst = make_texture(first.device.Get(), D3D12_RESOURCE_STATE_COPY_DEST);
    D3D12_TEXTURE_COPY_LOCATION from{}, to{};
    from.pResource = src.Get();
    to.pResource = dst.Get();
    from.Type = to.Type = D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
    const auto copy = method<Copy>(first.list.Get(), 83);
    check(first.list->Reset(first.allocator.Get(), nullptr), "reset before flags control");
    expect_hr(first.list->Reset(first.allocator.Get(), nullptr), E_FAIL, "reset an open recording");
    copy(first.list.Get(), &to, 0, 0, 0, &from, nullptr, 1);
    expect_hr(first.list->Close(), E_NOTIMPL, "close after unsupported copy flags");
    check(first.list->Reset(first.allocator.Get(), nullptr), "reset before invalid location control");
    copy(first.list.Get(), &to, 0, 0, 0, nullptr, nullptr, 0);
    expect_hr(first.list->Close(), E_INVALIDARG, "close after null copy source");

    check(first.list->Reset(first.allocator.Get(), nullptr), "reset after failed recording");
    copy(first.list.Get(), &to, 0, 0, 0, &from, nullptr, 0);
    check(first.list->Close(), "close recovered list");
    ID3D12CommandList* batch[]{first.list.Get()};
    first.queue->ExecuteCommandLists(1, batch);
    check(first.queue->Signal(first.fence.Get(), 1), "signal after recovered submission");
    wait_for_recovered_list(first.fence.Get());

    D3D12_COMMAND_QUEUE_DESC compute_desc{};
    compute_desc.Type = D3D12_COMMAND_LIST_TYPE_COMPUTE;
    ComPtr<ID3D12CommandQueue> compute;
    check(method<CreateQueue>(first.device.Get(), 48)(first.device.Get(), &compute_desc,
          IID_PPV_ARGS(&compute)), "create compute queue");
    ID3D12Resource* texture = src.Get();
    Plane plane{1, 1, 0, &texture};
    expect_hr(method<Present>(compute.Get(), 30)(compute.Get(), 1, &plane, nullptr),
              E_NOTIMPL, "present on compute queue");
    compute->ExecuteCommandLists(1, batch);
    expect_hr(compute->Signal(first.fence.Get(), 2), E_INVALIDARG, "direct list on compute queue");
    if (first.fence->GetCompletedValue() != 1) throw std::runtime_error("compute batch advanced fence");

    ID3D12CommandList* mixed[]{second.list.Get(), first.list.Get()};
    second.queue->ExecuteCommandLists(2, mixed);
    expect_hr(second.queue->Signal(second.fence.Get(), 1), E_INVALIDARG, "signal after mixed batch");
    if (second.fence->GetCompletedValue() != 0) throw std::runtime_error("mixed batch advanced fence");
    check(first.list->Reset(first.allocator.Get(), nullptr), "open list for invalid submission");
    first.queue->ExecuteCommandLists(1, batch);
    expect_hr(first.queue->Signal(first.fence.Get(), 2), E_INVALIDARG, "signal after open list");
    if (first.fence->GetCompletedValue() != 1) throw std::runtime_error("open list advanced fence");

    char receipt[144];
    std::snprintf(receipt, sizeof(receipt), "xbox-frame-negative PASS pci=%04x:%04x "
                  "foreign-parents=2 copy-errors=2 recovered=1 rejected-batches=3 compute-present-rejected=1",
                  first.selected.VendorId, first.selected.DeviceId);
    return receipt;
}
}

int wmain(int argc, wchar_t** argv) {
    return tuxbox_test::frame_main(argc, argv, "xbox-frame-negative", false, run);
}
