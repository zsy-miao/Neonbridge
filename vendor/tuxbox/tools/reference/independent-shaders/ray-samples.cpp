// Give each texel, mip, slice and channel a distinct value so the CPU can check
// the address selected by a translated material image instruction.
#include "context.h"
using namespace shader_control;

struct SampleTexture {
    ComPtr<ID3D12Resource> image, staging;
};

static SampleTexture make_texture(ID3D12Device *dev, ID3D12GraphicsCommandList *list,
                                  D3D12_CPU_DESCRIPTOR_HANDLE handle, D3D12_SRV_DIMENSION view, bool packed) {
    const bool volume = view == D3D12_SRV_DIMENSION_TEXTURE3D;
    D3D12_RESOURCE_DESC desc{};
    desc.Dimension = volume ? D3D12_RESOURCE_DIMENSION_TEXTURE3D : D3D12_RESOURCE_DIMENSION_TEXTURE2D;
    desc.Width = desc.Height = 8;
    desc.DepthOrArraySize = volume                                       ? 4
                            : view == D3D12_SRV_DIMENSION_TEXTURECUBE    ? 6
                            : view == D3D12_SRV_DIMENSION_TEXTURE2DARRAY ? 3
                                                                         : 1;
    desc.MipLevels = 4;
    desc.Format = packed ? DXGI_FORMAT_R32G32B32A32_UINT : DXGI_FORMAT_R32G32B32A32_FLOAT;
    desc.SampleDesc.Count = 1;
    D3D12_HEAP_PROPERTIES hp{};
    hp.Type = D3D12_HEAP_TYPE_DEFAULT;
    hp.CreationNodeMask = hp.VisibleNodeMask = 1;
    SampleTexture texture;
    check(dev->CreateCommittedResource(&hp, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COPY_DEST, nullptr,
                                       IID_PPV_ARGS(&texture.image)),
          "sample texture");
    UINT subresources = desc.MipLevels * (volume ? 1 : desc.DepthOrArraySize);
    std::vector<D3D12_PLACED_SUBRESOURCE_FOOTPRINT> footprints(subresources);
    UINT64 total = 0;
    dev->GetCopyableFootprints(&desc, 0, subresources, 0, footprints.data(), nullptr, nullptr, &total);
    std::vector<BYTE> data(static_cast<size_t>(total));
    for (UINT sub = 0; sub < subresources; ++sub) {
        const auto &f = footprints[sub].Footprint;
        for (UINT z = 0; z < f.Depth; ++z)
            for (UINT y = 0; y < f.Height; ++y)
                for (UINT x = 0; x < f.Width; ++x)
                    for (UINT c = 0; c < 4; ++c) {
                        UINT tag = 1 + x + 16 * y + 128 * (volume ? z : sub / 4) + 1024 * (sub % 4) + 4096 * c;
                        SIZE_T offset = footprints[sub].Offset + (z * f.Height + y) * f.RowPitch + x * 16 + c * 4;
                        float value = static_cast<float>(tag);
                        std::memcpy(data.data() + offset,
                                    packed ? static_cast<void *>(&tag) : static_cast<void *>(&value), 4);
                    }
    }
    texture.staging = buffer(dev, total, D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_STATE_GENERIC_READ);
    upload(texture.staging.Get(), data.data(), data.size());
    for (UINT sub = 0; sub < subresources; ++sub) {
        D3D12_TEXTURE_COPY_LOCATION src{}, dst{};
        src.pResource = texture.staging.Get();
        src.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
        src.PlacedFootprint = footprints[sub];
        dst.pResource = texture.image.Get();
        dst.SubresourceIndex = sub;
        list->CopyTextureRegion(&dst, 0, 0, 0, &src, nullptr);
    }
    transition(list, texture.image.Get(), D3D12_RESOURCE_STATE_COPY_DEST,
               D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
    D3D12_SHADER_RESOURCE_VIEW_DESC srv{};
    srv.Format = desc.Format;
    srv.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
    srv.ViewDimension = view;
    if (volume)
        srv.Texture3D.MipLevels = 4;
    else if (view == D3D12_SRV_DIMENSION_TEXTURECUBE)
        srv.TextureCube.MipLevels = 4;
    else if (view == D3D12_SRV_DIMENSION_TEXTURE2DARRAY) {
        srv.Texture2DArray.MipLevels = 4;
        srv.Texture2DArray.ArraySize = desc.DepthOrArraySize;
    } else
        srv.Texture2D.MipLevels = 4;
    dev->CreateShaderResourceView(texture.image.Get(), &srv, handle);
    return texture;
}

static void run(FILE *receipt) {
    Context host(receipt);
    auto *dev = host.device.Get();
    D3D12_DESCRIPTOR_RANGE range{D3D12_DESCRIPTOR_RANGE_TYPE_SRV, 12, 0, 0, 0};
    D3D12_ROOT_PARAMETER roots[2]{};
    roots[0].ParameterType = D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE;
    roots[0].DescriptorTable = {1, &range};
    roots[1].ParameterType = D3D12_ROOT_PARAMETER_TYPE_UAV;
    roots[1].Descriptor = {0, 0};
    D3D12_STATIC_SAMPLER_DESC samplers[6]{};
    for (UINT s = 0; s < 6; ++s) {
        auto &desc = samplers[s];
        desc.Filter = D3D12_FILTER_MIN_MAG_MIP_POINT;
        desc.AddressU = desc.AddressV = desc.AddressW = D3D12_TEXTURE_ADDRESS_MODE_CLAMP;
        desc.MaxAnisotropy = 1;
        desc.ComparisonFunc = D3D12_COMPARISON_FUNC_ALWAYS;
        desc.MaxLOD = D3D12_FLOAT32_MAX;
        desc.ShaderRegister = s;
    }
    D3D12_ROOT_SIGNATURE_DESC rs{2, roots, 6, samplers, D3D12_ROOT_SIGNATURE_FLAG_NONE};
    ComPtr<ID3DBlob> blob;
    check(D3D12SerializeRootSignature(&rs, D3D_ROOT_SIGNATURE_VERSION_1, &blob, nullptr), "sample root signature");
    ComPtr<ID3D12RootSignature> root;
    check(dev->CreateRootSignature(0, blob->GetBufferPointer(), blob->GetBufferSize(), IID_PPV_ARGS(&root)),
          "sample root");
    auto code = read_shader("ray-samples");
    D3D12_COMPUTE_PIPELINE_STATE_DESC desc{};
    desc.pRootSignature = root.Get();
    desc.CS = {code.data(), code.size()};
    ComPtr<ID3D12PipelineState> pipeline;
    check(dev->CreateComputePipelineState(&desc, IID_PPV_ARGS(&pipeline)), "sample pipeline");
    ComPtr<ID3D12CommandAllocator> alloc;
    ComPtr<ID3D12GraphicsCommandList> list;
    check(dev->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&alloc)), "sample allocator");
    check(dev->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, alloc.Get(), pipeline.Get(), IID_PPV_ARGS(&list)),
          "sample list");
    D3D12_DESCRIPTOR_HEAP_DESC hd{D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 12, D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                                  0};
    ComPtr<ID3D12DescriptorHeap> heap;
    check(dev->CreateDescriptorHeap(&hd, IID_PPV_ARGS(&heap)), "sample descriptors");
    const auto first = heap->GetCPUDescriptorHandleForHeapStart();
    const auto step = dev->GetDescriptorHandleIncrementSize(hd.Type);
    const auto slot = [&](UINT i) { return D3D12_CPU_DESCRIPTOR_HANDLE{first.ptr + i * step}; };
    const auto volume = make_texture(dev, list.Get(), slot(4), D3D12_SRV_DIMENSION_TEXTURE3D, false);
    const auto array = make_texture(dev, list.Get(), slot(5), D3D12_SRV_DIMENSION_TEXTURE2DARRAY, false);
    const auto cube = make_texture(dev, list.Get(), slot(6), D3D12_SRV_DIMENSION_TEXTURECUBE, false);
    const auto packed = make_texture(dev, list.Get(), slot(7), D3D12_SRV_DIMENSION_TEXTURE2D, true);
    const auto gradient = make_texture(dev, list.Get(), slot(11), D3D12_SRV_DIMENSION_TEXTURE2D, false);
    constexpr UINT samples = 48, size = samples * sizeof(UINT) * 4;
    auto output = buffer(dev, size, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                         D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
    auto readback = buffer(dev, size, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
    ID3D12DescriptorHeap *heaps[]{heap.Get()};
    list->SetDescriptorHeaps(1, heaps);
    list->SetComputeRootSignature(root.Get());
    list->SetComputeRootDescriptorTable(0, heap->GetGPUDescriptorHandleForHeapStart());
    list->SetComputeRootUnorderedAccessView(1, output->GetGPUVirtualAddress());
    list->Dispatch(samples, 1, 1);
    transition(list.Get(), output.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
    list->CopyBufferRegion(readback.Get(), 0, output.Get(), 0, size);
    submit_and_wait(dev, host.queue.Get(), list.Get());
    write_buffer(readback.Get(), "ray-samples", size);
}

int main() {
    FILE *receipt{};
    if (fopen_s(&receipt, "result.txt", "wb"))
        return 2;
    int status = 0;
    try {
        run(receipt);
    } catch (const std::exception &e) {
        std::fprintf(receipt, "error=%s\n", e.what());
        status = 1;
    }
    return fclose(receipt) ? 2 : status;
}
