// Route neighboring rays to different miss records of the same retail shader.
// Each record owns its descriptor tables and uses the title's 96-byte local ABI.
static void probe_lighting(ID3D12Device5 *dev, ID3D12CommandQueue *queue, ID3D12Resource *scene, FILE *receipt) {
    D3D12_ROOT_PARAMETER params[2]{};
    params[0].ParameterType = D3D12_ROOT_PARAMETER_TYPE_SRV;
    params[0].Descriptor = {0, 3};
    params[1].ParameterType = D3D12_ROOT_PARAMETER_TYPE_UAV;
    params[1].Descriptor = {0, 3};
    D3D12_ROOT_SIGNATURE_DESC rs{2, params, 0, nullptr, D3D12_ROOT_SIGNATURE_FLAG_NONE};
    ComPtr<ID3DBlob> blob;
    check(D3D12SerializeRootSignature(&rs, D3D_ROOT_SIGNATURE_VERSION_1, &blob, nullptr), "serialize lighting root");
    ComPtr<ID3D12RootSignature> global;
    check(dev->CreateRootSignature(0, blob->GetBufferPointer(), blob->GetBufferSize(), IID_PPV_ARGS(&global)),
          "lighting root");
    std::array<std::vector<char>, 3> libs{read_file("lighting-observer.dxil"), read_file("2752.dxil"),
                                          read_file("2670.dxil")};
    D3D12_DXIL_LIBRARY_DESC lib[3]{};
    D3D12_HIT_GROUP_DESC group{};
    group.HitGroupExport = L"ObservedHidden";
    group.ClosestHitShaderImport = L"HiddenMaterialCHS_95a384b3";
    group.AnyHitShaderImport = L"HiddenMaterialAHS_95a384b3";
    D3D12_RAYTRACING_SHADER_CONFIG config{64, 8};
    D3D12_RAYTRACING_PIPELINE_CONFIG depth{1};
    D3D12_GLOBAL_ROOT_SIGNATURE root{global.Get()};
    D3D12_STATE_SUBOBJECT subs[7]{};
    for (UINT i = 0; i < 3; ++i) {
        lib[i].DXILLibrary = {libs[i].data(), libs[i].size()};
        subs[i] = {D3D12_STATE_SUBOBJECT_TYPE_DXIL_LIBRARY, &lib[i]};
    }
    subs[3] = {D3D12_STATE_SUBOBJECT_TYPE_HIT_GROUP, &group};
    subs[4] = {D3D12_STATE_SUBOBJECT_TYPE_RAYTRACING_SHADER_CONFIG, &config};
    subs[5] = {D3D12_STATE_SUBOBJECT_TYPE_RAYTRACING_PIPELINE_CONFIG, &depth};
    subs[6] = {D3D12_STATE_SUBOBJECT_TYPE_GLOBAL_ROOT_SIGNATURE, &root};
    D3D12_STATE_OBJECT_DESC pd{D3D12_STATE_OBJECT_TYPE_RAYTRACING_PIPELINE, ARRAYSIZE(subs), subs};
    ComPtr<ID3D12StateObject> pso;
    check(dev->CreateStateObject(&pd, IID_PPV_ARGS(&pso)), "lighting and hidden pipeline");
    ComPtr<ID3D12StateObjectProperties> props;
    check(pso.As(&props), "lighting export identifiers");

    ComPtr<ID3D12DescriptorHeap> resources, samplers;
    D3D12_DESCRIPTOR_HEAP_DESC hd{D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 192,
                                  D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, 0};
    check(dev->CreateDescriptorHeap(&hd, IID_PPV_ARGS(&resources)), "lighting resource heap");
    auto step = dev->GetDescriptorHandleIncrementSize(hd.Type);
    hd.Type = D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER;
    hd.NumDescriptors = 16;
    check(dev->CreateDescriptorHeap(&hd, IID_PPV_ARGS(&samplers)), "lighting sampler heap");
    D3D12_SAMPLER_DESC sampler{};
    sampler.Filter = D3D12_FILTER_MIN_MAG_MIP_POINT;
    sampler.AddressU = sampler.AddressV = sampler.AddressW = D3D12_TEXTURE_ADDRESS_MODE_CLAMP;
    sampler.MaxAnisotropy = 1;
    sampler.MaxLOD = D3D12_FLOAT32_MAX;
    sampler.ComparisonFunc = D3D12_COMPARISON_FUNC_ALWAYS;
    auto scpu = samplers->GetCPUDescriptorHandleForHeapStart();
    for (UINT i = 0; i < 16; ++i, scpu.ptr += dev->GetDescriptorHandleIncrementSize(hd.Type))
        dev->CreateSampler(&sampler, scpu);
    std::array<float, 4096> constants{};
    constants[1] = 1.f;
    constants[244 * 4 + 1] = 1.f;
    auto uniform = upload(dev, constants.data(), sizeof(constants));
    std::array<ComPtr<ID3D12Resource>, 2> data;
    struct LocalRecord {
        BYTE id[32];
        UINT64 index, vertex;
        UINT constants[4];
        UINT64 srvs, cbvs, samplers, uavs;
    };
    static_assert(sizeof(LocalRecord) == 96 && offsetof(LocalRecord, srvs) == 64);
    std::array<BYTE, 448> table{};
    auto identify = [&](BYTE *dst, const wchar_t *name) {
        const auto *id = props->GetShaderIdentifier(name);
        if (!id)
            throw std::runtime_error("missing lighting or hidden shader identifier");
        std::memcpy(dst, id, 32);
    };
    identify(table.data(), L"LightingProbeRay");
    identify(table.data() + 256, L"HiddenProbeMiss");
    identify(table.data() + 384, L"ObservedHidden");
    for (UINT variant = 0; variant < 2; ++variant) {
        // One directional FRTLightingData entry occupies eight uint4s.
        std::array<std::array<float, 4>, 8> light{};
        const UINT no_profile = 0xffffffff;
        std::memcpy(&light[0][1], &no_profile, sizeof(no_profile));
        light[1] = {0, 0, 1, 1};
        light[2] = {0, 0, 1, 0};
        if (variant)
            light[3] = {1, .5f, .25f, 1};
        light[4] = {1, 0, 0, 1};
        light[5] = {0, 1, 0, 1};
        data[variant] = upload(dev, light.data(), sizeof(light));
        auto cpu = resources->GetCPUDescriptorHandleForHeapStart();
        cpu.ptr += variant * 96 * step;
        D3D12_SHADER_RESOURCE_VIEW_DESC tex{};
        tex.Format = DXGI_FORMAT_R32G32B32A32_FLOAT;
        tex.ViewDimension = D3D12_SRV_DIMENSION_TEXTURE2D;
        tex.Texture2D.MipLevels = 1;
        tex.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
        for (UINT slot = 0; slot < 64; ++slot)
            dev->CreateShaderResourceView(nullptr, &tex, {cpu.ptr + slot * step});
        D3D12_SHADER_RESOURCE_VIEW_DESC buf{};
        buf.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
        buf.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
        buf.Buffer.NumElements = 8;
        buf.Buffer.StructureByteStride = 16;
        dev->CreateShaderResourceView(data[variant].Get(), &buf, {cpu.ptr + 13 * step});
        const D3D12_CONSTANT_BUFFER_VIEW_DESC cb{uniform->GetGPUVirtualAddress(), sizeof(constants)};
        for (UINT slot = 64; slot < 80; ++slot)
            dev->CreateConstantBufferView(&cb, {cpu.ptr + slot * step});
        D3D12_UNORDERED_ACCESS_VIEW_DESC uav{};
        uav.ViewDimension = D3D12_UAV_DIMENSION_BUFFER;
        uav.Buffer.NumElements = 1;
        uav.Format = DXGI_FORMAT_R32_UINT;
        for (UINT slot = 80; slot < 96; ++slot)
            dev->CreateUnorderedAccessView(nullptr, nullptr, &uav, {cpu.ptr + slot * step});
        const UINT64 gpu = resources->GetGPUDescriptorHandleForHeapStart().ptr + variant * 96 * step;
        LocalRecord record{};
        identify(record.id, L"RayTracingLightingMS_c0e9eda2");
        record.index = record.vertex = uniform->GetGPUVirtualAddress();
        record.srvs = gpu;
        record.cbvs = gpu + 64 * step;
        record.samplers = samplers->GetGPUDescriptorHandleForHeapStart().ptr;
        record.uavs = gpu + 80 * step;
        std::memcpy(table.data() + 64 + variant * sizeof(record), &record, sizeof(record));
    }
    auto sbt = upload(dev, table.data(), table.size());
    constexpr UINT rays = 26, bytes = rays * 32 * sizeof(UINT);
    auto out = buffer(dev, bytes, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, true);
    auto rb = buffer(dev, bytes, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
    ComPtr<ID3D12CommandAllocator> alloc;
    ComPtr<ID3D12GraphicsCommandList4> list;
    check(dev->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&alloc)), "lighting allocator");
    check(dev->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, alloc.Get(), nullptr, IID_PPV_ARGS(&list)),
          "lighting list");
    ID3D12DescriptorHeap *heaps[]{resources.Get(), samplers.Get()};
    list->SetDescriptorHeaps(2, heaps);
    list->SetComputeRootSignature(global.Get());
    list->SetComputeRootShaderResourceView(0, scene->GetGPUVirtualAddress());
    list->SetComputeRootUnorderedAccessView(1, out->GetGPUVirtualAddress());
    list->SetPipelineState1(pso.Get());
    const auto va = sbt->GetGPUVirtualAddress();
    D3D12_DISPATCH_RAYS_DESC dispatch{};
    dispatch.RayGenerationShaderRecord = {va, 32};
    dispatch.MissShaderTable = {va + 64, 288, 96};
    dispatch.HitGroupTable = {va + 384, 32, 32};
    dispatch.Width = rays;
    dispatch.Height = dispatch.Depth = 1;
    list->DispatchRays(&dispatch);
    D3D12_RESOURCE_BARRIER barrier{};
    barrier.Transition = {out.Get(), 0, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE};
    list->ResourceBarrier(1, &barrier);
    list->CopyBufferRegion(rb.Get(), 0, out.Get(), 0, bytes);
    submit(dev, queue, list.Get());
    D3D12_RANGE range{0, bytes}, empty{};
    void *mapped{};
    check(rb->Map(0, &range, &mapped), "lighting readback");
    auto words = static_cast<const UINT *>(mapped);
    bool passed = true;
    UINT contributions = 0;
    auto half = [](UINT value) {
        const UINT bits = value & 0xffffu, exp = (bits >> 10) & 31u;
        if (exp == 31u)
            return NAN;
        const float mantissa = static_cast<float>((bits & 1023u) + (exp ? 1024u : 0u));
        return std::ldexp((bits & 0x8000u) ? -mantissa : mantissa, static_cast<int>(exp ? exp : 1u) - 25);
    };
    for (UINT i = 0; i < rays; ++i) {
        const UINT *before = words + 32 * i, *after = before + 16;
        for (UINT j = 0; j < 16; ++j) {
            if (i == 24 && j == 0)
                passed &= after[j] == 0xfeed;
            else if (i >= 24 || (j != 3 && j != 4))
                passed &= after[j] == before[j];
        }
        if (i < 24) {
            passed &= (after[4] >> 16) == (before[4] >> 16);
            if (!(i & 1))
                passed &= after[3] == before[3] && after[4] == before[4];
            else {
                const float r = half(after[3]) - half(before[3]);
                const float g = half(after[3] >> 16) - half(before[3] >> 16);
                const float b = half(after[4]) - half(before[4]);
                // White Lambertian material under a directional light with
                // RGB (1, 1/2, 1/4). Allow half-precision accumulation error.
                passed &= std::abs(g - r * .5f) < .0006f && std::abs(b - r * .25f) < .0006f;
                if (i / 4 == 2)
                    passed &= std::abs(r - 1.f / 3.14159265f) < .0006f;
                else if (i / 4 == 5)
                    passed &= r == 0.f;
                else
                    passed &= r > 0.f && r < 1.f / 3.14159265f;
                contributions += r > 0.f;
            }
        }
        std::fprintf(receipt, "lighting_ray_%u=%08x,%08x,%08x,%08x,%08x\n", i, after[0], before[3], before[4], after[3],
                     after[4]);
    }
    rb->Unmap(0, &empty);
    std::fprintf(receipt, "lighting_contributions=%u\n", contributions);
    if (!passed || contributions != 10)
        throw std::runtime_error("unexpected retail lighting or hidden payload");
}
