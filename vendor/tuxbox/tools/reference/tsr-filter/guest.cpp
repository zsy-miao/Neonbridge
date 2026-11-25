// Dispatch packaged TSR compute shaders through the Xbox device and read back every output.
#include "../visibility_fixture.h"

namespace {
using namespace visibility_fixture;
constexpr UINT width = 32, height = 32;
#ifdef TUXBOX_TSR_UPDATE_HISTORY
constexpr UINT srv_count = 17, uav_count = 8;
DXGI_FORMAT image_format(UINT slot, bool out) {
    return !out && (slot == 6 || slot == 16) ? DXGI_FORMAT_R32G32B32A32_UINT : DXGI_FORMAT_R32G32B32A32_FLOAT;
}
#else
constexpr UINT srv_count = 3, uav_count = 3;
DXGI_FORMAT image_format(UINT slot, bool) {
    return slot == 2 ? DXGI_FORMAT_R32_FLOAT : DXGI_FORMAT_R32G32B32A32_FLOAT;
}
#endif
constexpr UINT image_count = srv_count + uav_count;

void dispatch(const Context& gpu, ID3D12RootSignature* root, const fs::path& dir) {
    auto device = gpu.device.Get();
    auto list = gpu.list.Get();
    std::array<ComPtr<ID3D12Resource>, 2> cb;
    std::array<ComPtr<ID3D12Resource>, uav_count> readback;
    std::array<ComPtr<ID3D12Resource>, image_count> textures, seeds;
    D3D12_PLACED_SUBRESOURCE_FOOTPRINT layouts[image_count]{};
    for (UINT i = 0; i < cb.size(); ++i) {
        auto words = load(dir / ("b" + std::to_string(i)));
        cb[i] = input(device, words);
        D3D12_CONSTANT_BUFFER_VIEW_DESC desc{cb[i]->GetGPUVirtualAddress(), UINT(words.size())};
        device->CreateConstantBufferView(&desc, gpu.cpu(64 + i));
    }
    for (UINT i = 0; i < image_count; ++i) {
        const bool out = i >= srv_count;
        const UINT slot = out ? i - srv_count : i;
        const auto format = image_format(slot, out);
        const UINT bytes = format == DXGI_FORMAT_R32_FLOAT ? 4 : 16;
        auto data = load(dir / ((out ? "u" : "t") + std::to_string(slot)));
        if (data.size() != size_t(width) * height * bytes) throw std::runtime_error("invalid texture data");
        D3D12_HEAP_PROPERTIES heap{};
        heap.Type = D3D12_HEAP_TYPE_DEFAULT;
        heap.CreationNodeMask = heap.VisibleNodeMask = 1;
        D3D12_RESOURCE_DESC desc{};
        desc.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
        desc.Width = width;
        desc.Height = height;
        desc.Format = format;
        desc.DepthOrArraySize = desc.MipLevels = desc.SampleDesc.Count = 1;
        desc.Flags = out ? D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS : D3D12_RESOURCE_FLAG_NONE;
        check(device->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COPY_DEST,
                                              nullptr, IID_PPV_ARGS(&textures[i])), "create filter texture");
        UINT64 total{};
        device->GetCopyableFootprints(&desc, 0, 1, 0, &layouts[i], nullptr, nullptr, &total);
        Bytes rows(size_t(layouts[i].Footprint.RowPitch) * height);
        for (UINT y = 0; y < height; ++y)
            std::memcpy(rows.data() + layouts[i].Footprint.RowPitch * y, data.data() + width * bytes * y, width * bytes);
        seeds[i] = input(device, rows);
        D3D12_TEXTURE_COPY_LOCATION tex{}, linear{};
        tex.pResource = textures[i].Get();
        tex.Type = D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
        linear.pResource = seeds[i].Get();
        linear.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
        linear.PlacedFootprint = layouts[i];
        method<Copy>(list, 83)(list, &tex, 0, 0, 0, &linear, nullptr, 0);
        transition(list, tex.pResource, D3D12_RESOURCE_STATE_COPY_DEST,
                   out ? D3D12_RESOURCE_STATE_UNORDERED_ACCESS : D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
        if (out) {
            device->CreateUnorderedAccessView(tex.pResource, nullptr, nullptr, gpu.cpu(80 + slot));
            readback[slot] = buffer(device, D3D12_HEAP_TYPE_READBACK, total, D3D12_RESOURCE_STATE_COPY_DEST);
        } else {
            device->CreateShaderResourceView(tex.pResource, nullptr, gpu.cpu(slot));
        }
    }
#ifdef TUXBOX_TSR_UPDATE_HISTORY
    D3D12_DESCRIPTOR_HEAP_DESC sampler_desc{D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER, 16, D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, 1};
    ComPtr<ID3D12DescriptorHeap> samplers;
    check(device->CreateDescriptorHeap(&sampler_desc, IID_PPV_ARGS(&samplers)), "create sampler heap");
    const auto start = method<CpuHandle>(samplers.Get(), 9)(samplers.Get());
    const auto step = device->GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER);
    D3D12_SAMPLER_DESC sampler{};
    sampler.Filter = D3D12_FILTER_MIN_MAG_MIP_POINT;
    sampler.AddressU = sampler.AddressV = sampler.AddressW = D3D12_TEXTURE_ADDRESS_MODE_CLAMP;
    sampler.MaxLOD = D3D12_FLOAT32_MAX;
    sampler.MaxAnisotropy = 1;
    sampler.ComparisonFunc = D3D12_COMPARISON_FUNC_ALWAYS;
    for (UINT i = 0; i < sampler_desc.NumDescriptors; ++i)
        device->CreateSampler(&sampler, {start + UINT64(i) * step});
    ID3D12DescriptorHeap* heaps[]{gpu.views.Get(), samplers.Get()};
#else
    ID3D12DescriptorHeap* heaps[]{gpu.views.Get()};
#endif
    method<BindHeaps>(list, 32)(list, UINT(std::size(heaps)), heaps);
    method<BindRoot>(list, 33)(list, root);
    const auto first = method<GpuHandle>(heaps[0], 10)(heaps[0]);
    for (auto [parameter, slot] : {std::pair{0u, 0u}, {1u, 64u}, {3u, 80u}})
        method<SetTable>(list, 35)(list, parameter, {first + UINT64(slot) * gpu.stride});
#ifdef TUXBOX_TSR_UPDATE_HISTORY
    method<SetTable>(list, 35)(list, 2, {method<GpuHandle>(samplers.Get(), 10)(samplers.Get())});
#endif
    list->Dispatch(2, 2, 1);
    for (UINT slot = 0; slot < uav_count; ++slot) {
        const UINT i = srv_count + slot;
        transition(list, textures[i].Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
        D3D12_TEXTURE_COPY_LOCATION tex{}, linear{};
        tex.pResource = textures[i].Get();
        tex.Type = D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
        linear.pResource = readback[slot].Get();
        linear.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
        linear.PlacedFootprint = layouts[i];
        method<Copy>(list, 83)(list, &linear, 0, 0, 0, &tex, nullptr, 0);
    }
    gpu.submit();
    for (UINT slot = 0; slot < uav_count; ++slot) {
        const auto pitch = layouts[srv_count + slot].Footprint.RowPitch;
        const auto row = width * (image_format(slot, true) == DXGI_FORMAT_R32_FLOAT ? 4u : 16u);
        D3D12_RANGE range{0, size_t(pitch) * (height - 1) + row}, empty{};
        void* data{};
        check(readback[slot]->Map(0, &range, &data), "map filter output");
        std::ofstream out(dir / ("out" + std::to_string(slot)), std::ios::binary);
        for (UINT y = 0; y < height; ++y) out.write(static_cast<const char*>(data) + y * pitch, row);
        readback[slot]->Unmap(0, &empty);
        out.close();
        if (!out) throw std::runtime_error("cannot save filter image");
    }
}

std::string run_filter(const wchar_t* adapter, bool) {
    const auto temp = fs::absolute("temp");
    if (!SetEnvironmentVariableW(L"TMP", temp.c_str()) || !SetEnvironmentVariableW(L"TEMP", temp.c_str()))
        throw std::runtime_error("set shader cache directory");
    Context gpu(adapter, 96);
    auto shader = load("filter.dxbc");
    ComPtr<ID3D12RootSignature> root;
    check(gpu.device->CreateRootSignature(0, shader.data(), shader.size(), IID_PPV_ARGS(&root)), "embedded compute root");
    D3D12_COMPUTE_PIPELINE_STATE_DESC desc{};
    desc.pRootSignature = root.Get();
    desc.CS = {shader.data(), shader.size()};
    ComPtr<ID3D12PipelineState> pso;
    check(method<Pipeline>(gpu.device.Get(), 62)(gpu.device.Get(), &desc, 0, nullptr, IID_PPV_ARGS(&pso)), "Xbox filter PSO");
    std::ifstream cases("cases.txt");
    std::string name;
    UINT count = 0;
    while (cases >> name) {
        if (count++) {
            check(gpu.allocator->Reset(), "reset allocator");
            check(gpu.list->Reset(gpu.allocator.Get(), nullptr), "reset list");
        }
        gpu.list->SetPipelineState(pso.Get());
        dispatch(gpu, root.Get(), name);
    }
    if (!count) throw std::runtime_error("no filter cases");
    return "tsr-filter readbacks " + std::to_string(count);
}
}

int wmain(int argc, wchar_t** argv) {
    return tuxbox_test::frame_main(argc, argv, "tsr-filter", false, run_filter);
}
