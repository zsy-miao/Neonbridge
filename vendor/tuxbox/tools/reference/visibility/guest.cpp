#include "../compute_buffers.h"
#include "visibility_shaders.h"
#include <algorithm>
using namespace compute_buffers;

constexpr UINT width = 19, height = 13, pitch = 256;
constexpr DXGI_FORMAT format = DXGI_FORMAT_R32G32_UINT;
constexpr D3D12_RECT region{3, 2, 16, 10};

static void draw(ID3D12Device* device, bool meshInputs, bool clipped, FILE* out) {
    D3D12_DESCRIPTOR_RANGE range{D3D12_DESCRIPTOR_RANGE_TYPE_UAV, 1, 0, 0, 0};
    D3D12_ROOT_PARAMETER table{};
    table.ParameterType = D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE;
    table.DescriptorTable = {1, &range};
    table.ShaderVisibility = D3D12_SHADER_VISIBILITY_PIXEL;
    D3D12_ROOT_SIGNATURE_DESC layout{1, &table, 0, nullptr, D3D12_ROOT_SIGNATURE_FLAG_NONE};
    ComPtr<ID3DBlob> blob;
    check(D3D12SerializeRootSignature(&layout, D3D_ROOT_SIGNATURE_VERSION_1, &blob, nullptr), "serialize root");
    ComPtr<ID3D12RootSignature> root;
    check(device->CreateRootSignature(0, blob->GetBufferPointer(), blob->GetBufferSize(), IID_PPV_ARGS(&root)), "create root");
    D3D12_GRAPHICS_PIPELINE_STATE_DESC pso{};
    pso.VS = clipped ? D3D12_SHADER_BYTECODE{vs3050, sizeof(vs3050)} : meshInputs ? D3D12_SHADER_BYTECODE{vs2853, sizeof(vs2853)} : D3D12_SHADER_BYTECODE{vs2989, sizeof(vs2989)};
    pso.PS = clipped ? D3D12_SHADER_BYTECODE{ps3050, sizeof(ps3050)} : meshInputs ? D3D12_SHADER_BYTECODE{ps2853, sizeof(ps2853)} : D3D12_SHADER_BYTECODE{ps2989, sizeof(ps2989)};
    pso.pRootSignature = root.Get();
    pso.RasterizerState.FillMode = D3D12_FILL_MODE_SOLID;
    pso.RasterizerState.CullMode = D3D12_CULL_MODE_NONE;
    pso.RasterizerState.DepthClipEnable = TRUE;
    pso.PrimitiveTopologyType = D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE;
    pso.SampleDesc.Count = 1;
    pso.SampleMask = ~0u;
    ComPtr<ID3D12PipelineState> pipeline;
    check(device->CreateGraphicsPipelineState(&pso, IID_PPV_ARGS(&pipeline)), "visibility pixel pipeline");

    D3D12_HEAP_PROPERTIES props{};
    props.Type = D3D12_HEAP_TYPE_DEFAULT;
    props.VisibleNodeMask = props.CreationNodeMask = 1;
    D3D12_RESOURCE_DESC td{};
    td.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
    td.Format = format;
    td.Width = width;
    td.Height = height;
    td.DepthOrArraySize = td.MipLevels = td.SampleDesc.Count = 1;
    td.Flags = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
    ComPtr<ID3D12Resource> texture;
    check(device->CreateCommittedResource(&props, D3D12_HEAP_FLAG_NONE, &td, D3D12_RESOURCE_STATE_COPY_DEST, nullptr,
                                          IID_PPV_ARGS(&texture)), "visibility texture");
    D3D12_DESCRIPTOR_HEAP_DESC hd{D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 1, D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, 0};
    ComPtr<ID3D12DescriptorHeap> heap;
    check(device->CreateDescriptorHeap(&hd, IID_PPV_ARGS(&heap)), "visibility heap");
    device->CreateUnorderedAccessView(texture.Get(), nullptr, nullptr, heap->GetCPUDescriptorHandleForHeapStart());
    auto source = buffer(device, pitch * height, D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_STATE_GENERIC_READ);
    auto result = buffer(device, pitch * height, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
    std::array<UINT64, height * pitch / 8> initial{};
    for (UINT y = 0; y < height; ++y)
        for (UINT x = 0; x < width; ++x)
            initial[y * pitch / 8 + x] = ((x + y) % 3 == 0 ? 0xff01020300000000ull : 0x3580012300000000ull) |
                                         (0xabc000u + width * y + x);
    upload(source.Get(), initial.data(), sizeof(initial));

    ComPtr<ID3D12CommandQueue> queue;
    ComPtr<ID3D12CommandAllocator> alloc;
    ComPtr<ID3D12GraphicsCommandList> list;
    D3D12_COMMAND_QUEUE_DESC q{};
    check(device->CreateCommandQueue(&q, IID_PPV_ARGS(&queue)), "queue");
    check(device->CreateCommandAllocator(q.Type, IID_PPV_ARGS(&alloc)), "allocate commands");
    check(device->CreateCommandList(0, q.Type, alloc.Get(), pipeline.Get(), IID_PPV_ARGS(&list)), "create list");
    D3D12_TEXTURE_COPY_LOCATION tex{}, linear{};
    tex.pResource = texture.Get();
    linear.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
    linear.PlacedFootprint.Footprint = {format, width, height, 1, pitch};
    linear.pResource = source.Get();
    list->CopyTextureRegion(&tex, 0, 0, 0, &linear, nullptr);
    transition(list.Get(), texture.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
    ID3D12DescriptorHeap* heaps[] = {heap.Get()};
    list->SetDescriptorHeaps(1, heaps);
    list->SetGraphicsRootSignature(root.Get());
    list->SetGraphicsRootDescriptorTable(0, heap->GetGPUDescriptorHandleForHeapStart());
    D3D12_VIEWPORT vp{0, 0, float(width), float(height), 0, 1};
    list->RSSetViewports(1, &vp);
    list->RSSetScissorRects(1, &region);
    list->IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
    list->DrawInstanced(3, 3, 0, 0);
    transition(list.Get(), texture.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
    linear.pResource = result.Get();
    list->CopyTextureRegion(&linear, 0, 0, 0, &tex, nullptr);
    submit_and_wait(device, queue.Get(), list.Get());

    void* data;
    D3D12_RANGE read{0, sizeof(initial)};
    check(result->Map(0, &read, &data), "read back");
    auto words = static_cast<const UINT64*>(data);
    const UINT64 winner = 0x3f40000000000000ull | (meshInputs ? ((11u << 7) + 128u) | 3u : 11u);
    UINT matched = 0, changed = 0;
    for (LONG y = 0; y < LONG(height); ++y)
        for (LONG x = 0; x < LONG(width); ++x) {
            const UINT offset = y * pitch / 8 + x;
            const bool inside = clipped ? x >= 5 && x < 12 && y >= 4 && y < 8
                                        : x >= region.left && x < region.right && y >= region.top && y < region.bottom;
            const UINT64 expected = inside ? (std::max)(initial[offset], winner) : initial[offset];
            matched += words[offset] == expected;
            changed += words[offset] != initial[offset];
        }
    result->Unmap(0, nullptr);
    const UINT entry = clipped ? 3050 : meshInputs ? 2853 : 2989;
    std::fprintf(out, "entry_%u_matched=%u\nentry_%u_changed=%u\n", entry, matched, entry, changed);
    if (matched != width * height) throw std::runtime_error("visibility comparison failed");
}

int main() {
    FILE* out = nullptr;
    if (fopen_s(&out, "visibility.txt", "w")) return 1;
    try {
        ComPtr<ID3D12Device> device;
        check(D3D12CreateDevice(nullptr, D3D_FEATURE_LEVEL_12_0, IID_PPV_ARGS(&device)), "device");
        ComPtr<IDXGIFactory4> factory;
        ComPtr<IDXGIAdapter1> adapter;
        DXGI_ADAPTER_DESC1 info{};
        check(CreateDXGIFactory1(IID_PPV_ARGS(&factory)), "factory");
        check(factory->EnumAdapterByLuid(device->GetAdapterLuid(), IID_PPV_ARGS(&adapter)), "adapter");
        check(adapter->GetDesc1(&info), "adapter description");
        std::fprintf(out, "adapter=%04x:%04x\n", info.VendorId, info.DeviceId);
        D3D12_FEATURE_DATA_D3D12_OPTIONS9 opts{};
        check(device->CheckFeatureSupport(D3D12_FEATURE_D3D12_OPTIONS9, &opts, sizeof(opts)), "OPTIONS9");
        std::fprintf(out, "typed_int64_atomics=%u\n", unsigned(opts.AtomicInt64OnTypedResourceSupported));
        if (!opts.AtomicInt64OnTypedResourceSupported) throw std::runtime_error("typed integer atomics unavailable");
        draw(device.Get(), false, false, out);
        draw(device.Get(), true, false, out);
        draw(device.Get(), true, true, out);
    } catch (const std::exception& error) {
        std::fprintf(out, "error=%s\n", error.what());
        std::fclose(out);
        return 1;
    }
    std::fclose(out);
    return 0;
}
