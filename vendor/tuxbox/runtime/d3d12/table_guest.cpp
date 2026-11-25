// Exercise descriptor tables, wrapped heaps and null views on the selected GPU.
#include "compute_fixture.h"
#include "frame_cli.h"
#include "table.h"

namespace {
using namespace frame_abi;
using namespace compute_fixture;
using Pipeline = HRESULT (WINAPI*)(ID3D12Device*, const D3D12_COMPUTE_PIPELINE_STATE_DESC*,
                                   UINT, const void*, REFIID, void**);
using BindHeaps = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, ID3D12DescriptorHeap* const*);
using Root = void (WINAPI*)(ID3D12GraphicsCommandList*, ID3D12RootSignature*);
using Table = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, D3D12_GPU_DESCRIPTOR_HANDLE);
using GpuHandle = UINT64 (WINAPI*)(ID3D12DescriptorHeap*);

struct Heap {
    ComPtr<ID3D12DescriptorHeap> object;
    UINT stride;
    D3D12_CPU_DESCRIPTOR_HANDLE cpu(UINT index) const {
        return {method<CpuHandle>(object.Get(), 9)(object.Get()) + SIZE_T(index) * stride};
    }
    D3D12_GPU_DESCRIPTOR_HANDLE gpu(UINT index) const {
        return {method<GpuHandle>(object.Get(), 10)(object.Get()) + UINT64(index) * stride};
    }
};

Heap heap(ID3D12Device* device, D3D12_DESCRIPTOR_HEAP_TYPE type, UINT count, bool visible) {
    Heap result{};
    D3D12_DESCRIPTOR_HEAP_DESC desc{type, count,
        visible ? D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE : D3D12_DESCRIPTOR_HEAP_FLAG_NONE, 1};
    check(device->CreateDescriptorHeap(&desc, IID_PPV_ARGS(&result.object)), "descriptor heap");
    result.stride = device->GetDescriptorHandleIncrementSize(type);
    if (!result.stride) throw std::runtime_error("zero descriptor stride");
    return result;
}

ComPtr<ID3D12RootSignature> root_signature(ID3D12Device* device) {
    D3D12_DESCRIPTOR_RANGE ranges[] = {
        {D3D12_DESCRIPTOR_RANGE_TYPE_CBV, 1, 0, 0, 0}, {D3D12_DESCRIPTOR_RANGE_TYPE_SRV, 2, 0, 0, 1},
        {D3D12_DESCRIPTOR_RANGE_TYPE_UAV, 1, 0, 0, 3}, {D3D12_DESCRIPTOR_RANGE_TYPE_SAMPLER, 1, 0, 0, 0}};
    D3D12_ROOT_PARAMETER params[2]{};
    params[0].DescriptorTable = {3, ranges};
    params[1].DescriptorTable = {1, ranges + 3};
    static_assert(D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE == 0);
    D3D12_ROOT_SIGNATURE_DESC desc{2, params, 0, nullptr, D3D12_ROOT_SIGNATURE_FLAG_NONE};
    ComPtr<ID3DBlob> blob;
    check(D3D12SerializeRootSignature(&desc, D3D_ROOT_SIGNATURE_VERSION_1, &blob, nullptr), "serialize tables");
    ComPtr<ID3D12RootSignature> root;
    check(device->CreateRootSignature(0, blob->GetBufferPointer(), blob->GetBufferSize(),
                                      IID_PPV_ARGS(&root)), "table root signature");
    return root;
}

