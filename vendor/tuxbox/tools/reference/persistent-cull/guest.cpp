// Run the original persistent Nanite compute program against GPU-visible work queues.
#include "mesh_fixture.h"
#include "../visibility_fixture.h"
#include <filesystem>
#include <fstream>
#include <vector>

namespace {
using namespace mesh_fixture;
namespace fs = std::filesystem;
using Bytes = std::vector<BYTE>;
using Pipeline = HRESULT(WINAPI*)(ID3D12Device*, const D3D12_COMPUTE_PIPELINE_STATE_DESC*, UINT,
                                  const void*, REFIID, void**);

Bytes load(const fs::path& file) {
    std::ifstream stream(file, std::ios::binary);
    if (!stream) throw std::runtime_error("missing " + file.string());
    return {std::istreambuf_iterator<char>(stream), std::istreambuf_iterator<char>()};
}

ComPtr<ID3D12Resource> input(ID3D12Device* device, const Bytes& data) {
    auto result = buffer(device, D3D12_HEAP_TYPE_UPLOAD, data.size(), D3D12_RESOURCE_STATE_GENERIC_READ);
    upload(result.Get(), data.data(), data.size());
    return result;
}

void dispatch(const Context& ctx, ID3D12RootSignature* root, const fs::path& dir, UINT groups) {
    auto dev = ctx.device.Get();
    auto list = ctx.list.Get();
    const auto constants = load(dir / "b0");
    auto cb = input(dev, constants);
    D3D12_CONSTANT_BUFFER_VIEW_DESC cbv{cb->GetGPUVirtualAddress(), static_cast<UINT>(constants.size())};
    dev->CreateConstantBufferView(&cbv, ctx.cpu(64));
    const bool has_hzb = fs::exists(dir / "hzb");
    ComPtr<ID3D12Resource> depth, depth_seed;
    ComPtr<ID3D12DescriptorHeap> samplers;
    if (has_hzb) visibility_fixture::hzb_texture(ctx, dir, depth, depth_seed, samplers, 5);
    std::array<ComPtr<ID3D12Resource>, 7> srvs;
    constexpr UINT strides[]{16, 16, 16, 0, 800, 0, 8};
    for (UINT i = 0; i < srvs.size(); ++i) {
        const auto data = load(dir / ("t" + std::to_string(i)));
        const UINT stride = strides[i];
        srvs[i] = input(dev, data);
        D3D12_SHADER_RESOURCE_VIEW_DESC view{};
        view.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
        view.Format = stride ? DXGI_FORMAT_UNKNOWN : DXGI_FORMAT_R32_TYPELESS;
        view.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
        view.Buffer.NumElements = static_cast<UINT>(data.size()) / (stride ? stride : 4);
        view.Buffer.StructureByteStride = stride;
        view.Buffer.Flags = stride ? D3D12_BUFFER_SRV_FLAG_NONE : D3D12_BUFFER_SRV_FLAG_RAW;
        dev->CreateShaderResourceView(srvs[i].Get(), &view, ctx.cpu(has_hzb && i >= 5 ? i + 1 : i));
    }

    std::array<ComPtr<ID3D12Resource>, 6> uavs, uploads, reads;
    std::array<UINT, 6> sizes{};
    for (UINT i = 0; i < uavs.size(); ++i) {
        const auto data = load(dir / ("u" + std::to_string(i)));
        sizes[i] = static_cast<UINT>(data.size());
        uploads[i] = input(dev, data);
        uavs[i] = buffer(dev, D3D12_HEAP_TYPE_DEFAULT, sizes[i], D3D12_RESOURCE_STATE_COPY_DEST,
                         D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
        reads[i] = buffer(dev, D3D12_HEAP_TYPE_READBACK, sizes[i], D3D12_RESOURCE_STATE_COPY_DEST);
        list->CopyBufferRegion(uavs[i].Get(), 0, uploads[i].Get(), 0, sizes[i]);
        transition(list, uavs[i].Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
        const UINT stride = i == 0 ? 12 : i == 1 ? 44 : 0;
        const bool raw = i >= 2 && i <= 4;
        D3D12_UNORDERED_ACCESS_VIEW_DESC view{};
        view.ViewDimension = D3D12_UAV_DIMENSION_BUFFER;
        view.Format = raw ? DXGI_FORMAT_R32_TYPELESS : stride ? DXGI_FORMAT_UNKNOWN : DXGI_FORMAT_R32_UINT;
        view.Buffer.NumElements = sizes[i] / (stride ? stride : 4);
        view.Buffer.StructureByteStride = stride;
        view.Buffer.Flags = raw ? D3D12_BUFFER_UAV_FLAG_RAW : D3D12_BUFFER_UAV_FLAG_NONE;
        dev->CreateUnorderedAccessView(uavs[i].Get(), nullptr, &view, ctx.cpu(80 + i));
    }
    ID3D12DescriptorHeap* heaps[]{ctx.views.Get(), samplers.Get()};
    method<BindHeaps>(list, 32)(list, has_hzb ? 2 : 1, heaps);
    method<BindRoot>(list, 33)(list, root);
    auto set = method<SetTable>(list, 35);
    const auto base = method<GpuHandle>(ctx.views.Get(), 10)(ctx.views.Get());
    for (const auto [table, slot] : {std::pair{0u, 0u}, {1u, 64u}, {3u, 80u}})
        set(list, table, {base + UINT64(slot) * ctx.stride});
    if (has_hzb) set(list, 2, {method<GpuHandle>(samplers.Get(), 10)(samplers.Get())});
    list->Dispatch(groups, 1, 1);
    for (UINT i = 0; i < uavs.size(); ++i) {
        transition(list, uavs[i].Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
        list->CopyBufferRegion(reads[i].Get(), 0, uavs[i].Get(), 0, sizes[i]);
    }
    ctx.submit();
    for (UINT i = 0; i < reads.size(); ++i) {
        void* data = nullptr;
        D3D12_RANGE range{0, sizes[i]}, written{};
        check(reads[i]->Map(0, &range, &data), "map persistent culling queue");
        std::ofstream out(dir / ("out" + std::to_string(i)), std::ios::binary);
        out.write(static_cast<const char*>(data), sizes[i]);
        reads[i]->Unmap(0, &written);
        if (!out) throw std::runtime_error("could not write GPU result");
    }
}

std::string run(const wchar_t* adapter, bool) {
    const auto temp = fs::absolute("temp");
    if (!SetEnvironmentVariableW(L"TMP", temp.c_str()) || !SetEnvironmentVariableW(L"TEMP", temp.c_str()))
        throw std::runtime_error("cannot set shader cache directory");
    Context ctx(adapter, 96);
    const auto shader = load("original.dxbc");
    ComPtr<ID3D12RootSignature> root;
    check(ctx.device->CreateRootSignature(0, shader.data(), shader.size(), IID_PPV_ARGS(&root)), "embedded root");
    D3D12_COMPUTE_PIPELINE_STATE_DESC desc{};
    desc.pRootSignature = root.Get();
    desc.CS = {shader.data(), shader.size()};
    ComPtr<ID3D12PipelineState> pso;
    check(method<Pipeline>(ctx.device.Get(), 62)(ctx.device.Get(), &desc, 0, nullptr, IID_PPV_ARGS(&pso)),
          "Xbox persistent culling pipeline");
    std::ifstream manifest("cases.txt");
    std::string name;
    UINT groups, count = 0;
    while (manifest >> name >> groups) {
        if (count++) {
            check(ctx.allocator->Reset(), "reset allocator");
            check(ctx.list->Reset(ctx.allocator.Get(), nullptr), "reset command list");
        }
        ctx.list->SetPipelineState(pso.Get());
        dispatch(ctx, root.Get(), name, groups);
    }
    if (!manifest.eof() || !count) throw std::runtime_error("invalid queue case manifest");
    char line[112];
    std::snprintf(line, sizeof(line), "persistent-cull PASS pci=%04x:%04x cases=%u", ctx.adapter.VendorId,
                  ctx.adapter.DeviceId, count);
    return line;
}
}
int wmain(int argc, wchar_t** argv) {
    return tuxbox_test::frame_main(argc, argv, "persistent-cull", false, run);
}
