// Compare the observed Xbox command signature descriptor with a desktop D3D12 control.
// The command signature crosses the selected provider. Its owning desktop device runs
// the shader and reads back the result, isolating this test from unrelated Xbox list methods.
#include "frame_abi.h"
#include "groups_dxil.h"
#include <cstring>

using frame_abi::check;
using frame_abi::ComPtr;

static constexpr UINT WordCount = 81;
static constexpr UINT BufferBytes = WordCount * sizeof(UINT);

static ComPtr<ID3D12Resource> buffer(ID3D12Device* device, D3D12_HEAP_TYPE type, D3D12_RESOURCE_STATES state,
                                     D3D12_RESOURCE_FLAGS flags) {
    D3D12_HEAP_PROPERTIES props{};
    props.Type = type;
    props.VisibleNodeMask = props.CreationNodeMask = 1;
    D3D12_RESOURCE_DESC desc{};
    desc.Dimension = D3D12_RESOURCE_DIMENSION_BUFFER;
    desc.Width = BufferBytes;
    desc.Height = desc.MipLevels = desc.DepthOrArraySize = 1;
    desc.SampleDesc.Count = 1;
    desc.Layout = D3D12_TEXTURE_LAYOUT_ROW_MAJOR;
    desc.Flags = flags;
    ComPtr<ID3D12Resource> resource;
    check(device->CreateCommittedResource(&props, D3D12_HEAP_FLAG_NONE, &desc, state, nullptr,
                                          IID_PPV_ARGS(&resource)),
          "CreateCommittedResource");
    return resource;
}

static void transition(ID3D12GraphicsCommandList* list, ID3D12Resource* buffer, D3D12_RESOURCE_STATES before,
                       D3D12_RESOURCE_STATES after) {
    D3D12_RESOURCE_BARRIER barrier{};
    barrier.Type = D3D12_RESOURCE_BARRIER_TYPE_TRANSITION;
    barrier.Transition = {buffer, D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES, before, after};
    list->ResourceBarrier(1, &barrier);
}

static void write_upload(ID3D12Resource* resource, const void* data, SIZE_T size) {
    D3D12_RANGE empty{}, written{0, size};
    void* dest = nullptr;
    check(resource->Map(0, &empty, &dest), "Map upload");
    std::memcpy(dest, data, size);
    resource->Unmap(0, &written);
}

static ComPtr<ID3D12RootSignature> create_root(ID3D12Device* device) {
    D3D12_ROOT_PARAMETER parameter{};
    parameter.ParameterType = D3D12_ROOT_PARAMETER_TYPE_UAV;
    D3D12_ROOT_SIGNATURE_DESC desc{1, &parameter, 0, nullptr, D3D12_ROOT_SIGNATURE_FLAG_NONE};
    ComPtr<ID3DBlob> blob;
    check(D3D12SerializeRootSignature(&desc, D3D_ROOT_SIGNATURE_VERSION_1, &blob, nullptr),
          "SerializeRootSignature");
    ComPtr<ID3D12RootSignature> root;
    check(device->CreateRootSignature(0, blob->GetBufferPointer(), blob->GetBufferSize(), IID_PPV_ARGS(&root)),
          "CreateRootSignature");
    return root;
}

