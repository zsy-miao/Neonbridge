#include <cstring>
#include "course.h"
#include "gpu.h"
#include "course_shader.h"

using Microsoft::WRL::ComPtr;
using frame_abi::check;
using frame_abi::method;

namespace {
// Release device creation record used by Frosty with AutoHDR off.
constexpr std::array<UINT, 18> device_profile{
    0x20004, 0, UINT_MAX, 0, 0, 0, UINT_MAX, UINT_MAX, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0};

bool matches_adapter(IDXGIAdapter1* adapter, const wchar_t* requested, std::string& pci) {
    DXGI_ADAPTER_DESC1 desc{};
    check(adapter->GetDesc1(&desc), "adapter description");
    if (desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE) return false;
    wchar_t id[16];
    std::swprintf(id, 16, L"%04x:%04x", desc.VendorId, desc.DeviceId);
    if (requested && _wcsicmp(id, requested)) return false;
    pci.clear();
    for (const wchar_t* p = id; *p; ++p) pci.push_back(static_cast<char>(*p));
    return true;
}
}

ComPtr<ID3D12Device> create_lanes_device(const wchar_t* requested, std::string& pci) {
    ComPtr<ID3D12Device> device;
    check(D3D12XboxCreateDevice(nullptr, device_profile.data(), IID_PPV_ARGS(&device)), "Xbox device");
    ComPtr<IDXGIDevice1> dxgi;
    check(device.As(&dxgi), "Xbox device DXGI interface");
    ComPtr<IDXGIAdapter> adapter;
    check(dxgi->GetAdapter(&adapter), "Xbox device adapter");
    ComPtr<IDXGIAdapter1> adapter1;
    check(adapter.As(&adapter1), "Xbox device adapter interface");
    if (matches_adapter(adapter1.Get(), requested, pci)) return device;

    // Reach the factory through the Xbox device for an explicit adapter override.
    ComPtr<IDXGIFactory1> factory;
    check(adapter->GetParent(IID_PPV_ARGS(&factory)), "Xbox adapter parent");
    for (UINT n = 0; ; ++n) {
        ComPtr<IDXGIAdapter1> candidate;
        HRESULT hr = factory->EnumAdapters1(n, &candidate);
        if (hr == DXGI_ERROR_NOT_FOUND) throw std::runtime_error("no matching Xbox D3D12 hardware adapter");
        check(hr, "adapter enumeration");
        if (!matches_adapter(candidate.Get(), requested, pci)) continue;
        ComPtr<ID3D12Device> other;
        hr = D3D12XboxCreateDevice(candidate.Get(), device_profile.data(), IID_PPV_ARGS(&other));
        if (SUCCEEDED(hr)) return other;
        if (requested) check(hr, "Xbox device on selected adapter");
    }
}

ComPtr<ID3D12CommandQueue> create_lanes_queue(ID3D12Device* device, D3D12_COMMAND_LIST_TYPE type) {
    D3D12_COMMAND_QUEUE_DESC desc{};
    desc.Type = type;
    ComPtr<ID3D12CommandQueue> queue;
    const size_t slot = type == D3D12_COMMAND_LIST_TYPE_COMPUTE ? 48 : 8;
    check(method<frame_abi::CreateQueue>(device, slot)(device, &desc, IID_PPV_ARGS(&queue)), "Xbox queue");
    return queue;
}

ComPtr<ID3D12GraphicsCommandList> create_lanes_list(
    ID3D12Device* device, D3D12_COMMAND_LIST_TYPE type, ID3D12CommandAllocator* allocator) {
    const UINT desc[] = {static_cast<UINT>(type), 0, 1, 0};
    ComPtr<ID3D12GraphicsCommandList> list;
    check(method<frame_abi::CreateList>(device, 73)(device, desc, allocator, nullptr, IID_PPV_ARGS(&list)),
          "Xbox command list");
    return list;
}

