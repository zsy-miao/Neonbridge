// Dispatch the packaged virtual-shadow-map shaders using their embedded root signatures.
#include "../independent-shaders/context.h"

using namespace shader_control;
#include "../page-list/instance_hzb.h"

static std::vector<char> read_file(const std::string &name) {
    std::ifstream file(name, std::ios::binary);
    if (!file) throw std::runtime_error(name);
    return {std::istreambuf_iterator<char>(file), {}};
}

static void dispatch(Context &ctx, const std::string &name, UINT entry, UINT groups_x, UINT groups_y,
                     UINT srv_stride, const std::vector<UINT> &strides) {
    auto dev = ctx.device.Get();
    auto cooked = read_file(std::to_string(entry) + ".dxbc"), code = read_file(std::to_string(entry) + ".dxil");
    ComPtr<ID3D12RootSignature> root;
    check(dev->CreateRootSignature(0, cooked.data(), cooked.size(), IID_PPV_ARGS(&root)), "VSM shader root");
    D3D12_COMPUTE_PIPELINE_STATE_DESC pd{};
    pd.pRootSignature = root.Get();
    pd.CS = {code.data(), code.size()};
    ComPtr<ID3D12PipelineState> pipeline;
    check(dev->CreateComputePipelineState(&pd, IID_PPV_ARGS(&pipeline)), "VSM compute pipeline");
    ComPtr<ID3D12CommandAllocator> alloc;
    ComPtr<ID3D12GraphicsCommandList> list;
    check(dev->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&alloc)), "VSM allocator");
    check(dev->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, alloc.Get(), pipeline.Get(), IID_PPV_ARGS(&list)), "VSM list");
    D3D12_DESCRIPTOR_HEAP_DESC hd{D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 96, D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, 0};
    ComPtr<ID3D12DescriptorHeap> heap;
    check(dev->CreateDescriptorHeap(&hd, IID_PPV_ARGS(&heap)), "VSM heap");
    const UINT step = dev->GetDescriptorHandleIncrementSize(hd.Type);
    auto cpu = [&](UINT i) { return D3D12_CPU_DESCRIPTOR_HANDLE{heap->GetCPUDescriptorHandleForHeapStart().ptr + i * step}; };
    auto gpu = [&](UINT i) { return D3D12_GPU_DESCRIPTOR_HANDLE{heap->GetGPUDescriptorHandleForHeapStart().ptr + i * step}; };
    auto source = [&](const std::vector<char> &bytes) {
        auto resource = buffer(dev, static_cast<UINT>(bytes.size()), D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_STATE_GENERIC_READ);
        upload(resource.Get(), bytes.data(), bytes.size());
        return resource;
    };
    // A cached shadow page also reads the previous frame's tables and
    // metadata. The other page selectors read one table and one CBV.
    const bool cached = entry == 1228, instance = entry == 3403, cluster = entry == 1793;
    const bool no_hzb = entry == 3413 || entry == 3416 || entry == 3426 || entry == 3433 || entry == 3437 || entry == 3440 || entry == 3468 || entry == 3474;
    const bool post_instance = entry == 3397 || entry == 3398 || entry == 3412 || entry == 3422 || entry == 3432 || entry == 3454 || entry == 3457 || entry == 3470;
    const bool single_instance = no_hzb || entry == 3396 || entry == 3399 || entry == 3438 || entry == 3430 || entry == 3449 || entry == 3451 || entry == 3456 || entry == 3458 || post_instance;
    const bool depth_cull = instance || cluster || (single_instance && !no_hzb);
    const bool small_reflection = entry == 582 || entry == 587 || entry == 592 || entry == 600 || entry == 606;
    const bool reflection = small_reflection || entry == 576 || entry == 580 || entry == 579 || entry == 589 || entry == 601;
    const bool cards = entry == 1243 || entry == 1257;
    InstanceDepth hzb;
    InstanceDepth reflection_textures[3];
    const std::vector<UINT> srvs = small_reflection ? std::vector<UINT>{4,4,4,4} : reflection ? std::vector<UINT>{4,4,4,4,4} : no_hzb ? std::vector<UINT>{16,16,16,800,4} : post_instance ? std::vector<UINT>{16,16,16,4,800,4,8,4} : single_instance ? std::vector<UINT>{16,16,16,4,800,4} : cluster ? std::vector<UINT>{16,16,16,4,4,4,4,16,4,800,4}
                                           : instance ? std::vector<UINT>{16,16,16,4,4,4,4,800,8,4}
                                           : cards ? std::vector<UINT>{16, 4, 4, 4}
                                           : (entry == 248 || entry == 249 || entry == 259) ? std::vector<UINT>{4, 8}
                                           : (entry == 507 || entry == 509 || entry == 512 || entry == 513) ? std::vector<UINT>{4, 20}
                                           : entry == 517 ? std::vector<UINT>{4, 4}
                                           : entry == 1016 ? std::vector<UINT>{8, 4, 4}
                                           : entry == 2138 ? std::vector<UINT>{16}
                                           : cached ? std::vector<UINT>{4, 4, 4, 8, 4, 4}
                                           : entry == 3477 ? std::vector<UINT>{16, 800} : std::vector<UINT>{srv_stride};
    std::vector<ComPtr<ID3D12Resource>> bound;
    for (UINT i = 0; i < (cached || instance || cluster || cards || reflection ? 2u : 1u); ++i) {
        auto bytes = read_file(name + (cached || instance || cluster || cards || reflection ? ".b" + std::to_string(i) : ".cbv"));
        bound.push_back(source(bytes));
        D3D12_CONSTANT_BUFFER_VIEW_DESC cb{bound.back()->GetGPUVirtualAddress(), static_cast<UINT>(bytes.size())};
        dev->CreateConstantBufferView(&cb, cpu(64 + i));
    }
    for (UINT i = 0; i < srvs.size(); ++i) {
        auto bytes = read_file(name + (srvs.size() > 1 ? ".t" + std::to_string(i) : ".srv"));
        if (reflection && i < (small_reflection ? 2u : 3u)) { reflection_textures[i].bind(dev, list.Get(), cpu(i), bytes); continue; }
        if (depth_cull && i == 3) { hzb.bind(dev, list.Get(), cpu(i), bytes); continue; }
        bound.push_back(source(bytes));
        D3D12_SHADER_RESOURCE_VIEW_DESC view{};
        view.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
        view.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
        view.Buffer.NumElements = static_cast<UINT>(bytes.size()) / srvs[i];
        view.Buffer.StructureByteStride = srvs[i];
        if (post_instance && i == 7) {
            view.Buffer.StructureByteStride = 0;
            view.Format = DXGI_FORMAT_R32_UINT;
        }
        if ((reflection && i >= (small_reflection ? 2u : 3u)) || entry == 1019 || ((entry == 507 || entry == 509 || entry == 512 || entry == 513) && i == 0)) {
            view.Buffer.StructureByteStride = 0;
            view.Format = DXGI_FORMAT_R32_UINT;
        }
        if ((cluster && (i == 8 || i == 10)) || (single_instance && !no_hzb && i == 5) || (no_hzb && i == 4)) {
            view.Buffer.StructureByteStride = 0;
            view.Format = DXGI_FORMAT_R32_TYPELESS;
            view.Buffer.Flags = D3D12_BUFFER_SRV_FLAG_RAW;
        }
        dev->CreateShaderResourceView(bound.back().Get(), &view, cpu(i));
    }
    std::vector<ComPtr<ID3D12Resource>> inputs(strides.size()), outputs(strides.size()), readbacks(strides.size());
    std::vector<UINT> sizes(strides.size());
    for (UINT i = 0; i < strides.size(); ++i) {
        auto bytes = read_file(name + ".u" + std::to_string(i));
        sizes[i] = static_cast<UINT>(bytes.size());
        if (single_instance && i < 2) {
            const UINT pixel_bytes = i == 0 ? 16 : 4;
            const UINT row_size = 9 * pixel_bytes, pitch = (row_size + 255u) & ~255u;
            if (bytes.size() != row_size * 9u) throw std::runtime_error("invalid instance texture input");
            std::vector<char> padded(pitch * 9u);
            for (UINT y = 0; y < 9; ++y) std::memcpy(padded.data() + y * pitch, bytes.data() + y * row_size, row_size);
            inputs[i] = source(padded);
            sizes[i] = pitch * 9u;
            readbacks[i] = buffer(dev, sizes[i], D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
            D3D12_HEAP_PROPERTIES prop{};
            prop.Type = D3D12_HEAP_TYPE_DEFAULT;
            prop.CreationNodeMask = prop.VisibleNodeMask = 1;
            D3D12_RESOURCE_DESC desc{};
            desc.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
            desc.Width = desc.Height = 9;
            desc.DepthOrArraySize = desc.MipLevels = desc.SampleDesc.Count = 1;
            desc.Format = i == 0 ? DXGI_FORMAT_R32G32B32A32_UINT : DXGI_FORMAT_R32_UINT;
            desc.Flags = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
            check(dev->CreateCommittedResource(&prop, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COPY_DEST, nullptr,
                                              IID_PPV_ARGS(&outputs[i])), "visibility texture");
            D3D12_TEXTURE_COPY_LOCATION src{}, dst{};
            src.pResource = inputs[i].Get();
            src.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
            src.PlacedFootprint.Footprint = {desc.Format, 9, 9, 1, pitch};
            dst.pResource = outputs[i].Get();
            list->CopyTextureRegion(&dst, 0, 0, 0, &src, nullptr);
            transition(list.Get(), outputs[i].Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
            dev->CreateUnorderedAccessView(outputs[i].Get(), nullptr, nullptr, cpu(80 + i));
            continue;
        }
        inputs[i] = source(bytes);
        outputs[i] = buffer(dev, sizes[i], D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
        readbacks[i] = buffer(dev, sizes[i], D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
        list->CopyBufferRegion(outputs[i].Get(), 0, inputs[i].Get(), 0, sizes[i]);
        transition(list.Get(), outputs[i].Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
        D3D12_UNORDERED_ACCESS_VIEW_DESC u{};
        u.ViewDimension = D3D12_UAV_DIMENSION_BUFFER;
        u.Buffer.NumElements = sizes[i] / strides[i];
        if (reflection && i == 1) {
            D3D12_UNORDERED_ACCESS_VIEW_DESC u{};
            u.ViewDimension = D3D12_UAV_DIMENSION_BUFFER; u.Format = DXGI_FORMAT_R32G32_UINT;
            u.Buffer.NumElements = sizes[i] / 8;
            dev->CreateUnorderedAccessView(outputs[i].Get(), nullptr, &u, cpu(80 + i));
            continue;
        }
        if (reflection && i == 2) {
            D3D12_UNORDERED_ACCESS_VIEW_DESC u{};
            u.ViewDimension = D3D12_UAV_DIMENSION_BUFFER; u.Format = DXGI_FORMAT_R32G32B32A32_UINT;
            u.Buffer.NumElements = sizes[i] / 16;
            dev->CreateUnorderedAccessView(outputs[i].Get(), nullptr, &u, cpu(80 + i));
            continue;
        }
        const bool typed = (reflection && i == 0) || ((entry == 248 || entry == 249 || entry == 259 || entry == 507 || entry == 509 || entry == 512 || entry == 513) && i == 0) || (!post_instance && !no_hzb && single_instance && i == 3) || ((entry == 1019 || entry == 2138) && i == 0) || (cluster && i == 5) || ((entry == 1213 || entry == 1215 || entry == 1230) && i == 0)
                        || ((entry == 1219 || entry == 1236) && i == 1);
        u.Buffer.StructureByteStride = typed ? 0 : strides[i];
        if (typed) u.Format = DXGI_FORMAT_R32_UINT;
        if ((((post_instance || no_hzb) && i == 2) || (!post_instance && !no_hzb && single_instance && i == 4)) || (instance && i == 0) || (cluster && i >= 2 && i <= 4)) {
            u.Buffer.NumElements = sizes[i] / 4;
            u.Buffer.StructureByteStride = 0;
            u.Format = DXGI_FORMAT_R32_TYPELESS;
            u.Buffer.Flags = D3D12_BUFFER_UAV_FLAG_RAW;
        }
        dev->CreateUnorderedAccessView(outputs[i].Get(), nullptr, &u, cpu(80 + i));
    }
    ID3D12DescriptorHeap *heaps[] = {heap.Get(), hzb.samplers.Get()};
    list->SetDescriptorHeaps(depth_cull ? 2 : 1, heaps);
    list->SetComputeRootSignature(root.Get());
    list->SetComputeRootDescriptorTable(0, gpu(0));
    list->SetComputeRootDescriptorTable(1, gpu(64));
    list->SetComputeRootDescriptorTable(3, gpu(80));
    if (depth_cull) list->SetComputeRootDescriptorTable(2, hzb.samplers->GetGPUDescriptorHandleForHeapStart());
    list->Dispatch(groups_x, groups_y, 1);
    for (UINT i = 0; i < strides.size(); ++i) {
        transition(list.Get(), outputs[i].Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
        if (single_instance && i < 2) {
            D3D12_TEXTURE_COPY_LOCATION src{}, dst{};
            src.pResource = outputs[i].Get();
            dst.pResource = readbacks[i].Get();
            dst.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
            dst.PlacedFootprint.Footprint = {i == 0 ? DXGI_FORMAT_R32G32B32A32_UINT : DXGI_FORMAT_R32_UINT, 9, 9, 1, 256};
            list->CopyTextureRegion(&dst, 0, 0, 0, &src, nullptr);
        } else {
            list->CopyBufferRegion(readbacks[i].Get(), 0, outputs[i].Get(), 0, sizes[i]);
        }
    }
    submit_and_wait(dev, ctx.queue.Get(), list.Get());
    for (UINT i = 0; i < strides.size(); ++i)
        write_buffer(readbacks[i].Get(), name + ".out" + std::to_string(i), sizes[i]);
}

int main() {
    FILE *receipt{};
    if (fopen_s(&receipt, "vsm.txt", "wb")) return 2;
    int status = 0;
    try {
        Context ctx(receipt);
        std::ifstream manifest("vsm.inputs");
        std::string name;
        UINT entry, gx, gy, srv, count;
        while (manifest >> name >> entry >> gx >> gy >> srv >> count) {
            std::vector<UINT> strides(count);
            for (UINT &x : strides) manifest >> x;
            if (!manifest) throw std::runtime_error("incomplete VSM fixture");
            dispatch(ctx, name, entry, gx, gy, srv, strides);
        }
    } catch (const std::exception &err) { std::fprintf(receipt, "error=%s\n", err.what()); status = 1; }
    std::fclose(receipt);
    return status;
}