static void dispatch_and_check(ID3D12Device* owner, ID3D12RootSignature* root, ID3D12CommandSignature* signature) {
    D3D12_COMPUTE_PIPELINE_STATE_DESC pso_desc{};
    pso_desc.pRootSignature = root;
    pso_desc.CS = {groups_dxil, sizeof(groups_dxil)};
    ComPtr<ID3D12PipelineState> pso;
    check(owner->CreateComputePipelineState(&pso_desc, IID_PPV_ARGS(&pso)), "CreateComputePipelineState");
    D3D12_COMMAND_QUEUE_DESC queue_desc{};
    ComPtr<ID3D12CommandQueue> queue;
    ComPtr<ID3D12CommandAllocator> allocator;
    ComPtr<ID3D12GraphicsCommandList> list;
    check(owner->CreateCommandQueue(&queue_desc, IID_PPV_ARGS(&queue)), "CreateCommandQueue");
    check(owner->CreateCommandAllocator(queue_desc.Type, IID_PPV_ARGS(&allocator)), "CreateCommandAllocator");
    check(owner->CreateCommandList(0, queue_desc.Type, allocator.Get(), pso.Get(), IID_PPV_ARGS(&list)),
          "CreateCommandList");

    auto args = buffer(owner, D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_STATE_GENERIC_READ, D3D12_RESOURCE_FLAG_NONE);
    auto initial =
        buffer(owner, D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_STATE_GENERIC_READ, D3D12_RESOURCE_FLAG_NONE);
    auto output = buffer(owner, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COPY_DEST,
                         D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
    auto result = buffer(owner, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_FLAG_NONE);
    // An unused command precedes the two contiguous records to exercise offset and stride.
    const D3D12_DISPATCH_ARGUMENTS records[] = {{1, 1, 1}, {3, 2, 1}, {1, 2, 3}};
    static_assert(sizeof(records[0]) == 12);
    write_upload(args.Get(), records, sizeof(records));
    std::array<UINT, WordCount> expected{};
    write_upload(initial.Get(), expected.data(), BufferBytes);
    list->CopyBufferRegion(output.Get(), 0, initial.Get(), 0, BufferBytes);
    transition(list.Get(), output.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
    list->SetComputeRootSignature(root);
    list->SetComputeRootUnorderedAccessView(0, output->GetGPUVirtualAddress());
    list->ExecuteIndirect(signature, 2, args.Get(), sizeof(records[0]), nullptr, 0);
    transition(list.Get(), output.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
    list->CopyBufferRegion(result.Get(), 0, output.Get(), 0, BufferBytes);
    check(list->Close(), "Close command list");

    ID3D12CommandList* lists[] = {list.Get()};
    ComPtr<ID3D12Fence> fence;
    check(owner->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&fence)), "CreateFence");
    queue->ExecuteCommandLists(1, lists);
    check(queue->Signal(fence.Get(), 1), "Signal");
    HANDLE event = CreateEventW(nullptr, FALSE, FALSE, nullptr);
    if (!event)
        throw std::runtime_error("CreateEventW failed");
    HRESULT hr = fence->SetEventOnCompletion(1, event);
    DWORD wait = SUCCEEDED(hr) ? WaitForSingleObject(event, 15000) : WAIT_FAILED;
    CloseHandle(event);
    check(hr, "SetEventOnCompletion");
    if (wait != WAIT_OBJECT_0 || fence->GetCompletedValue() != 1)
        throw std::runtime_error("GPU fence did not complete");

    for (UINT z = 0; z < 4; ++z) {
        for (UINT y = 0; y < 4; ++y) {
            for (UINT x = 0; x < 5; ++x) {
                expected[x + 5 * (y + 4 * z)] = (x < 3 && y < 2 && z == 0) + (x == 0 && y < 2 && z < 3);
            }
        }
    }
    D3D12_RANGE read{0, BufferBytes}, empty{};
    void* mapped = nullptr;
    check(result->Map(0, &read, &mapped), "Map readback");
    bool matches = std::memcmp(mapped, expected.data(), BufferBytes) == 0;
    result->Unmap(0, &empty);
    if (!matches)
        throw std::runtime_error("GPU group counts differ from CPU oracle");
}

static HRESULT create_signature(ID3D12Device* device, UINT type, UINT stride, ID3D12CommandSignature** out,
                                ID3D12RootSignature* root = nullptr) {
    D3D12_INDIRECT_ARGUMENT_DESC arg{};
    static_assert(sizeof(arg.Type) == sizeof(type));
    std::memcpy(&arg.Type, &type, sizeof(type));
    D3D12_COMMAND_SIGNATURE_DESC desc{stride, 1, &arg, 1};
    return device->CreateCommandSignature(&desc, root, __uuidof(ID3D12CommandSignature),
                                          reinterpret_cast<void**>(out));
}

static void run(bool xbox, FILE* receipt) {
    ComPtr<ID3D12Device> provider;
    if (xbox) {
        // Frosty's 72-byte profile, with debug and AutoHDR fields set to zero.
        const std::array<UINT, 18> profile{0x20004, 0, ~0u, 0, 0, 0, ~0u, ~0u, 0, 1};
        HMODULE mod = LoadLibraryExW(L"d3d12_xs.dll", nullptr,
                                     LOAD_LIBRARY_SEARCH_APPLICATION_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32);
        if (!mod)
            throw std::runtime_error("LoadLibraryExW d3d12_xs.dll error " + std::to_string(GetLastError()));
        using Factory = HRESULT(WINAPI*)(IUnknown*, const void*, REFIID, void**);
        auto factory = reinterpret_cast<Factory>(GetProcAddress(mod, "D3D12XboxCreateDevice"));
        if (!factory)
            throw std::runtime_error("Missing D3D12XboxCreateDevice");
        check(factory(nullptr, profile.data(), IID_PPV_ARGS(&provider)), "D3D12XboxCreateDevice");
    } else {
        check(D3D12CreateDevice(nullptr, D3D_FEATURE_LEVEL_12_0, IID_PPV_ARGS(&provider)), "D3D12CreateDevice");
    }
    ComPtr<ID3D12CommandSignature> compute, mesh;
    const HRESULT compute_hr =
        create_signature(provider.Get(), xbox ? 105u : D3D12_INDIRECT_ARGUMENT_TYPE_DISPATCH, 12, &compute);
    const HRESULT mesh_hr =
        create_signature(provider.Get(), xbox ? 107u : D3D12_INDIRECT_ARGUMENT_TYPE_DISPATCH_MESH, 16, &mesh);
    std::fprintf(receipt, "compute_hr=0x%08lx\nmesh_hr=0x%08lx\n", static_cast<ULONG>(compute_hr),
                 static_cast<ULONG>(mesh_hr));
    auto root = create_root(provider.Get());
    ComPtr<ID3D12Device> owner;
    check(root->GetDevice(IID_PPV_ARGS(&owner)), "root GetDevice");
    D3D12_FEATURE_DATA_D3D12_OPTIONS5 dxr{};
    check(owner->CheckFeatureSupport(D3D12_FEATURE_D3D12_OPTIONS5, &dxr, sizeof(dxr)), "D3D12 options5");
    std::fprintf(receipt, "raytracing_tier=%u\n", static_cast<UINT>(dxr.RaytracingTier));
    ComPtr<ID3D12CommandSignature> rays, rays_with_root, different_stride;
    constexpr UINT ray_type = D3D12_INDIRECT_ARGUMENT_TYPE_DISPATCH_RAYS;
    static_assert(sizeof(D3D12_DISPATCH_RAYS_DESC) == 104);
    const HRESULT ray_hr = create_signature(provider.Get(), ray_type, 104, &rays);
    const HRESULT ray_root_hr = create_signature(provider.Get(), ray_type, 104, &rays_with_root, root.Get());
    const HRESULT other_hr = create_signature(provider.Get(), ray_type, 112, &different_stride, root.Get());
    std::fprintf(receipt,
                 "ray_null_root_hr=0x%08lx\nray_nonnull_root_hr=0x%08lx\n"
                 "ray_nonnull_root_112_hr=0x%08lx\n",
                 static_cast<ULONG>(ray_hr), static_cast<ULONG>(ray_root_hr), static_cast<ULONG>(other_hr));
    std::fflush(receipt);
    if ((SUCCEEDED(compute_hr) && !compute) || (SUCCEEDED(mesh_hr) && !mesh) || (SUCCEEDED(ray_hr) && !rays) ||
        (SUCCEEDED(ray_root_hr) && !rays_with_root)) {
        throw std::runtime_error("empty command signature");
    }
    ComPtr<IDXGIFactory4> factory;
    ComPtr<IDXGIAdapter1> adapter;
    DXGI_ADAPTER_DESC1 gpu{};
    check(CreateDXGIFactory1(IID_PPV_ARGS(&factory)), "CreateDXGIFactory1");
    check(factory->EnumAdapterByLuid(owner->GetAdapterLuid(), IID_PPV_ARGS(&adapter)), "EnumAdapterByLuid");
    check(adapter->GetDesc1(&gpu), "GetDesc1");
    std::fprintf(receipt, "adapter=%04x:%04x\n", gpu.VendorId, gpu.DeviceId);
    check(compute_hr, "Create compute command signature");
    dispatch_and_check(owner.Get(), root.Get(), compute.Get());
    std::fprintf(receipt, "verified_words=%u\n", WordCount);
    check(mesh_hr, "Create mesh command signature");
    check(ray_hr, "Create ray command signature");
    if (xbox)
        check(ray_root_hr, "Create ray command signature with root");
    else if (SUCCEEDED(ray_root_hr))
        throw std::runtime_error("desktop accepted unused root");
    if (SUCCEEDED(other_hr))
        throw std::runtime_error("unexpected unused root acceptance for stride 112");
}

int wmain(int argc, wchar_t** argv) {
    if (argc != 3 || (wcscmp(argv[1], L"desktop") && wcscmp(argv[1], L"xbox")))
        return 2;
    FILE* receipt{};
    if (_wfopen_s(&receipt, argv[2], L"wb"))
        return 2;
    int result = 0;
    try {
        run(wcscmp(argv[1], L"xbox") == 0, receipt);
    } catch (const std::exception& error) {
        std::fprintf(receipt, "error=%s\n", error.what());
        result = 1;
    }
    return fclose(receipt) ? 2 : result;
}
