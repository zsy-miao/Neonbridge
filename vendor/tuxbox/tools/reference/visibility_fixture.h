// Dispatch packaged compute shaders that update a Nanite visibility texture.
#pragma once
#include "mesh_fixture.h"
#include "frame_cli.h"
#include <filesystem>
#include <fstream>
#include <vector>

namespace visibility_fixture {
using namespace mesh_fixture;
namespace fs = std::filesystem;
using Pipeline = HRESULT(WINAPI*)(ID3D12Device*, const D3D12_COMPUTE_PIPELINE_STATE_DESC*, UINT,
                                  const void*, REFIID, void**);
using Bytes = std::vector<BYTE>;

Bytes load(const fs::path& path) {
    std::ifstream file(path, std::ios::binary);
    if (!file) throw std::runtime_error("cannot read " + path.string());
    return {std::istreambuf_iterator<char>(file), std::istreambuf_iterator<char>()};
}

ComPtr<ID3D12Resource> input(ID3D12Device* device, const Bytes& bytes) {
    auto result = buffer(device, D3D12_HEAP_TYPE_UPLOAD, bytes.size(), D3D12_RESOURCE_STATE_GENERIC_READ);
    upload(result.Get(), bytes.data(), bytes.size());
    return result;
}

void srv(const Context& gpu, UINT index, ID3D12Resource* resource, UINT bytes, UINT stride) {
    D3D12_SHADER_RESOURCE_VIEW_DESC desc{};
    desc.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
    desc.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
    desc.Format = stride ? DXGI_FORMAT_UNKNOWN : DXGI_FORMAT_R32_TYPELESS;
    desc.Buffer.NumElements = bytes / (stride ? stride : 4);
    desc.Buffer.StructureByteStride = stride;
    desc.Buffer.Flags = stride ? D3D12_BUFFER_SRV_FLAG_NONE : D3D12_BUFFER_SRV_FLAG_RAW;
    gpu.device->CreateShaderResourceView(resource, &desc, gpu.cpu(index));
}

void uav(const Context& gpu, UINT index, ID3D12Resource* resource, UINT bytes, UINT stride) {
    D3D12_UNORDERED_ACCESS_VIEW_DESC desc{};
    desc.ViewDimension = D3D12_UAV_DIMENSION_BUFFER;
    desc.Format = stride ? DXGI_FORMAT_UNKNOWN : DXGI_FORMAT_R32_TYPELESS;
    desc.Buffer.NumElements = bytes / (stride ? stride : 4);
    desc.Buffer.StructureByteStride = stride;
    desc.Buffer.Flags = stride ? D3D12_BUFFER_UAV_FLAG_NONE : D3D12_BUFFER_UAV_FLAG_RAW;
    gpu.device->CreateUnorderedAccessView(resource, nullptr, &desc, gpu.cpu(index));
}

struct Image {
    UINT width, height, words;
    DXGI_FORMAT format() const { return words == 2 ? DXGI_FORMAT_R32G32_UINT : DXGI_FORMAT_R32_UINT; }
    UINT row_size() const { return width * words * sizeof(UINT); }
    UINT pitch() const { return (row_size() + 255u) & ~255u; }
};

// Four R32_FLOAT mips, each with a 256-byte row pitch and a 512-byte
// aligned start. Keep the upload allocation alive until the list completes.
void hzb_texture(const Context& gpu, const fs::path& dir, ComPtr<ID3D12Resource>& texture,
                 ComPtr<ID3D12Resource>& upload_buffer, ComPtr<ID3D12DescriptorHeap>& samplers, UINT slot = 3) {
    auto bytes = load(dir / "hzb");
    if (bytes.size() != (64 + 16 + 4 + 1) * sizeof(float)) throw std::runtime_error("invalid HZB mips");
    D3D12_RESOURCE_DESC desc{};
    desc.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
    desc.Format = DXGI_FORMAT_R32_FLOAT;
    desc.Width = desc.Height = 8;
    desc.MipLevels = 4;
    desc.DepthOrArraySize = desc.SampleDesc.Count = 1;
    D3D12_HEAP_PROPERTIES props{};
    props.Type = D3D12_HEAP_TYPE_DEFAULT;
    props.CreationNodeMask = props.VisibleNodeMask = 1;
    check(gpu.device->CreateCommittedResource(&props, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COPY_DEST,
                                              nullptr, IID_PPV_ARGS(&texture)), "create HZB");
    Bytes rows(4096);
    size_t src = 0;
    UINT offset = 0;
    for (UINT dim = 8; dim; dim >>= 1) {
        for (UINT y = 0; y < dim; ++y) {
            std::memcpy(rows.data() + offset + y * 256, bytes.data() + src, dim * 4);
            src += dim * 4;
        }
        offset += std::max(2u, dim) * 256;
    }
    upload_buffer = input(gpu.device.Get(), rows);
    offset = 0;
    for (UINT level = 0; level < 4; ++level) {
        UINT dim = 8u >> level;
        D3D12_TEXTURE_COPY_LOCATION dst{}, linear{};
        dst.pResource = texture.Get();
        dst.Type = D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
        dst.SubresourceIndex = level;
        linear.pResource = upload_buffer.Get();
        linear.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
        linear.PlacedFootprint.Offset = offset;
        linear.PlacedFootprint.Footprint = {desc.Format, dim, dim, 1, 256};
        method<Copy>(gpu.list.Get(), 83)(gpu.list.Get(), &dst, 0, 0, 0, &linear, nullptr, 0);
        offset += std::max(2u, dim) * 256;
    }
    transition(gpu.list.Get(), texture.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
    gpu.device->CreateShaderResourceView(texture.Get(), nullptr, gpu.cpu(slot));
    D3D12_DESCRIPTOR_HEAP_DESC heap{D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER, 16, D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, 1};
    check(gpu.device->CreateDescriptorHeap(&heap, IID_PPV_ARGS(&samplers)), "HZB sampler heap");
    D3D12_SAMPLER_DESC sampler{};
    sampler.Filter = D3D12_FILTER_MIN_MAG_MIP_POINT;
    sampler.AddressU = sampler.AddressV = sampler.AddressW = D3D12_TEXTURE_ADDRESS_MODE_CLAMP;
    sampler.MaxLOD = D3D12_FLOAT32_MAX;
    sampler.MaxAnisotropy = 1;
    sampler.ComparisonFunc = D3D12_COMPARISON_FUNC_ALWAYS;
    gpu.device->CreateSampler(&sampler, {method<CpuHandle>(samplers.Get(), 9)(samplers.Get())});
}

void check_case(const Context& gpu, ID3D12RootSignature* root, const fs::path& dir, UINT groups, Image image, bool micropoly) {
    auto* device = gpu.device.Get();
    auto* list = gpu.list.Get();
    const auto constants = load(dir / "b0");
    auto cb = input(device, constants);
    D3D12_CONSTANT_BUFFER_VIEW_DESC cbv{cb->GetGPUVirtualAddress(), static_cast<UINT>(constants.size())};
    device->CreateConstantBufferView(&cbv, gpu.cpu(64));
    std::array<ComPtr<ID3D12Resource>, 5> sources;
    // The rasterizer reads the instance rows, page, visible-cluster list and
    // packed view. InstanceCull has a fifth input and three writable queues.
    const UINT source_count = micropoly ? 4 : 5;
    const bool has_hzb = fs::exists(dir / "hzb");
    const bool post = fs::exists(dir / "t6");
    ComPtr<ID3D12Resource> hzb, hzb_seed;
    ComPtr<ID3D12DescriptorHeap> samplers;
    if (has_hzb) hzb_texture(gpu, dir, hzb, hzb_seed, samplers);
    for (UINT i = 0; i < source_count; ++i) {
        const auto bytes = load(dir / ("t" + std::to_string(i)));
        sources[i] = input(device, bytes);
        const UINT strides[]{16, micropoly ? 0u : 16u, micropoly ? 0u : 16u, 800, 0};
        srv(gpu, has_hzb && i >= 3 ? i + 1 : i, sources[i].Get(), static_cast<UINT>(bytes.size()), strides[i]);
    }
    ComPtr<ID3D12Resource> hidden, args_input;
    if (post) {
        const auto ids = load(dir / "t6"), args = load(dir / "t7");
        hidden = input(device, ids);
        args_input = input(device, args);
        srv(gpu, 6, hidden.Get(), static_cast<UINT>(ids.size()), 8);
        D3D12_SHADER_RESOURCE_VIEW_DESC view{};
        view.Format = DXGI_FORMAT_R32_UINT;
        view.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
        view.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
        view.Buffer.NumElements = static_cast<UINT>(args.size() / 4);
        device->CreateShaderResourceView(args_input.Get(), &view, gpu.cpu(7));
    }
    const auto pixels = load(dir / "u0");
    if (pixels.size() != size_t(image.row_size()) * image.height)
        throw std::runtime_error("invalid visibility image input");
    Bytes rows(image.pitch() * image.height);
    for (UINT y = 0; y < image.height; ++y)
        std::memcpy(rows.data() + y * image.pitch(), pixels.data() + y * image.row_size(), image.row_size());
    auto image_seed = input(device, rows);
    auto image_result = buffer(device, D3D12_HEAP_TYPE_READBACK, rows.size(), D3D12_RESOURCE_STATE_COPY_DEST);
    D3D12_HEAP_PROPERTIES props{};
    props.Type = D3D12_HEAP_TYPE_DEFAULT;
    props.CreationNodeMask = props.VisibleNodeMask = 1;
    D3D12_RESOURCE_DESC desc{};
    desc.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
    desc.Width = image.width;
    desc.Height = image.height;
    desc.DepthOrArraySize = desc.MipLevels = desc.SampleDesc.Count = 1;
    desc.Format = image.format();
    desc.Flags = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
    ComPtr<ID3D12Resource> visibility;
    check(device->CreateCommittedResource(&props, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COPY_DEST,
                                          nullptr, IID_PPV_ARGS(&visibility)), "create visibility image");
    D3D12_TEXTURE_COPY_LOCATION texture{}, linear{};
    texture.pResource = visibility.Get();
    texture.Type = D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
    linear.pResource = image_seed.Get();
    linear.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
    linear.PlacedFootprint.Footprint = {image.format(), image.width, image.height, 1, image.pitch()};
    auto copy = method<Copy>(list, 83);
    copy(list, &texture, 0, 0, 0, &linear, nullptr, 0);
    transition(list, visibility.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
    device->CreateUnorderedAccessView(visibility.Get(), nullptr, nullptr, gpu.cpu(80));

    std::array<ComPtr<ID3D12Resource>, 5> seeds, outputs, readbacks;
    std::array<UINT, 5> sizes{};
    const UINT output_count = has_hzb && !post ? 5 : (micropoly ? 0 : 3);
    for (UINT i = 0; i < output_count; ++i) {
        const auto seed = load(dir / ("u" + std::to_string(i + 1)));
        sizes[i] = static_cast<UINT>(seed.size());
        seeds[i] = input(device, seed);
        outputs[i] = buffer(device, D3D12_HEAP_TYPE_DEFAULT, sizes[i], D3D12_RESOURCE_STATE_COPY_DEST,
                            D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
        readbacks[i] = buffer(device, D3D12_HEAP_TYPE_READBACK, sizes[i], D3D12_RESOURCE_STATE_COPY_DEST);
        list->CopyBufferRegion(outputs[i].Get(), 0, seeds[i].Get(), 0, sizes[i]);
        transition(list, outputs[i].Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
        if (has_hzb && !post && i == 1) {
            D3D12_UNORDERED_ACCESS_VIEW_DESC args{};
            args.ViewDimension = D3D12_UAV_DIMENSION_BUFFER;
            args.Format = DXGI_FORMAT_R32_UINT;
            args.Buffer.NumElements = sizes[i] / 4;
            device->CreateUnorderedAccessView(outputs[i].Get(), nullptr, &args, gpu.cpu(81 + i));
        } else {
            const UINT raw_slot = has_hzb && !post ? 2u : 0u;
            const UINT stride = has_hzb && !post && i == 0 ? 8 : (i == raw_slot ? 0 : sizes[i]);
            uav(gpu, 81 + i, outputs[i].Get(), sizes[i], stride);
        }
    }
    ID3D12DescriptorHeap* heaps[]{gpu.views.Get(), samplers.Get()};
    method<BindHeaps>(list, 32)(list, has_hzb ? 2 : 1, heaps);
    method<BindRoot>(list, 33)(list, root);
    const auto set_table = method<SetTable>(list, 35);
    const auto handle = method<GpuHandle>(gpu.views.Get(), 10)(gpu.views.Get());
    for (auto [parameter, index] : {std::pair{0u, 0u}, {1u, 64u}, {3u, 80u}})
        set_table(list, parameter, {handle + UINT64(index) * gpu.stride});
    if (has_hzb) set_table(list, 2, {method<GpuHandle>(samplers.Get(), 10)(samplers.Get())});
    list->Dispatch(groups, 1, 1);
    for (UINT i = 0; i < output_count; ++i) {
        transition(list, outputs[i].Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
        list->CopyBufferRegion(readbacks[i].Get(), 0, outputs[i].Get(), 0, sizes[i]);
    }
    transition(list, visibility.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
    linear.pResource = image_result.Get();
    copy(list, &linear, 0, 0, 0, &texture, nullptr, 0);
    gpu.submit();
    D3D12_RANGE span{0, rows.size()}, unread{};
    void* mapped = nullptr;
    check(image_result->Map(0, &span, &mapped), "map visibility image");
    std::ofstream out(dir / "out0", std::ios::binary);
    for (UINT y = 0; y < image.height; ++y)
        out.write(static_cast<const char*>(mapped) + y * image.pitch(), image.row_size());
    image_result->Unmap(0, &unread);
    if (!out) throw std::runtime_error("cannot save visibility image");
    for (UINT i = 0; i < output_count; ++i) {
        D3D12_RANGE read{0, sizes[i]}, empty{};
        void* values = nullptr;
        check(readbacks[i]->Map(0, &read, &values), "map InstanceCull result");
        std::ofstream file(dir / ("out" + std::to_string(i + 1)), std::ios::binary);
        file.write(static_cast<const char*>(values), sizes[i]);
        readbacks[i]->Unmap(0, &empty);
        if (!file) throw std::runtime_error("cannot save InstanceCull result");
    }
}

std::string run(const wchar_t* adapter, bool micropoly) {
    const auto temp = fs::absolute("temp");
    if (!SetEnvironmentVariableW(L"TMP", temp.c_str()) || !SetEnvironmentVariableW(L"TEMP", temp.c_str()))
        throw std::runtime_error("set shader cache directory");
    Context gpu(adapter, 96);
    auto shader = load(micropoly ? "micropoly.dxbc" : "instance.dxbc");
    ComPtr<ID3D12RootSignature> root;
    check(gpu.device->CreateRootSignature(0, shader.data(), shader.size(), IID_PPV_ARGS(&root)),
          "create embedded compute root signature");
    D3D12_COMPUTE_PIPELINE_STATE_DESC desc{};
    desc.pRootSignature = root.Get();
    desc.CS = {shader.data(), shader.size()};
    ComPtr<ID3D12PipelineState> pipeline;
    check(method<Pipeline>(gpu.device.Get(), 62)(gpu.device.Get(), &desc, 0, nullptr, IID_PPV_ARGS(&pipeline)),
          "create Xbox compute pipeline");
    Image image{};
    std::ifstream layout("image.txt");
    if (!(layout >> image.width >> image.height >> image.words) || !image.width || !image.height ||
        image.width > 4096 || image.height > 4096 ||
        (image.words != 1 && image.words != 2))
        throw std::runtime_error("invalid visibility image layout");
    std::ifstream manifest("cases.txt");
    UINT groups, count = 0;
    std::string name;
    while (manifest >> name >> groups) {
        if (count++) {
            check(gpu.allocator->Reset(), "reset allocator");
            check(gpu.list->Reset(gpu.allocator.Get(), nullptr), "reset command list");
        }
        gpu.list->SetPipelineState(pipeline.Get());
        check_case(gpu, root.Get(), name, groups, image, micropoly);
    }
    if (!manifest.eof() || !count) throw std::runtime_error("invalid compute cases");
    char result[120];
    std::snprintf(result, sizeof(result), "%s PASS pci=%04x:%04x cases=%u", micropoly ? "micropoly" : "instance-cull",
                  gpu.adapter.VendorId, gpu.adapter.DeviceId, count);
    return result;
}
} // namespace visibility_fixture