void generate_course(ID3D12Device* device, Course& rows, std::uint32_t seed) {
    D3D12_ROOT_PARAMETER params[2]{};
    params[0].ParameterType = D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS;
    params[0].Constants = {1, 2, 2};
    params[1].ParameterType = D3D12_ROOT_PARAMETER_TYPE_UAV;
    params[1].Descriptor = {2, 1};
    D3D12_ROOT_SIGNATURE_DESC signature{};
    signature.NumParameters = 2;
    signature.pParameters = params;
    ComPtr<ID3DBlob> serialized;
    check(D3D12SerializeRootSignature(&signature, D3D_ROOT_SIGNATURE_VERSION_1, &serialized, nullptr),
          "Xbox-facing root serializer");
    ComPtr<ID3D12RootSignature> root;
    check(device->CreateRootSignature(0, serialized->GetBufferPointer(), serialized->GetBufferSize(),
                                      IID_PPV_ARGS(&root)), "course root signature");
    D3D12_COMPUTE_PIPELINE_STATE_DESC shader{};
    shader.pRootSignature = root.Get();
    shader.CS = {course_dxil, sizeof(course_dxil)};
    ComPtr<ID3D12PipelineState> pipeline;
    using CreateComputePipeline = HRESULT (WINAPI*)(ID3D12Device*, const D3D12_COMPUTE_PIPELINE_STATE_DESC*,
                                                    UINT, const void*, REFIID, void**);
    check(method<CreateComputePipeline>(device, 62)(device, &shader, 0, nullptr, IID_PPV_ARGS(&pipeline)),
          "Xbox compute pipeline");
    auto queue = create_lanes_queue(device, D3D12_COMMAND_LIST_TYPE_COMPUTE);
    ComPtr<ID3D12CommandAllocator> allocator;
    check(device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_COMPUTE, IID_PPV_ARGS(&allocator)), "course allocator");
    auto commands = create_lanes_list(device, D3D12_COMMAND_LIST_TYPE_COMPUTE, allocator.Get());
    check(commands->Reset(allocator.Get(), pipeline.Get()), "course list reset");

    D3D12_RESOURCE_DESC desc{};
    desc.Dimension = D3D12_RESOURCE_DIMENSION_BUFFER;
    desc.Width = sizeof(rows);
    desc.Height = desc.DepthOrArraySize = desc.MipLevels = 1;
    desc.SampleDesc.Count = 1;
    desc.Layout = D3D12_TEXTURE_LAYOUT_ROW_MAJOR;
    desc.Flags = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
    D3D12_HEAP_PROPERTIES heap{};
    heap.CreationNodeMask = heap.VisibleNodeMask = 1;
    heap.Type = D3D12_HEAP_TYPE_DEFAULT;
    ComPtr<ID3D12Resource> result, readback;
    check(device->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                                          nullptr, IID_PPV_ARGS(&result)), "course output");
    heap.Type = D3D12_HEAP_TYPE_READBACK;
    desc.Flags = D3D12_RESOURCE_FLAG_NONE;
    check(device->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COPY_DEST,
                                          nullptr, IID_PPV_ARGS(&readback)), "course readback");
    const UINT constants[] = {seed, course_length};
    using SetRoot = void (WINAPI*)(ID3D12GraphicsCommandList*, ID3D12RootSignature*);
    using SetConstants = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, UINT, const void*, UINT);
    using SetUav = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, D3D12_GPU_VIRTUAL_ADDRESS);
    auto* list = commands.Get();
    method<SetRoot>(list, 33)(list, root.Get());
    method<SetConstants>(list, 39)(list, 0, 2, constants, 0);
    method<SetUav>(list, 45)(list, 1, result->GetGPUVirtualAddress());
    commands->Dispatch((course_length + 7) / 8, 1, 1);
    D3D12_RESOURCE_BARRIER step{};
    step.Type = D3D12_RESOURCE_BARRIER_TYPE_TRANSITION;
    step.Transition = {result.Get(), D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES, D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                       D3D12_RESOURCE_STATE_COPY_SOURCE};
    commands->ResourceBarrier(1, &step);
    commands->CopyBufferRegion(readback.Get(), 0, result.Get(), 0, sizeof(rows));
    check(commands->Close(), "course list close");
    ComPtr<ID3D12Fence> fence;
    check(device->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&fence)), "course fence");
    ID3D12CommandList* batch[] = {commands.Get()};
    queue->ExecuteCommandLists(1, batch);
    check(queue->Signal(fence.Get(), 1), "course signal");
    HANDLE done = CreateEventW(nullptr, FALSE, FALSE, nullptr);
    if (!done) throw std::runtime_error("cannot create course completion event");
    HRESULT hr = fence->SetEventOnCompletion(1, done);
    DWORD wait = SUCCEEDED(hr) ? WaitForSingleObject(done, 15000) : WAIT_FAILED;
    CloseHandle(done);
    check(hr, "course completion notification");
    if (wait != WAIT_OBJECT_0 || fence->GetCompletedValue() != 1) throw std::runtime_error("course dispatch did not complete");
    D3D12_RANGE bytes{0, sizeof(rows)}, no_writes{};
    void* mapped = nullptr;
    check(readback->Map(0, &bytes, &mapped), "course readback map");
    std::memcpy(rows.data(), mapped, sizeof(rows));
    readback->Unmap(0, &no_writes);
    for (auto packed : rows)
        if ((packed & 255) > 2 || (packed >> 8) > 2 || (packed & 255) == (packed >> 8))
            throw std::runtime_error("GPU generated an invalid lane pair");
}