std::string run(const wchar_t* requested, bool) {
    DXGI_ADAPTER_DESC1 selected{};
    constexpr std::array<UINT, 18> creation{
        0x20004, 0, UINT_MAX, 0, 0, 0, UINT_MAX, UINT_MAX, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0};
    auto device = create_device(requested, selected, creation.data());
    // SetUAVs reads the device mode byte when choosing the descriptor-copy path.
    if (reinterpret_cast<const BYTE*>(device.Get())[0x18] != 1)
        throw std::runtime_error("incorrect guest descriptor mode");
    auto views = heap(device.Get(), D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 5, true);
    auto samplers = heap(device.Get(), D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER, 1, true);
    auto root = root_signature(device.Get());
    D3D12_COMPUTE_PIPELINE_STATE_DESC shader{};
    shader.pRootSignature = root.Get();
    shader.CS = {table_dxil, sizeof(table_dxil)};
    ComPtr<ID3D12PipelineState> pipeline;
    check(method<Pipeline>(device.Get(), 62)(device.Get(), &shader, 0, nullptr, IID_PPV_ARGS(&pipeline)), "table pipeline");

    std::array<UINT, 16> input;
    for (UINT i = 0; i < input.size(); ++i) input[i] = (i + 7) * (i + 11);
    std::array<UINT, 20> expected;
    expected.fill(0x1234abcd);
    constexpr UINT constants[] = {7, 91};
    constexpr UINT alternate[] = {13, 19};
    auto src = buffer(device.Get(), D3D12_HEAP_TYPE_UPLOAD, sizeof(input), D3D12_RESOURCE_STATE_GENERIC_READ);
    auto cb = buffer(device.Get(), D3D12_HEAP_TYPE_UPLOAD, 256, D3D12_RESOURCE_STATE_GENERIC_READ);
    auto alternate_cb = buffer(device.Get(), D3D12_HEAP_TYPE_UPLOAD, 256, D3D12_RESOURCE_STATE_GENERIC_READ);
    auto seed = buffer(device.Get(), D3D12_HEAP_TYPE_UPLOAD, sizeof(expected), D3D12_RESOURCE_STATE_GENERIC_READ);
    auto result = buffer(device.Get(), D3D12_HEAP_TYPE_DEFAULT, sizeof(expected), D3D12_RESOURCE_STATE_COPY_DEST,
                         D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
    auto readback = buffer(device.Get(), D3D12_HEAP_TYPE_READBACK, sizeof(expected), D3D12_RESOURCE_STATE_COPY_DEST);
    upload(src.Get(), input.data(), sizeof(input));
    upload(cb.Get(), constants, sizeof(constants));
    upload(alternate_cb.Get(), alternate, sizeof(alternate));
    upload(seed.Get(), expected.data(), sizeof(expected));

    // Start the table one entry into the heap. The unbound first entry and the
    // null texture entry use the same formats seen in Frosty's default views.
    D3D12_UNORDERED_ACCESS_VIEW_DESC uav{};
    uav.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
    uav.ViewDimension = D3D12_UAV_DIMENSION_TEXTURE2D;
    device->CreateUnorderedAccessView(nullptr, nullptr, &uav, views.cpu(0));
    D3D12_CONSTANT_BUFFER_VIEW_DESC cbv{cb->GetGPUVirtualAddress(), 256};
    device->CreateConstantBufferView(&cbv, views.cpu(1));
    D3D12_SHADER_RESOURCE_VIEW_DESC srv{};
    srv.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
    srv.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
    srv.Buffer.NumElements = UINT(input.size());
    srv.Buffer.StructureByteStride = sizeof(UINT);
    const auto input_srv = srv;
    device->CreateShaderResourceView(src.Get(), &srv, views.cpu(2));
    srv = {};
    srv.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
    srv.ViewDimension = D3D12_SRV_DIMENSION_TEXTURE2D;
    srv.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
    srv.Texture2D.MipLevels = 1;
    device->CreateShaderResourceView(nullptr, &srv, views.cpu(3));
    uav = {};
    uav.ViewDimension = D3D12_UAV_DIMENSION_BUFFER;
    uav.Buffer.FirstElement = 2;
    uav.Buffer.NumElements = UINT(input.size());
    uav.Buffer.StructureByteStride = sizeof(UINT);
    device->CreateUnorderedAccessView(result.Get(), nullptr, &uav, views.cpu(4));
    D3D12_SAMPLER_DESC sampler{};
    sampler.Filter = D3D12_FILTER_MIN_MAG_MIP_POINT;
    sampler.AddressU = sampler.AddressV = sampler.AddressW = D3D12_TEXTURE_ADDRESS_MODE_CLAMP;
    sampler.ComparisonFunc = D3D12_COMPARISON_FUNC_ALWAYS;
    sampler.MaxAnisotropy = 1;
    sampler.MaxLOD = D3D12_FLOAT32_MAX;
    device->CreateSampler(&sampler, samplers.cpu(0));
    auto rtv = heap(device.Get(), D3D12_DESCRIPTOR_HEAP_TYPE_RTV, 1, false);
    D3D12_RENDER_TARGET_VIEW_DESC rt{};
    rt.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
    rt.ViewDimension = D3D12_RTV_DIMENSION_TEXTURE2D;
    device->CreateRenderTargetView(nullptr, &rt, rtv.cpu(0));
    auto dsv = heap(device.Get(), D3D12_DESCRIPTOR_HEAP_TYPE_DSV, 1, false);
    D3D12_DEPTH_STENCIL_VIEW_DESC depth{};
    depth.Format = DXGI_FORMAT_D32_FLOAT;
    depth.ViewDimension = D3D12_DSV_DIMENSION_TEXTURE2D;
    device->CreateDepthStencilView(nullptr, &depth, dsv.cpu(0));

    ComPtr<ID3D12CommandQueue> queue;
    D3D12_COMMAND_QUEUE_DESC queue_desc{};
    queue_desc.Type = D3D12_COMMAND_LIST_TYPE_COMPUTE;
    check(device->CreateCommandQueue(&queue_desc, IID_PPV_ARGS(&queue)), "table queue");
    ComPtr<ID3D12CommandAllocator> allocator;
    check(device->CreateCommandAllocator(queue_desc.Type, IID_PPV_ARGS(&allocator)), "table allocator");
    const UINT list_desc[] = {D3D12_COMMAND_LIST_TYPE_COMPUTE, 0, 1, 0};
    ComPtr<ID3D12GraphicsCommandList> list;
    check(method<CreateList>(device.Get(), 73)(device.Get(), list_desc, allocator.Get(), nullptr,
                                               IID_PPV_ARGS(&list)), "table list");
    const auto bind = method<BindHeaps>(list.Get(), 32);
    DXGI_ADAPTER_DESC1 other_desc{};
    wchar_t pci[16];
    std::swprintf(pci, 16, L"%04x:%04x", selected.VendorId, selected.DeviceId);
    auto other = create_device(pci, other_desc);
    auto foreign = heap(other.Get(), D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER, 1, true);
    auto reject = [&](ID3D12DescriptorHeap* first, ID3D12DescriptorHeap* second) {
        check(list->Reset(allocator.Get(), nullptr), "reset invalid binding");
        ID3D12DescriptorHeap* bad[]{first, second};
        bind(list.Get(), second ? 2 : 1, bad);
        if (list->Close() != E_INVALIDARG) throw std::runtime_error("invalid heap binding accepted");
    };
    reject(views.object.Get(), foreign.object.Get());
    reject(views.object.Get(), views.object.Get());
    reject(rtv.object.Get(), nullptr);

    // t1 initially holds a null view. Replace it from a CPU heap after closing
    // the second list so the shader observes both the new view and its late write.
    alignas(256) std::array<BYTE, 256> backing{};
    D3D12_RESOURCE_DESC tex{};
    tex.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
    tex.Width = 1;
    tex.Height = 1;
    tex.DepthOrArraySize = 1;
    tex.MipLevels = 1;
    tex.SampleDesc.Count = 1;
    tex.Format = DXGI_FORMAT_R32_FLOAT;
    tex.Layout = static_cast<D3D12_TEXTURE_LAYOUT>(256);
    ComPtr<ID3D12Resource> placed;
    using Place = HRESULT (WINAPI*)(ID3D12Device*, UINT64, const D3D12_RESOURCE_DESC*,
                                    D3D12_RESOURCE_STATES, const D3D12_CLEAR_VALUE*, REFIID, void**);
    check(method<Place>(device.Get(), 44)(device.Get(), reinterpret_cast<UINT64>(backing.data()), &tex,
          D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE, nullptr, IID_PPV_ARGS(&placed)), "place sampled texture");
    constexpr auto kind = D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV;
    auto staging = heap(device.Get(), kind, 7, false);
    srv.Format = tex.Format;
    device->CreateShaderResourceView(placed.Get(), &srv, staging.cpu(0));
    // Xbox accepts a single 16-byte constant register as the view size.
    D3D12_CONSTANT_BUFFER_VIEW_DESC alternate_cbv{alternate_cb->GetGPUVirtualAddress(), 16};
    device->CreateConstantBufferView(&alternate_cbv, staging.cpu(1));
    device->CreateShaderResourceView(src.Get(), &input_srv, staging.cpu(2));
    auto shifted_uav = uav;
    shifted_uav.Buffer.FirstElement = 1;
    device->CreateUnorderedAccessView(result.Get(), nullptr, &shifted_uav, staging.cpu(3));
    device->CreateConstantBufferView(&cbv, staging.cpu(4));
    device->CreateShaderResourceView(nullptr, &srv, staging.cpu(5));
    device->CreateUnorderedAccessView(result.Get(), nullptr, &uav, staging.cpu(6));
    ComPtr<ID3D12Fence> fence;
    check(device->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&fence)), "table fence");
    for (UINT pass = 0; pass < 6; ++pass) {
        check(allocator->Reset(), "reset table allocator");
        check(list->Reset(allocator.Get(), pipeline.Get()), "reset valid table dispatch");
        if (pass)
            transition(list.Get(), result.Get(), D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_COPY_DEST);
        list->CopyBufferRegion(result.Get(), 0, seed.Get(), 0, sizeof(expected));
        transition(list.Get(), result.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
        ID3D12DescriptorHeap* valid[] = {views.object.Get(), samplers.object.Get()};
        bind(list.Get(), 2, valid);
        method<Root>(list.Get(), 33)(list.Get(), root.Get());
        auto set_table = method<Table>(list.Get(), 35);
        set_table(list.Get(), 0, views.gpu(1));
        set_table(list.Get(), 1, samplers.gpu(0));
        list->Dispatch(2, 1, 1);
        transition(list.Get(), result.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
        list->CopyBufferRegion(readback.Get(), 0, result.Get(), 0, sizeof(expected));
        check(list->Close(), "close table dispatch");
        if (pass == 1) {
            device->CopyDescriptorsSimple(1, views.cpu(3), staging.cpu(0), kind);
        } else if (pass == 2) {
            // Frosty's SetUAVs uses one contiguous destination and omits source sizes.
            const auto dst = views.cpu(4), src_handle = staging.cpu(3);
            const UINT one = 1;
            device->CopyDescriptors(1, &dst, &one, 1, &src_handle, nullptr, kind);
        } else if (pass == 3) {
            const auto dst = views.cpu(1);
            const UINT four = 4;
            const D3D12_CPU_DESCRIPTOR_HANDLE slots[]{staging.cpu(1), staging.cpu(2), staging.cpu(5), staging.cpu(6)};
            device->CopyDescriptors(1, &dst, &four, 4, slots, nullptr, kind);
        } else if (pass == 4) {
            // Scatter three descriptors across t1, u0 and b0, leaving t0 alone.
            const D3D12_CPU_DESCRIPTOR_HANDLE dst[]{views.cpu(4), views.cpu(1), views.cpu(3)};
            const D3D12_CPU_DESCRIPTOR_HANDLE slots[]{staging.cpu(3), staging.cpu(2), staging.cpu(0)};
            const UINT sizes[]{2, 0, 1};
            device->CopyDescriptors(3, dst, nullptr, 3, slots, sizes, kind);
        } else if (pass == 5) {
            const D3D12_CPU_DESCRIPTOR_HANDLE dst[]{views.cpu(2), views.cpu(1), views.cpu(4)};
            const D3D12_CPU_DESCRIPTOR_HANDLE slots[]{staging.cpu(2), staging.cpu(5), staging.cpu(6)};
            const UINT dst_sizes[]{2, 0, 1}, src_sizes[]{1, 2, 0};
            device->CopyDescriptors(3, dst, dst_sizes, 3, slots, src_sizes, kind);
        }
        constexpr UINT sampled[]{0, 0x3f800000, 0x40000000, 0, 0x40800000, 0};
        std::memcpy(backing.data(), &sampled[pass], sizeof(UINT));
        const UINT first = (pass == 2 || pass == 4) ? 1 : 2;
        const auto* params = pass == 3 ? alternate : constants;
        expected.fill(0x1234abcd);
        for (UINT i = 0; i < input.size(); ++i)
            expected[i + first] = input[i] * params[0] + params[1] + sampled[pass];
        ID3D12CommandList* batch[]{list.Get()};
        queue->ExecuteCommandLists(1, batch);
        check(queue->Signal(fence.Get(), pass + 1), "signal table dispatch");
        wait_for_fence(fence.Get(), pass + 1);
        D3D12_RANGE range{0, sizeof(expected)}, empty{};
        void* mapped{};
        check(readback->Map(0, &range, &mapped), "map table output");
        const bool match = std::memcmp(mapped, expected.data(), sizeof(expected)) == 0;
        readback->Unmap(0, &empty);
        if (!match) throw std::runtime_error("descriptor table values or guards differ at pass " + std::to_string(pass));
    }
    char receipt[150];
    std::snprintf(receipt, sizeof(receipt), "d3d12-tables PASS pci=%04x:%04x words=16/16 guards=4 heaps=2 rejected=3 placed-srv=16/16 range-copies=4 descriptor-mode=1",
                  selected.VendorId, selected.DeviceId);
    return receipt;
}
}

int wmain(int argc, wchar_t** argv) {
    return tuxbox_test::frame_main(argc, argv, "d3d12-tables", false, run);
}
