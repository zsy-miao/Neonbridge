#include "inline_scene_fixture.h"

static void replay_inline_radiance(RadianceInput input, const std::array<std::array<UINT, 1024>, 4>& cb,
                             const std::array<UINT, 130>& expected, const std::array<UINT, 325>& retrace, UINT hits, FILE* receipt, UINT scenario, std::array<UINT, 325>& actual_retrace,
                             ComPtr<ID3D12Resource>& produced, bool consume = false) {
    constexpr UINT entry = TUXBOX_INLINE_RADIANCE;
    constexpr const char* shader = entry == 306 ? "306.dxbc" : "318.dxbc";
#ifdef TUXBOX_INLINE313_REPLAY
    constexpr UINT near_mask = 0x01, far_mask = 0x20;
#else
    constexpr UINT near_mask = 0xff, far_mask = 0xff;
#endif
    static InlineSceneFixture forward(shader, near_mask, far_mask, true);
    static InlineSceneFixture reversed(shader, near_mask, far_mask, true, true);
    auto& context = scenario == 4 ? reversed : forward;
    auto dev = context.dev.Get();
    auto pipeline = context.pipeline;
#ifdef TUXBOX_INLINE313_REPLAY
    if (consume) {
        const auto original = read_file("313.dxbc");
        D3D12_COMPUTE_PIPELINE_STATE_DESC desc{};
        desc.pRootSignature = context.root.Get();
        desc.CS = {original.data(), original.size()};
        using Create = HRESULT(WINAPI*)(ID3D12Device*, const D3D12_COMPUTE_PIPELINE_STATE_DESC*, UINT,
                                        const void*, REFIID, void**);
        pipeline.Reset();
        check(method<Create>(dev, 62)(dev, &desc, 0, nullptr, IID_PPV_ARGS(&pipeline)), "313 Xbox slot 62");
    }
#endif
    ComPtr<ID3D12CommandAllocator> alloc;
    check(dev->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&alloc)), "318 allocator");
    auto list = guest_list(dev, alloc.Get());
    D3D12_DESCRIPTOR_HEAP_DESC hd{D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 96,
                                  D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, 0};
    ComPtr<ID3D12DescriptorHeap> heap, samplers;
    check(dev->CreateDescriptorHeap(&hd, IID_PPV_ARGS(&heap)), "318 view descriptors");
    hd.Type = D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER;
    hd.NumDescriptors = 16;
    check(dev->CreateDescriptorHeap(&hd, IID_PPV_ARGS(&samplers)), "318 samplers");
    using Gpu = UINT64(WINAPI*)(ID3D12DescriptorHeap*);
    const UINT step = dev->GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV);
    auto cpu = [&](UINT slot) {
        return D3D12_CPU_DESCRIPTOR_HANDLE{method<frame_abi::CpuHandle>(heap.Get(), 9)(heap.Get()) + slot * step};
    };
    auto gpu = [&](UINT slot) {
        return D3D12_GPU_DESCRIPTOR_HANDLE{method<Gpu>(heap.Get(), 10)(heap.Get()) + slot * step};
    };
    RadianceInputs resources(dev, list.Get(), context.scene.Get(), cpu(0), input, false, true, scenario == 3 || scenario == 4);
    std::array<ComPtr<ID3D12Resource>, 4> constants;
    for (UINT i = 0; i < constants.size(); ++i) {
        constants[i] = upload(dev, cb[i].data(), sizeof(cb[i]));
        D3D12_CONSTANT_BUFFER_VIEW_DESC desc{constants[i]->GetGPUVirtualAddress(), sizeof(cb[i])};
        dev->CreateConstantBufferView(&desc, cpu(64 + i));
    }
    D3D12_SAMPLER_DESC sampler{};
    sampler.Filter = D3D12_FILTER_MIN_MAG_MIP_POINT;
    sampler.AddressU = sampler.AddressV = sampler.AddressW = D3D12_TEXTURE_ADDRESS_MODE_CLAMP;
    sampler.ComparisonFunc = D3D12_COMPARISON_FUNC_ALWAYS;
    sampler.MaxAnisotropy = 1;
    sampler.MaxLOD = D3D12_FLOAT32_MAX;
    auto sample_cpu = method<frame_abi::CpuHandle>(samplers.Get(), 9)(samplers.Get());
    for (UINT i = 0; i < 2; ++i) {
        dev->CreateSampler(&sampler, D3D12_CPU_DESCRIPTOR_HANDLE{sample_cpu});
        sample_cpu += dev->GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER);
    }
    std::array<UINT, 130> initial;
    initial.fill(0xdeadbeef);
    std::array<UINT, 325> unwritten;
    unwritten.fill(0xdeadbeef);
    auto seed = upload(dev, unwritten.data(), sizeof(unwritten));
    constexpr UINT outputs = entry == 306 ? 3 : 2;
    constexpr UINT bytes[]{sizeof(initial), sizeof(initial), sizeof(unwritten)};
    constexpr UINT read_bytes = sizeof(initial) * 2 + (entry == 306 ? sizeof(unwritten) : 0);
    auto read = buffer(dev, read_bytes, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
    std::array<ComPtr<ID3D12Resource>, outputs> output;
    auto transition = [&](UINT i, D3D12_RESOURCE_STATES from, D3D12_RESOURCE_STATES to) {
        D3D12_RESOURCE_BARRIER bar{};
        bar.Transition = {output[i].Get(), D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES, from, to};
        list->ResourceBarrier(1, &bar);
    };
    for (UINT i = 0; i < outputs; ++i) {
        if (consume && i == 2) {
            // This is the same DEFAULT allocation that 306 populated on the GPU.
            output[i] = produced;
            transition(i, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
        } else {
            output[i] = buffer(dev, bytes[i], D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COPY_DEST, true);
            list->CopyBufferRegion(output[i].Get(), 0, seed.Get(), 0, bytes[i]);
            transition(i, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
        }
        D3D12_UNORDERED_ACCESS_VIEW_DESC uav{};
        uav.ViewDimension = D3D12_UAV_DIMENSION_BUFFER;
        constexpr UINT strides[]{4, 8, 20};
        uav.Buffer.StructureByteStride = strides[i];
        uav.Buffer.NumElements = bytes[i] / uav.Buffer.StructureByteStride;
        dev->CreateUnorderedAccessView(output[i].Get(), nullptr, &uav, cpu(80 + i));
    }
    list->SetPipelineState(pipeline.Get());
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
    list->Dispatch(2, 1, 1);
    for (UINT i = 0; i < outputs; ++i) {
        transition(i, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
        list->CopyBufferRegion(read.Get(), i * sizeof(initial), output[i].Get(), 0, bytes[i]);
    }
    submit(dev, context.queue.Get(), list.Get());
    D3D12_RANGE r{0, read_bytes}, none{};
    void* data{};
    check(read->Map(0, &r, &data), "318 result");
    auto feedback = static_cast<const UINT*>(data), words = feedback + initial.size();
    auto expected_feedback = initial;
    if (scenario == 3) expected_feedback[4] = 123;
    bool feedback_ok = std::equal(expected_feedback.begin(), expected_feedback.end(), feedback);
    if (!feedback_ok)
        for (UINT i = 0; i < initial.size(); ++i)
            if (expected_feedback[i] != feedback[i])
                std::fprintf(receipt, "318_feedback_%u=%08x expected %08x\n", i, feedback[i], expected_feedback[i]);
    UINT correct = 0;
    for (UINT i = 0; i < 65; ++i) {
        UINT rg = expected[2*i], bd = expected[2*i+1];
        if (scenario == 3 && rg == 0) {
            rg = 0x42004000; // Cache page RGB (2,3,4), retaining the 323 hit distance.
            bd |= 0x4400;
        }
        if (words[2*i] == rg && words[2*i+1] == bd)
            ++correct;
        else
            std::fprintf(receipt, "318_pixel_%u=expected %08x,%08x; 318 %08x,%08x\n", i,
                         rg, bd, words[2*i], words[2*i+1]);
    }
    const UINT dispatch_entry = consume ? 313 : entry;
    bool retrace_ok = true;
    if constexpr (entry == 306) {
        produced = output[2];
        const auto* actual = words + initial.size();
        std::copy_n(actual, actual_retrace.size(), actual_retrace.begin());
        UINT winding_rounds = 0;
        for (UINT i = 0; i < 65; ++i) {
            std::array<UINT, 5> row;
            std::copy_n(retrace.data() + 5 * i, 5, row.begin());
            // The 299 control intersects forward-wound triangles. Reversing winding may
            // round T into an adjacent packed bin. The low three status bits must match.
            UINT produced = actual[5*i+2];
            if (scenario == 4 && i < 64 && expected[2*i] == 0 &&
                (produced == row[2] + 8u || row[2] == produced + 8u)) {
                row[2] = produced;
                ++winding_rounds;
            }
            if (!std::equal(row.begin(), row.end(), actual + 5*i)) {
                std::fprintf(receipt, "retrace_%u=expected %08x,%08x,%08x,%08x,%08x; actual %08x,%08x,%08x,%08x,%08x\n", i,
                             row[0], row[1], row[2], row[3], row[4], actual[5*i], actual[5*i+1], actual[5*i+2], actual[5*i+3], actual[5*i+4]);
                retrace_ok = false;
            }
        }
        std::fprintf(receipt, "retrace_%u_case_%u=tile[27] %08x,%08x,%08x,%08x,%08x; 65 records matched %u; adjacent winding distance bins %u\n",
                     dispatch_entry, scenario, actual[135], actual[136], actual[137], actual[138], actual[139], retrace_ok, winding_rounds);
    }
    std::fprintf(receipt, "inline%u_%u=%u/65 records matched; %u reference hits; feedback[4] %08x; pixel[27] %08x,%08x; feedback guards %u\n",
                 dispatch_entry, scenario, correct, hits, feedback[4], words[54], words[55], feedback_ok);
    read->Unmap(0, &none);
    if (correct != 65 || !feedback_ok || !retrace_ok)
        throw std::runtime_error("radiance, feedback or retrace data differs from expected");
}
