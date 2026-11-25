#include "context.h"
#include <array>

using namespace shader_control;
constexpr UINT extent = 256;
constexpr UINT groups = extent / 8;
constexpr UINT bytes = extent * extent * 4 * sizeof(float);
constexpr auto format = DXGI_FORMAT_R32G32B32A32_FLOAT;

static ComPtr<ID3D12Resource> texture(ID3D12Device *device, D3D12_RESOURCE_STATES state) {
    D3D12_HEAP_PROPERTIES heap{};
    heap.Type = D3D12_HEAP_TYPE_DEFAULT;
    heap.CreationNodeMask = heap.VisibleNodeMask = 1;
    D3D12_RESOURCE_DESC desc{};
    desc.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
    desc.Width = desc.Height = extent;
    desc.DepthOrArraySize = desc.MipLevels = desc.SampleDesc.Count = 1;
    desc.Format = format;
    desc.Flags = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
    ComPtr<ID3D12Resource> result;
    check(
        device->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, state, nullptr, IID_PPV_ARGS(&result)),
        "texture");
    return result;
}

static void run(Context &ctx, UINT resolution) {
    auto *device = ctx.device.Get();
    D3D12_DESCRIPTOR_RANGE ranges[] = {{D3D12_DESCRIPTOR_RANGE_TYPE_CBV, 1, 0, 0, 0},
                                       {D3D12_DESCRIPTOR_RANGE_TYPE_SRV, 4, 0, 0, 1},
                                       {D3D12_DESCRIPTOR_RANGE_TYPE_UAV, 1, 0, 0, 5}};
    D3D12_ROOT_PARAMETER parameter{};
    parameter.ParameterType = D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE;
    parameter.DescriptorTable = {3, ranges};
    D3D12_STATIC_SAMPLER_DESC sampler{};
    sampler.Filter = D3D12_FILTER_MIN_MAG_MIP_POINT;
    sampler.AddressU = sampler.AddressV = sampler.AddressW = D3D12_TEXTURE_ADDRESS_MODE_CLAMP;
    sampler.ComparisonFunc = D3D12_COMPARISON_FUNC_ALWAYS;
    sampler.MaxLOD = D3D12_FLOAT32_MAX;
    D3D12_ROOT_SIGNATURE_DESC root_desc{1, &parameter, 1, &sampler};
    ComPtr<ID3DBlob> blob;
    check(D3D12SerializeRootSignature(&root_desc, D3D_ROOT_SIGNATURE_VERSION_1, &blob, nullptr), "serialize root");
    ComPtr<ID3D12RootSignature> root;
    check(device->CreateRootSignature(0, blob->GetBufferPointer(), blob->GetBufferSize(), IID_PPV_ARGS(&root)),
          "root");
    auto shader = read_shader("481");
    D3D12_COMPUTE_PIPELINE_STATE_DESC pipeline_desc{};
    pipeline_desc.pRootSignature = root.Get();
    pipeline_desc.CS = {shader.data(), shader.size()};
    ComPtr<ID3D12PipelineState> pipeline;
    check(device->CreateComputePipelineState(&pipeline_desc, IID_PPV_ARGS(&pipeline)), "pipeline");
    ComPtr<ID3D12CommandAllocator> allocator;
    ComPtr<ID3D12GraphicsCommandList> list;
    check(device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&allocator)), "allocator");
    check(device->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, allocator.Get(), pipeline.Get(),
                                    IID_PPV_ARGS(&list)),
          "list");
    D3D12_DESCRIPTOR_HEAP_DESC heap_desc{D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 6,
                                         D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE};
    ComPtr<ID3D12DescriptorHeap> heap;
    check(device->CreateDescriptorHeap(&heap_desc, IID_PPV_ARGS(&heap)), "heap");
    auto slot = [&](UINT i) {
        auto h = heap->GetCPUDescriptorHandleForHeapStart();
        h.ptr += i * device->GetDescriptorHandleIncrementSize(heap_desc.Type);
        return h;
    };

    auto constants = buffer(device, 512, D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_STATE_GENERIC_READ);
    std::array<UINT, 68> words{};
    words[13] = resolution;
    words[18] = groups;
    words[33] = groups * groups;
    words[34] = 1;
    words[64] = 0x3f800000;
    upload(constants.Get(), words.data(), sizeof(words));
    D3D12_CONSTANT_BUFFER_VIEW_DESC cb{constants->GetGPUVirtualAddress(), 512};
    device->CreateConstantBufferView(&cb, slot(0));
    auto source = texture(device, D3D12_RESOURCE_STATE_COPY_DEST);
    auto output = texture(device, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
    auto input = buffer(device, bytes, D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_STATE_GENERIC_READ);
    std::vector<float> rgb(extent * extent * 4);
    for (UINT y = 0; y < extent; ++y)
        for (UINT x = 0; x < extent; ++x) {
            float value = 1.f + float((x / 8) * 187 + (y / 8) * 83 + (x % 8) * 11 + (y % 8) * 41) / 2000.f;
            for (UINT c = 0; c < 4; ++c)
                rgb[(y * extent + x) * 4 + c] = value * (float(c + 1) / 16.f);
        }
    upload(input.Get(), rgb.data(), bytes);
    D3D12_TEXTURE_COPY_LOCATION image{}, footprint{};
    image.pResource = source.Get();
    image.Type = D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
    footprint.pResource = input.Get();
    footprint.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
    footprint.PlacedFootprint.Footprint = {format, extent, extent, 1, extent * 16};
    list->CopyTextureRegion(&image, 0, 0, 0, &footprint, nullptr);
    transition(list.Get(), source.Get(), D3D12_RESOURCE_STATE_COPY_DEST,
               D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
    D3D12_SHADER_RESOURCE_VIEW_DESC srv{};
    srv.Format = format;
    srv.ViewDimension = D3D12_SRV_DIMENSION_TEXTURE2D;
    srv.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
    srv.Texture2D.MipLevels = 1;
    for (UINT i : {1u, 3u, 4u})
        device->CreateShaderResourceView(source.Get(), &srv, slot(i));
    srv.Format = DXGI_FORMAT_UNKNOWN;
    srv.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
    srv.Buffer = {0, 64, 4, D3D12_BUFFER_SRV_FLAG_NONE};
    device->CreateShaderResourceView(input.Get(), &srv, slot(2));
    D3D12_UNORDERED_ACCESS_VIEW_DESC uav{};
    uav.Format = format;
    uav.ViewDimension = D3D12_UAV_DIMENSION_TEXTURE2D;
    device->CreateUnorderedAccessView(output.Get(), nullptr, &uav, slot(5));
    ID3D12DescriptorHeap *heaps[] = {heap.Get()};
    list->SetDescriptorHeaps(1, heaps);
    list->SetComputeRootSignature(root.Get());
    list->SetComputeRootDescriptorTable(0, heap->GetGPUDescriptorHandleForHeapStart());
    list->Dispatch(groups, groups, 1);
    auto readback = buffer(device, bytes, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
    transition(list.Get(), output.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
    image.pResource = output.Get();
    footprint.pResource = readback.Get();
    list->CopyTextureRegion(&footprint, 0, 0, 0, &image, nullptr);
    submit_and_wait(device, ctx.queue.Get(), list.Get());
    write_buffer(readback.Get(), std::to_string(resolution), bytes);
}

int main() {
    FILE *receipt{};
    if (fopen_s(&receipt, "adapter.txt", "w") != 0)
        return 1;
    try {
        Context ctx(receipt);
        for (UINT resolution : {8u, 6u, 4u})
            run(ctx, resolution);
    } catch (const std::exception &e) {
        std::fprintf(receipt, "%s\n", e.what());
        std::fclose(receipt);
        return 1;
    }
    std::fclose(receipt);
    return 0;
}
