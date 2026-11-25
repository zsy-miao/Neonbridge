// Trace both retail translucency-volume shaders against a two-column depth
// texture. Each pair of X coordinates maps to the next slice of the volume.
static void probe_lumen_volume(ID3D12Device5 *dev, ID3D12CommandQueue *queue, ID3D12Resource *scene, FILE *receipt,
                               UINT entry) {
    using Pixel = std::array<float, 4>;
    constexpr UINT pitch = D3D12_TEXTURE_DATA_PITCH_ALIGNMENT, active_depth = 32, slices = active_depth + 1;
    constexpr UINT dispatch_width = active_depth * 2 + 1, srvs = 14;
    const UINT depth_slot = entry == 619 ? 12 : 9;
    const UINT scene_slot = depth_slot + 1;
    D3D12_DESCRIPTOR_RANGE ranges[]{{D3D12_DESCRIPTOR_RANGE_TYPE_SRV, srvs, 0, 1, 0},
                                    {D3D12_DESCRIPTOR_RANGE_TYPE_UAV, 2, 1, 1, 0}};
    D3D12_ROOT_PARAMETER params[8]{};
    params[7].ParameterType = D3D12_ROOT_PARAMETER_TYPE_UAV;
    params[7].Descriptor = {0, 1};
    for (UINT i = 0; i < 2; ++i) {
        params[i].ParameterType = D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE;
        params[i].DescriptorTable = {1, &ranges[i]};
    }
    for (UINT i = 0; i < 4; ++i) {
        params[2 + i].ParameterType = D3D12_ROOT_PARAMETER_TYPE_CBV;
        params[2 + i].Descriptor = {i, 1};
    }
    params[6].ParameterType = D3D12_ROOT_PARAMETER_TYPE_UAV;
    params[6].Descriptor = {0, 2};
    D3D12_STATIC_SAMPLER_DESC samplers[8]{};
    for (UINT i = 0; i < ARRAYSIZE(samplers); ++i) {
        auto &s = samplers[i];
        s.Filter = D3D12_FILTER_MIN_MAG_MIP_POINT;
        s.AddressU = s.AddressV = s.AddressW = D3D12_TEXTURE_ADDRESS_MODE_CLAMP;
        s.MaxAnisotropy = 1;
        s.ComparisonFunc = D3D12_COMPARISON_FUNC_ALWAYS;
        s.MaxLOD = D3D12_FLOAT32_MAX;
        s.ShaderRegister = i;
        s.RegisterSpace = 1;
    }
    D3D12_ROOT_SIGNATURE_DESC rs{ARRAYSIZE(params), params, ARRAYSIZE(samplers), samplers,
                                 D3D12_ROOT_SIGNATURE_FLAG_NONE};
    ObservedRayPipeline observed(dev, rs, entry,
                                 entry == 619 ? L"LumenTranslucencyVolumeHardwareRayTracingRGS_808058e3"
                                              : L"LumenTranslucencyVolumeHardwareRayTracingRGS_8e197dd2");
    ComPtr<ID3D12CommandAllocator> allocator;
    check(dev->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&allocator)), "volume allocator");
    ComPtr<ID3D12GraphicsCommandList4> list;
    check(dev->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, allocator.Get(), nullptr, IID_PPV_ARGS(&list)),
          "volume list");
    auto transition = [&](ID3D12Resource *r, D3D12_RESOURCE_STATES from, D3D12_RESOURCE_STATES to) {
        D3D12_RESOURCE_BARRIER b{};
        b.Type = D3D12_RESOURCE_BARRIER_TYPE_TRANSITION;
        b.Transition = {r, D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES, from, to};
        list->ResourceBarrier(1, &b);
    };
    D3D12_DESCRIPTOR_HEAP_DESC hd{D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, srvs + 2,
                                  D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, 0};
    ComPtr<ID3D12DescriptorHeap> heap;
    check(dev->CreateDescriptorHeap(&hd, IID_PPV_ARGS(&heap)), "volume descriptors");
    auto cpu = heap->GetCPUDescriptorHandleForHeapStart();
    const UINT step = dev->GetDescriptorHandleIncrementSize(hd.Type);
    D3D12_HEAP_PROPERTIES local{};
    local.Type = D3D12_HEAP_TYPE_DEFAULT;
    local.CreationNodeMask = local.VisibleNodeMask = 1;
    std::array<ComPtr<ID3D12Resource>, srvs + 2> resources, uploads;
    D3D12_SUBRESOURCE_FOOTPRINT output_fp{DXGI_FORMAT_R32G32B32A32_FLOAT, 2, 1, slices, pitch};
    auto sky = [](UINT face) { return Pixel{2.f + face, 3.f + 2 * face, 4.f + 3 * face, 1}; };
    const Pixel clear{-99, -99, -99, -99};
    for (UINT i = 0; i < srvs + 2; ++i, cpu.ptr += step) {
        D3D12_SHADER_RESOURCE_VIEW_DESC view{};
        view.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
        if (i == scene_slot) {
            view.ViewDimension = D3D12_SRV_DIMENSION_RAYTRACING_ACCELERATION_STRUCTURE;
            view.RaytracingAccelerationStructure.Location = scene->GetGPUVirtualAddress();
            dev->CreateShaderResourceView(nullptr, &view, cpu);
            continue;
        }
        const bool cube = i == 4, depth = i == depth_slot, out = i >= srvs;
        if (!cube && !depth && !out) {
            if (i == 6 || i == 7 || i == 8 || i == 10) {
                view.ViewDimension = D3D12_SRV_DIMENSION_TEXTURE2D;
                view.Format = DXGI_FORMAT_R32G32B32A32_FLOAT;
                view.Texture2D.MipLevels = 1;
            } else if (i == 9) {
                view.ViewDimension = D3D12_SRV_DIMENSION_TEXTURE3D;
                view.Format = DXGI_FORMAT_R32_UINT;
                view.Texture3D.MipLevels = 1;
            } else {
                view.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
                if (i == 2 || i == 3) {
                    view.Format = DXGI_FORMAT_R32_TYPELESS;
                    view.Buffer.Flags = D3D12_BUFFER_SRV_FLAG_RAW;
                } else {
                    view.Format = DXGI_FORMAT_UNKNOWN;
                    view.Buffer.StructureByteStride = 16;
                }
                view.Buffer.NumElements = 64;
            }
            dev->CreateShaderResourceView(nullptr, &view, cpu);
            continue;
        }
        D3D12_RESOURCE_DESC tex{};
        tex.Dimension = out ? D3D12_RESOURCE_DIMENSION_TEXTURE3D : D3D12_RESOURCE_DIMENSION_TEXTURE2D;
        tex.Width = cube ? 1u : 2u;
        tex.Height = tex.MipLevels = tex.SampleDesc.Count = 1;
        tex.DepthOrArraySize = cube ? 6 : out ? slices : 1;
        const bool scalar = i == srvs + 1;
        tex.Format = scalar ? DXGI_FORMAT_R32_FLOAT : DXGI_FORMAT_R32G32B32A32_FLOAT;
        tex.Flags = out ? D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS : D3D12_RESOURCE_FLAG_NONE;
        check(dev->CreateCommittedResource(&local, D3D12_HEAP_FLAG_NONE, &tex, D3D12_RESOURCE_STATE_COPY_DEST, nullptr,
                                           IID_PPV_ARGS(&resources[i])),
              "volume image");
        std::array<unsigned char, pitch * slices> data{};
        for (UINT z = 0; z < (out ? slices : 1u); ++z)
            for (UINT x = 0; x < tex.Width; ++x) {
                Pixel p = cube ? sky(0) : depth ? Pixel{float(x), 0, 0, 0} : clear;
                const size_t bytes = scalar ? sizeof(float) : sizeof(p);
                std::memcpy(data.data() + z * pitch + x * bytes, &p, bytes);
            }
        if (cube)
            for (UINT face = 0; face < 6; ++face) {
                Pixel color = sky(face);
                std::memcpy(data.data() + face * D3D12_TEXTURE_DATA_PLACEMENT_ALIGNMENT, &color, sizeof(color));
            }
        uploads[i] = upload(dev, data.data(), data.size());
        D3D12_TEXTURE_COPY_LOCATION src{}, dst{};
        src.pResource = uploads[i].Get();
        src.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
        src.PlacedFootprint.Footprint = {tex.Format, UINT(tex.Width), 1, out ? slices : 1, pitch};
        dst.pResource = resources[i].Get();
        for (UINT face = 0; face < (cube ? 6u : 1u); ++face) {
            src.PlacedFootprint.Offset = face * D3D12_TEXTURE_DATA_PLACEMENT_ALIGNMENT;
            dst.SubresourceIndex = face;
            list->CopyTextureRegion(&dst, 0, 0, 0, &src, nullptr);
        }
        if (out)
            dev->CreateUnorderedAccessView(resources[i].Get(), nullptr, nullptr, cpu);
        else {
            view.Format = tex.Format;
            view.ViewDimension = cube ? D3D12_SRV_DIMENSION_TEXTURECUBE : D3D12_SRV_DIMENSION_TEXTURE2D;
            if (cube)
                view.TextureCube.MipLevels = 1;
            else
                view.Texture2D.MipLevels = 1;
            dev->CreateShaderResourceView(resources[i].Get(), &view, cpu);
        }
        transition(resources[i].Get(), D3D12_RESOURCE_STATE_COPY_DEST,
                   out ? D3D12_RESOURCE_STATE_UNORDERED_ACCESS : D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
    }
    std::array<std::array<UINT, 1024>, 4> words{};
    auto f = [&](UINT cb, UINT bytes, float value) { std::memcpy(&words[cb][bytes / 4], &value, 4); };
    auto u = [&](UINT cb, UINT bytes, UINT value) { words[cb][bytes / 4] = value; };
    u(0, 0x340, 2);
    u(0, 0x344, 1);
    u(0, 0x348, active_depth);
    u(0, 0x3a0, 1);
    f(0, 0x330, 1);
    f(0, 0x338, 1000000);
    f(0, 0x3b0, -1);
    f(0, 0x3b8, 1);
    f(0, 0x3bc, 1);
    f(0, 0x3c4, 17);
    f(0, 0x3d0, 100);
    // Fixed world origin away from both triangles. The sky supplies each miss's radiance.
    f(0, 0x390, 20);
    f(0, 0x394, 20);
    f(0, 0x398, 20);
    f(0, 0x39c, 1);
    f(1, 0x410, 2);
    f(1, 0x41c, -2);
    f(1, 0x868, 0.5f);
    f(1, 0x86c, 1);
    f(1, 0x8b4, 1);
    f(1, 0xbe0, 1);
    f(1, 0xbe4, 1);
    f(1, 0xbe8, 1);
    f(3, 4, 1);
    std::array<ComPtr<ID3D12Resource>, 4> constants;
    for (UINT i = 0; i < 4; ++i)
        constants[i] = upload(dev, words[i].data(), sizeof(words[i]));
    std::array<UINT, 4 * dispatch_width> counts{};
    auto clean = upload(dev, counts.data(), sizeof(counts));
    auto counter = buffer(dev, sizeof(counts), D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COPY_DEST, true);
    list->CopyBufferRegion(counter.Get(), 0, clean.Get(), 0, sizeof(counts));
    transition(counter.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
    list->SetPipelineState1(observed.pipeline.Get());
    list->SetComputeRootSignature(observed.root.Get());
    ID3D12DescriptorHeap *heaps[]{heap.Get()};
    list->SetDescriptorHeaps(1, heaps);
    auto gpu = heap->GetGPUDescriptorHandleForHeapStart();
    list->SetComputeRootDescriptorTable(0, gpu);
    gpu.ptr += UINT64(srvs) * step;
    list->SetComputeRootDescriptorTable(1, gpu);
    for (UINT i = 0; i < 4; ++i)
        list->SetComputeRootConstantBufferView(2 + i, constants[i]->GetGPUVirtualAddress());
    list->SetComputeRootUnorderedAccessView(6, counter->GetGPUVirtualAddress());
    auto cards = buffer(dev, 1024, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, true);
    list->SetComputeRootUnorderedAccessView(7, cards->GetGPUVirtualAddress());
    auto table = observed.sbt->GetGPUVirtualAddress();
    D3D12_DISPATCH_RAYS_DESC rays{};
    rays.RayGenerationShaderRecord = {table, 32};
    rays.MissShaderTable = {table + 64, 32, 32};
    rays.HitGroupTable = {table + 128, 128, 32};
    rays.Width = dispatch_width;
    rays.Height = rays.Depth = 1;
    list->DispatchRays(&rays);
    constexpr UINT image_size = pitch * slices, image_stride = (image_size + 511) & ~511u,
                   count_offset = image_stride + image_size;
    auto rb = buffer(dev, count_offset + sizeof(counts), D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
    for (UINT i = 0; i < 2; ++i) {
        auto *r = resources[srvs + i].Get();
        transition(r, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
        D3D12_TEXTURE_COPY_LOCATION src{}, dst{};
        src.pResource = r;
        dst.pResource = rb.Get();
        dst.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
        dst.PlacedFootprint = {i * image_stride, output_fp};
        if (i)
            dst.PlacedFootprint.Footprint.Format = DXGI_FORMAT_R32_FLOAT;
        list->CopyTextureRegion(&dst, 0, 0, 0, &src, nullptr);
    }
    transition(counter.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
    list->CopyBufferRegion(rb.Get(), count_offset, counter.Get(), 0, sizeof(counts));
    submit(dev, queue, list.Get());
    D3D12_RANGE read{0, count_offset + sizeof(counts)}, empty{};
    void *memory{};
    check(rb->Map(0, &read, &memory), "read volume");
    const auto *bytes = static_cast<const unsigned char *>(memory);
    std::memcpy(counts.data(), bytes + count_offset, sizeof(counts));
    bool matched = true;
    UINT faces = 0;
    for (UINT i = 0; i < dispatch_width; ++i) {
        std::fprintf(receipt, "volume_rays_%u_%u=%u,%u\n", entry, i, counts[4 * i], counts[4 * i + 1]);
        matched &= counts[4 * i] == 0 && counts[4 * i + 1] == (i % 2);
    }
    for (UINT z = 0; z < slices; ++z)
        for (UINT x = 0; x < 2; ++x) {
            Pixel radiance{};
            float distance{};
            std::memcpy(&radiance, bytes + z * pitch + x * sizeof(Pixel), sizeof(Pixel));
            std::memcpy(&distance, bytes + z * pitch + image_stride + x * sizeof(float), sizeof(float));
            const UINT face = z < active_depth && x ? counts[4 * (z * 2 + x) + 3] : 0;
            if (z < active_depth && x && face < 6)
                faces |= 1u << face;
            const Pixel expected_color = z == active_depth ? clear : x ? sky(face) : Pixel{};
            matched &= std::equal(radiance.begin(), radiance.begin() + 3, expected_color.begin());
            matched &= distance == (z == active_depth ? -99.0f : 17.0f);
            std::fprintf(receipt, "volume_cell_%u_%u_%u=%.8g,%.8g,%.8g;%.8g\n", entry, x, z, radiance[0], radiance[1],
                         radiance[2], distance);
        }
    std::fprintf(receipt, "volume_sky_faces_%u=0x%02x\n", entry, faces);
    matched &= faces == 0x3fu;
    rb->Unmap(0, &empty);
    if (!matched)
        throw std::runtime_error("unexpected Lumen volume lighting, distance or callbacks");
}
