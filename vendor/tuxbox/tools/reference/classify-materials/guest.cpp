// Dispatch the packaged material classifier over an input visibility image.
#include "../visibility_fixture.h"

namespace {
using namespace visibility_fixture;

void classify(const Context& gpu, ID3D12RootSignature* root, const fs::path& dir, UINT gx, UINT gy, Image image) {
    auto* device = gpu.device.Get();
    auto* list = gpu.list.Get();
    auto constants = input(device, load(dir / "b0"));
    D3D12_CONSTANT_BUFFER_VIEW_DESC cb{constants->GetGPUVirtualAddress(), 256};
    device->CreateConstantBufferView(&cb, gpu.cpu(64));
    std::array<ComPtr<ID3D12Resource>, 6> sources;
    for (UINT i : {0u, 1u, 2u, 3u, 5u}) {
        const auto data = load(dir / ("t" + std::to_string(i)));
        sources[i] = input(device, data);
        srv(gpu, i, sources[i].Get(), static_cast<UINT>(data.size()), i < 2 ? 16u : 0u);
    }
    const auto pixels = load(dir / "t4");
    if (pixels.size() != size_t(image.row_size()) * image.height)
        throw std::runtime_error("invalid input image");
    Bytes rows(size_t(image.pitch()) * image.height);
    for (UINT y = 0; y < image.height; ++y)
        std::memcpy(rows.data() + y * image.pitch(), pixels.data() + y * image.row_size(), image.row_size());
    auto pixels_upload = input(device, rows);
    D3D12_HEAP_PROPERTIES heap{};
    heap.Type = D3D12_HEAP_TYPE_DEFAULT;
    heap.CreationNodeMask = heap.VisibleNodeMask = 1;
    D3D12_RESOURCE_DESC desc{};
    desc.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
    desc.Format = image.format();
    desc.Width = image.width;
    desc.Height = image.height;
    desc.MipLevels = desc.DepthOrArraySize = desc.SampleDesc.Count = 1;
    check(device->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COPY_DEST,
                                          nullptr, IID_PPV_ARGS(&sources[4])), "create input image");
    D3D12_TEXTURE_COPY_LOCATION src{}, dst{};
    src.pResource = pixels_upload.Get();
    src.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
    src.PlacedFootprint.Footprint = {image.format(), image.width, image.height, 1, image.pitch()};
    dst.pResource = sources[4].Get();
    dst.Type = D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
    method<Copy>(list, 83)(list, &dst, 0, 0, 0, &src, nullptr, 0);
    transition(list, sources[4].Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
    device->CreateShaderResourceView(sources[4].Get(), nullptr, gpu.cpu(4));

    std::array<ComPtr<ID3D12Resource>, 2> uploads, outputs, reads;
    std::array<UINT, 2> sizes{};
    for (UINT i = 0; i < outputs.size(); ++i) {
        auto data = load(dir / ("u" + std::to_string(i)));
        sizes[i] = static_cast<UINT>(data.size());
        uploads[i] = input(device, data);
        outputs[i] = buffer(device, D3D12_HEAP_TYPE_DEFAULT, sizes[i], D3D12_RESOURCE_STATE_COPY_DEST,
                            D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
        reads[i] = buffer(device, D3D12_HEAP_TYPE_READBACK, sizes[i], D3D12_RESOURCE_STATE_COPY_DEST);
        list->CopyBufferRegion(outputs[i].Get(), 0, uploads[i].Get(), 0, sizes[i]);
        transition(list, outputs[i].Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
        D3D12_UNORDERED_ACCESS_VIEW_DESC view{};
        view.ViewDimension = D3D12_UAV_DIMENSION_BUFFER;
        view.Format = i ? DXGI_FORMAT_UNKNOWN : DXGI_FORMAT_R32_UINT;
        view.Buffer.NumElements = sizes[i] / 4;
        view.Buffer.StructureByteStride = i ? 4 : 0;
        device->CreateUnorderedAccessView(outputs[i].Get(), nullptr, &view, gpu.cpu(80 + i));
    }
    ID3D12DescriptorHeap* heaps[]{gpu.views.Get()};
    method<BindHeaps>(list, 32)(list, 1, heaps);
    method<BindRoot>(list, 33)(list, root);
    auto base = method<GpuHandle>(gpu.views.Get(), 10)(gpu.views.Get());
    for (auto [table, slot] : {std::pair{0u, 0u}, {1u, 64u}, {3u, 80u}})
        method<SetTable>(list, 35)(list, table, {base + UINT64(slot) * gpu.stride});
    list->Dispatch(gx, gy, 1);
    for (UINT i = 0; i < outputs.size(); ++i) {
        transition(list, outputs[i].Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
        list->CopyBufferRegion(reads[i].Get(), 0, outputs[i].Get(), 0, sizes[i]);
    }
    gpu.submit();
    for (UINT i = 0; i < reads.size(); ++i) {
        D3D12_RANGE read{0, sizes[i]}, empty{};
        void* mapped = nullptr;
        check(reads[i]->Map(0, &read, &mapped), "map material buffer");
        std::ofstream file(dir / ("out" + std::to_string(i)), std::ios::binary);
        file.write(static_cast<const char*>(mapped), sizes[i]);
        reads[i]->Unmap(0, &empty);
        if (!file) throw std::runtime_error("cannot save material buffer");
    }
}

std::string run_cases(const wchar_t* adapter, bool) {
    const auto temp = fs::absolute("temp");
    if (!SetEnvironmentVariableW(L"TMP", temp.c_str()) || !SetEnvironmentVariableW(L"TEMP", temp.c_str()))
        throw std::runtime_error("cannot set shader cache directory");
    Context gpu(adapter, 96);
    auto shader = load("classify.dxbc");
    ComPtr<ID3D12RootSignature> root;
    check(gpu.device->CreateRootSignature(0, shader.data(), shader.size(), IID_PPV_ARGS(&root)), "embedded root");
    D3D12_COMPUTE_PIPELINE_STATE_DESC desc{};
    desc.pRootSignature = root.Get();
    desc.CS = {shader.data(), shader.size()};
    ComPtr<ID3D12PipelineState> pso;
    check(method<Pipeline>(gpu.device.Get(), 62)(gpu.device.Get(), &desc, 0, nullptr, IID_PPV_ARGS(&pso)), "Xbox compute PSO");
    std::ifstream manifest("cases.txt");
    std::string name;
    UINT gx, gy, width, height, count = 0;
    while (manifest >> name >> gx >> gy >> width >> height) {
        if (count++) {
            check(gpu.allocator->Reset(), "reset allocator");
            check(gpu.list->Reset(gpu.allocator.Get(), nullptr), "reset list");
        }
        gpu.list->SetPipelineState(pso.Get());
        classify(gpu, root.Get(), name, gx, gy, {width, height, 2});
    }
    if (!manifest.eof() || !count) throw std::runtime_error("invalid material case manifest");
    char line[96];
    std::snprintf(line, sizeof(line), "classify-materials PASS pci=%04x:%04x cases=%u",
                  gpu.adapter.VendorId, gpu.adapter.DeviceId, count);
    return line;
}
}
int wmain(int argc, wchar_t** argv) {
    return tuxbox_test::frame_main(argc, argv, "classify-materials", false, run_cases);
}
