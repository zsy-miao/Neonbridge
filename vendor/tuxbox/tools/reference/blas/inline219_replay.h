#include "inline_scene_fixture.h"

static void replay_inline219(const std::array<std::array<UINT, 32>, 7>& input_words,
                             const std::array<std::array<UINT, 1024>, 4>& cb, const std::array<UINT, 32>& initial,
                             const std::array<UINT, 32>& expected, UINT scenario, FILE* receipt) {
    static InlineSceneFixture context("219.dxbc", 0x20, 1, false);
    auto dev = context.dev.Get();
    std::array<ComPtr<ID3D12Resource>, 7> inputs;
    std::array<ComPtr<ID3D12Resource>, 3> constants;
    D3D12_DESCRIPTOR_HEAP_DESC hd{D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 96,
                                  D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, 0};
    ComPtr<ID3D12DescriptorHeap> heap, samplers;
    check(dev->CreateDescriptorHeap(&hd, IID_PPV_ARGS(&heap)), "219 view descriptors");
    hd.Type = D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER;
    hd.NumDescriptors = 16;
    check(dev->CreateDescriptorHeap(&hd, IID_PPV_ARGS(&samplers)), "219 sampler descriptors");
    const auto step = dev->GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV);
    using Gpu = UINT64(WINAPI*)(ID3D12DescriptorHeap*);
    auto cpu = [&](UINT i) {
        return D3D12_CPU_DESCRIPTOR_HANDLE{method<frame_abi::CpuHandle>(heap.Get(), 9)(heap.Get()) + i * step};
    };
    auto gpu = [&](UINT i) {
        return D3D12_GPU_DESCRIPTOR_HANDLE{method<Gpu>(heap.Get(), 10)(heap.Get()) + i * step};
    };
    for (UINT i = 0; i < 7; ++i) {
        D3D12_SHADER_RESOURCE_VIEW_DESC view{};
        view.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
        if (i == 2 || i == 3) {
            view.ViewDimension = D3D12_SRV_DIMENSION_TEXTURE2D;
            view.Format = DXGI_FORMAT_R32G32B32A32_FLOAT;
            view.Texture2D.MipLevels = 1;
        } else {
            view.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
            if (i == 5) {
                view.Format = DXGI_FORMAT_R32_TYPELESS;
                view.Buffer.Flags = D3D12_BUFFER_SRV_FLAG_RAW;
            } else
                view.Buffer.StructureByteStride = i == 4 || i == 6 ? 4u : 16u;
            view.Buffer.NumElements =
                128 / (view.Buffer.StructureByteStride ? view.Buffer.StructureByteStride : 4u);
        }
        if (i != 2 && i != 3 && i != 5)
            inputs[i] = upload(dev, input_words[i].data(), sizeof(input_words[i]));
        dev->CreateShaderResourceView(i == 5 ? context.scene.Get() : inputs[i].Get(), &view, cpu(i));
    }
    for (UINT i = 0; i < 3; ++i) {
        constants[i] = upload(dev, cb[i].data(), sizeof(cb[i]));
        D3D12_CONSTANT_BUFFER_VIEW_DESC cbv{constants[i]->GetGPUVirtualAddress(), 4096};
        dev->CreateConstantBufferView(&cbv, cpu(64 + i));
    }
    D3D12_SAMPLER_DESC sampler{};
    sampler.Filter = D3D12_FILTER_MIN_MAG_MIP_POINT;
    sampler.AddressU = sampler.AddressV = sampler.AddressW = D3D12_TEXTURE_ADDRESS_MODE_CLAMP;
    sampler.MaxLOD = D3D12_FLOAT32_MAX;
    dev->CreateSampler(
        &sampler, D3D12_CPU_DESCRIPTOR_HANDLE{method<frame_abi::CpuHandle>(samplers.Get(), 9)(samplers.Get())});
    auto out = buffer(dev, sizeof(initial), D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COPY_DEST, true);
    auto read = buffer(dev, sizeof(initial), D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
    auto seed = upload(dev, initial.data(), sizeof(initial));
    D3D12_UNORDERED_ACCESS_VIEW_DESC uav{};
    uav.ViewDimension = D3D12_UAV_DIMENSION_BUFFER;
    uav.Buffer.StructureByteStride = 4;
    uav.Buffer.NumElements = UINT(initial.size());
    dev->CreateUnorderedAccessView(out.Get(), nullptr, &uav, cpu(80));
    ComPtr<ID3D12CommandAllocator> alloc;
    check(dev->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&alloc)), "219 allocator");
    auto list = guest_list(dev, alloc.Get());
    list->SetPipelineState(context.pipeline.Get());
    D3D12_RESOURCE_BARRIER barrier{};
    barrier.Transition = {out.Get(), 0, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS};
    list->CopyBufferRegion(out.Get(), 0, seed.Get(), 0, sizeof(initial));
    list->ResourceBarrier(1, &barrier);
    ID3D12DescriptorHeap* heaps[]{heap.Get(), samplers.Get()};
    using Heaps = void(WINAPI*)(ID3D12GraphicsCommandList*, UINT, ID3D12DescriptorHeap* const*);
    using Root = void(WINAPI*)(ID3D12GraphicsCommandList*, ID3D12RootSignature*);
    using Table = void(WINAPI*)(ID3D12GraphicsCommandList*, UINT, D3D12_GPU_DESCRIPTOR_HANDLE);
    method<Heaps>(list.Get(), 32)(list.Get(), 2, heaps);
    method<Root>(list.Get(), 33)(list.Get(), context.root.Get());
    auto table = method<Table>(list.Get(), 35);
    table(list.Get(), 0, gpu(0));
    table(list.Get(), 1, gpu(64));
    table(list.Get(), 2, {method<Gpu>(samplers.Get(), 10)(samplers.Get())});
    table(list.Get(), 3, gpu(80));
    D3D12_DISPATCH_ARGUMENTS groups{8, 1, 1};
    ComPtr<ID3D12Resource> indirectArgs;
    ComPtr<ID3D12CommandSignature> signature;
    if (scenario == 2 || scenario == 4) {
        D3D12_INDIRECT_ARGUMENT_DESC arg{};
        arg.Type = D3D12_INDIRECT_ARGUMENT_TYPE_DISPATCH;
        D3D12_COMMAND_SIGNATURE_DESC cmd{sizeof(groups), 1, &arg, 0};
        check(dev->CreateCommandSignature(&cmd, nullptr, IID_PPV_ARGS(&signature)), "219 indirect signature");
        indirectArgs = upload(dev, &groups, sizeof(groups));
        using Execute = void(WINAPI*)(ID3D12GraphicsCommandList*, ID3D12CommandSignature*, UINT, ID3D12Resource*,
                                      UINT64, ID3D12Resource*, UINT64);
        method<Execute>(list.Get(), 59)(list.Get(), signature.Get(), 1, indirectArgs.Get(), 0, nullptr, 0);
    } else
        list->Dispatch(groups.ThreadGroupCountX, groups.ThreadGroupCountY, groups.ThreadGroupCountZ);
    barrier.Transition.StateBefore = D3D12_RESOURCE_STATE_UNORDERED_ACCESS;
    barrier.Transition.StateAfter = D3D12_RESOURCE_STATE_COPY_SOURCE;
    list->ResourceBarrier(1, &barrier);
    list->CopyBufferRegion(read.Get(), 0, out.Get(), 0, sizeof(initial));
    submit(dev, context.queue.Get(), list.Get());
    D3D12_RANGE range{0, sizeof(initial)}, none{};
    void* data{};
    check(read->Map(0, &range, &data), "219 result");
    auto words = static_cast<const UINT*>(data);
    const bool guards = std::equal(initial.begin() + 4, initial.end(), words + 4);
    std::array<UINT, 4> analytic{};
    if (scenario == 0)
        analytic[0] = 2;
    else if (scenario == 2 || scenario == 3) {
        analytic.fill(0xffffffffu);
        analytic[0] &= ~1u; // The first pixel was already resolved before dispatch.
    } else
        analytic.fill(0xaaaaaaaau);
    const bool match = guards && std::equal(expected.begin(), expected.end(), words) &&
                       std::equal(analytic.begin(), analytic.end(), words);
    std::fprintf(receipt, "inline219_%u=218 %08x,%08x,%08x,%08x; 219 %08x,%08x,%08x,%08x; guards %u; %s\n",
                 scenario, expected[0], expected[1], expected[2], expected[3], words[0], words[1], words[2],
                 words[3], guards, signature ? "indirect" : "direct");
    read->Unmap(0, &none);
    if (!match)
        throw std::runtime_error("219 output differs from 218");
}
