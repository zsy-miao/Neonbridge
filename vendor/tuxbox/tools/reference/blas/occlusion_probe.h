enum class ShadowInput { baseline, scene_masked, absent_page, empty_page, occupied_page, hair_masked, hair_lit };

// Submit the native screen-space shadow raygen with controlled G-buffer inputs.
static void probe_screen_occlusion(ID3D12Device5* dev, ID3D12CommandQueue* queue,
                                   ID3D12Resource* scene, FILE* receipt, UINT entry,
                                   ShadowInput input = ShadowInput::baseline) {
    constexpr UINT width = 4, height = 2, count = width * height;
    using Pixel = std::array<float, 4>;
    const bool hair = entry == 3810;
    const bool distance_output = entry == 3824;
    const bool occupied = input == ShadowInput::occupied_page;
    const bool exclude_scene = input == ShadowInput::scene_masked;
    const bool hair_channel = input == ShadowInput::hair_lit || input == ShadowInput::hair_masked;
    const char* variant = "";
    switch (input) {
        case ShadowInput::baseline: break;
        case ShadowInput::scene_masked: variant = "_mask"; break;
        case ShadowInput::absent_page: variant = "_absent"; break;
        case ShadowInput::empty_page: variant = "_empty"; break;
        case ShadowInput::occupied_page: variant = "_resident"; break;
        case ShadowInput::hair_masked: variant = "_hair_mask"; break;
        case ShadowInput::hair_lit: variant = "_hair"; break;
    }
    const UINT srv_count = hair ? 12u : 7u;
    const UINT scene_index = hair ? 10u : 6u;
    const UINT uav_count = entry == 3801 ? 1u : 2u;
    constexpr UINT pitch = D3D12_TEXTURE_DATA_PITCH_ALIGNMENT;

    D3D12_DESCRIPTOR_RANGE ranges[]{
        {D3D12_DESCRIPTOR_RANGE_TYPE_SRV, srv_count, 0, 1, 0},
        {D3D12_DESCRIPTOR_RANGE_TYPE_UAV, uav_count, 0, 1, 0}};
    D3D12_ROOT_PARAMETER params[6]{};
    params[0].ParameterType = params[1].ParameterType = D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE;
    params[0].DescriptorTable = {1, ranges};
    params[1].DescriptorTable = {1, ranges + 1};
    params[2].ParameterType = D3D12_ROOT_PARAMETER_TYPE_CBV;
    params[2].Descriptor = {2, 1};
    params[3].ParameterType = params[4].ParameterType = D3D12_ROOT_PARAMETER_TYPE_CBV;
    params[3].Descriptor = {0, 1};
    params[4].Descriptor = {1, 1};
    params[5].ParameterType = D3D12_ROOT_PARAMETER_TYPE_UAV;
    params[5].Descriptor = {0, 2};
    D3D12_ROOT_SIGNATURE_DESC root_desc{ARRAYSIZE(params), params, 0, nullptr, D3D12_ROOT_SIGNATURE_FLAG_NONE};
    ObservedRayPipeline observed(dev, root_desc, entry, hair ? L"OcclusionRGS_f203fe18" :
                                 distance_output ? L"OcclusionRGS_2e01868a" : L"OcclusionRGS_d7047c3a");

    ComPtr<ID3D12CommandAllocator> allocator;
    check(dev->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&allocator)), "shadow allocator");
    ComPtr<ID3D12GraphicsCommandList4> list;
    check(dev->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, allocator.Get(), nullptr, IID_PPV_ARGS(&list)), "shadow list");
    auto transition = [&](ID3D12Resource* resource, D3D12_RESOURCE_STATES from, D3D12_RESOURCE_STATES to) {
        D3D12_RESOURCE_BARRIER b{};
        b.Type = D3D12_RESOURCE_BARRIER_TYPE_TRANSITION;
        b.Transition = {resource, 0, from, to};
        list->ResourceBarrier(1, &b);
    };
    D3D12_DESCRIPTOR_HEAP_DESC heap_desc{D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, srv_count + uav_count, D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, 0};
    ComPtr<ID3D12DescriptorHeap> heap;
    check(dev->CreateDescriptorHeap(&heap_desc, IID_PPV_ARGS(&heap)), "shadow descriptor heap");
    const auto step = dev->GetDescriptorHandleIncrementSize(heap_desc.Type);
    auto cpu = heap->GetCPUDescriptorHandleForHeapStart();
    D3D12_RESOURCE_DESC tex{};
    tex.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
    tex.Width = width; tex.Height = height;
    tex.MipLevels = tex.DepthOrArraySize = tex.SampleDesc.Count = 1;
    tex.Format = DXGI_FORMAT_R32G32B32A32_FLOAT;
    D3D12_PLACED_SUBRESOURCE_FOOTPRINT footprint{0, {tex.Format, width, height, 1, pitch}};
    D3D12_HEAP_PROPERTIES local{};
    local.Type = D3D12_HEAP_TYPE_DEFAULT;
    local.CreationNodeMask = local.VisibleNodeMask = 1;
    std::array<ComPtr<ID3D12Resource>, 14> images, uploads;
    for (UINT i = 0; i < srv_count + uav_count; ++i, cpu.ptr += step) {
        if (i == scene_index) {
            D3D12_SHADER_RESOURCE_VIEW_DESC as{};
            as.ViewDimension = D3D12_SRV_DIMENSION_RAYTRACING_ACCELERATION_STRUCTURE;
            as.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
            as.RaytracingAccelerationStructure.Location = scene->GetGPUVirtualAddress();
            dev->CreateShaderResourceView(nullptr, &as, cpu);
            continue;
        }
        if (hair && i < 2) {
            // One physical page and a 32-byte virtual voxel node containing its world bounds.
            struct Node { float minimum[3]; UINT resolution; float maximum[3]; UINT page; };
            const Node node{{-128,-128,-128}, 0x010101u, {128,128,128}, 0};
            static_assert(sizeof(node) == 32);
            UINT physical_page = occupied || input == ShadowInput::empty_page ? 0u : UINT_MAX;
            images[i] = i ? upload(dev, &node, sizeof(node)) : upload(dev, &physical_page, sizeof(physical_page));
            D3D12_SHADER_RESOURCE_VIEW_DESC view{};
            view.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
            view.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
            view.Format = i ? DXGI_FORMAT_UNKNOWN : DXGI_FORMAT_R32_UINT;
            view.Buffer.NumElements = 1;
            view.Buffer.StructureByteStride = i ? sizeof(node) : 0;
            dev->CreateShaderResourceView(images[i].Get(), &view, cpu);
            continue;
        }
        const bool volume = hair && i == 2;
        const bool channels = hair ? (i == 9 || i == 11) : i == 5;
        const bool target = i >= srv_count;
        auto shape = tex;
        if (volume) { shape.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE3D; shape.Format = DXGI_FORMAT_R32_UINT;
                      shape.Width = shape.Height = 1; }
        if (channels) shape.Format = DXGI_FORMAT_R32_UINT;
        if (target) shape.Flags = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
        check(dev->CreateCommittedResource(&local, D3D12_HEAP_FLAG_NONE, &shape, D3D12_RESOURCE_STATE_COPY_DEST,
                                            nullptr, IID_PPV_ARGS(&images[i])), "shadow texture");
        std::array<unsigned char, pitch * height> pixels{};
        if (volume) {
            // The low 24 bits carry density. Flag bits alone describe an empty voxel.
            UINT density = input == ShadowInput::empty_page ? 0xff000000u : 1024u;
            std::memcpy(pixels.data(), &density, sizeof(density));
        } else for (UINT y = 0; y < height; ++y) for (UINT x = 0; x < width; ++x) {
            if (channels) {
                // The light uses bit 0. A different channel also has a nonzero mask.
                const bool disjoint = (exclude_scene && x == 1 && !y) ||
                                      (input == ShadowInput::hair_masked && i == 11);
                UINT mask = disjoint ? 0x100u : 0x101u;
                std::memcpy(pixels.data() + y * pitch + x * sizeof(mask), &mask, sizeof(mask));
                continue;
            }
            Pixel value{};
            const UINT gbuffer = hair ? i - 4u : i;
            if (hair && i == 3 && hair_channel && x == 2 && !y) value[0] = 1.0f;
            if (gbuffer == 0) value[0] = x == 0 ? 0.0f : 1.0f;
            if (gbuffer == 2) value = {.5f, .5f, 1, 0};
            if (gbuffer == 3) value[3] = (y ? 2.0f : 1.0f) / 255.0f;
            if (target) value = {-99,-99,-99,-99};
            std::memcpy(pixels.data() + y * pitch + x * sizeof(Pixel), &value, sizeof(value));
        }
        uploads[i] = upload(dev, pixels.data(), pixels.size());
        D3D12_TEXTURE_COPY_LOCATION src{};
        src.pResource = uploads[i].Get(); src.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
        src.PlacedFootprint = footprint;
        src.PlacedFootprint.Footprint = {shape.Format, static_cast<UINT>(shape.Width), shape.Height, 1, pitch};
        D3D12_TEXTURE_COPY_LOCATION dst{};
        dst.pResource = images[i].Get(); dst.Type = D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
        list->CopyTextureRegion(&dst, 0, 0, 0, &src, nullptr);
        if (target) {
            dev->CreateUnorderedAccessView(images[i].Get(), nullptr, nullptr, cpu);
            transition(images[i].Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
        } else {
            dev->CreateShaderResourceView(images[i].Get(), nullptr, cpu);
            transition(images[i].Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
        }
    }
    std::array<UINT, 128> b0{};
    std::array<UINT, 640> b1{};
    auto write_float = [](UINT* cb, UINT byte, float value) { std::memcpy(cb + byte / 4, &value, 4); };
    // All columns except the last are inside the light's rectangle.
    b0[0x38 / 4] = width - 2; b0[0x3c / 4] = height - 1;
    b0[0x28 / 4] = 1; // light-channel mask
    b0[0x120 / 4] = 1; // read each pixel's scene lighting channels
    b0[0x68 / 4] = 1; // leave both triangle orientations enabled
    write_float(b0.data(), 0x98, 1.0f); // light direction +Z
    write_float(b0.data(), 0x4c, 10.0f); // secondary ray length
    // World x = raster x - 2.5. At columns 1 and 2: x = -1 and 0.
    write_float(b1.data(), 0x2c0, 1.0f);
    write_float(b1.data(), 0x2f0, -2.5f);
    write_float(b1.data(), 0x2f8, -1.0f);
    write_float(b1.data(), 0x2fc, 1.0f);
    std::array<UINT, 64> b2{};
    b0[0x48 / 4] = hair ? 1u : 0u;
    b2[0] = b2[1] = 1;
    b2[0x20 / 4] = 1; b2[0x2c / 4] = 1;
    write_float(b2.data(), 0x0c, 1.0f);
    write_float(b2.data(), 0x44, 1.0f);
    write_float(b2.data(), 0x60, 1.0f);
    write_float(b2.data(), 0x68, 0.01f);
    auto cb2 = upload(dev, b2.data(), sizeof(b2));
    auto cb0 = upload(dev, b0.data(), sizeof(b0));
    auto cb1 = upload(dev, b1.data(), sizeof(b1));
    std::array<UINT, 4 * count> counts{};
    auto clear = upload(dev, counts.data(), sizeof(counts));
    auto counter = buffer(dev, sizeof(counts), D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COPY_DEST, true);
    list->CopyBufferRegion(counter.Get(), 0, clear.Get(), 0, sizeof(counts));
    transition(counter.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);

    list->SetPipelineState1(observed.pipeline.Get());
    list->SetComputeRootSignature(observed.root.Get());
    ID3D12DescriptorHeap* heaps[]{heap.Get()}; list->SetDescriptorHeaps(1, heaps);
    auto gpu = heap->GetGPUDescriptorHandleForHeapStart();
    list->SetComputeRootDescriptorTable(0, gpu);
    gpu.ptr += static_cast<UINT64>(srv_count) * step; list->SetComputeRootDescriptorTable(1, gpu);
    list->SetComputeRootConstantBufferView(2, cb2->GetGPUVirtualAddress());
    list->SetComputeRootConstantBufferView(3, cb0->GetGPUVirtualAddress());
    list->SetComputeRootConstantBufferView(4, cb1->GetGPUVirtualAddress());
    list->SetComputeRootUnorderedAccessView(5, counter->GetGPUVirtualAddress());
    const auto address = observed.sbt->GetGPUVirtualAddress();
    D3D12_DISPATCH_RAYS_DESC dispatch{};
    dispatch.RayGenerationShaderRecord = {address,32};
    dispatch.MissShaderTable = {address+64,32,32};
    dispatch.HitGroupTable = {address+128,128,32};
    dispatch.Width = width; dispatch.Height = height; dispatch.Depth = 1;
    list->DispatchRays(&dispatch);
    const UINT count_offset = pitch * height * uav_count;
    for (UINT i = 0; i < uav_count; ++i)
        transition(images[srv_count + i].Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
    transition(counter.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
    auto output = buffer(dev, count_offset + sizeof(counts), D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
    D3D12_TEXTURE_COPY_LOCATION src{}, dst{};
    src.pResource = images[srv_count].Get(); src.Type = D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
    dst.pResource = output.Get(); dst.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT; dst.PlacedFootprint = footprint;
    for (UINT i = 0; i < uav_count; ++i) {
        src.pResource = images[srv_count+i].Get();
        dst.PlacedFootprint.Offset = i * pitch * height;
        list->CopyTextureRegion(&dst, 0, 0, 0, &src, nullptr);
    }
    list->CopyBufferRegion(output.Get(), count_offset, counter.Get(), 0, sizeof(counts));
    submit(dev, queue, list.Get());
    D3D12_RANGE all{0, count_offset + sizeof(counts)}, empty{};
    void* data{};
    check(output->Map(0, &all, &data), "read shadow texture");
    auto* bytes = static_cast<const unsigned char*>(data);
    std::memcpy(counts.data(), bytes + count_offset, sizeof(counts));
    bool matched = true;
    for (UINT y = 0; y < height; ++y) for (UINT x = 0; x < width; ++x) {
        Pixel c{}; std::memcpy(&c, bytes + y * pitch + x * sizeof(Pixel), sizeof(c));
        UINT i = 4 * (y * width + x);
        // Column 1 hits. Material 2 (row 1) traces back through the surface.
        // With zero absorption this second continuation computes exp2(0) and
        // adds 1 on equality, writing 2 to the green and alpha channels.
        const Pixel dark = distance_output ? Pixel{0,0,0,0} : Pixel{0,0,1,0};
        // A resident voxel blocks the open ray without a triangle callback.
        // Material 2 follows up with a second miss against the same geometry.
        const Pixel expected = x == 2 && !occupied ? Pixel{1,1,1,1} :
                               x == 1 && y == 1 ? Pixel{1,2,1,2} : dark;
        const bool masked = exclude_scene && x == 1 && y == 0;
        const bool lit_hair = input == ShadowInput::hair_lit && x == 2 && !y;
        const UINT hits = x == 1 && !masked ? 1 + y : 0;
        const UINT misses = x == 2 ? 1u + (occupied && y ? 1u : 0u) + (lit_hair ? 1u : 0u) : 0u;
        matched &= c == expected && counts[i] == hits && counts[i+1] == misses;
        std::fprintf(receipt, "screen_shadow_%u%s_%u_%u=%.8g,%.8g,%.8g,%.8g;hits %u;misses %u\n",
                     entry,variant,x,y,c[0],c[1],c[2],c[3],counts[i],counts[i+1]);
        if (uav_count == 2) {
            Pixel auxiliary{};
            std::memcpy(&auxiliary, bytes + pitch * height + y * pitch + x * sizeof(Pixel), sizeof(auxiliary));
            if (distance_output) {
                // Entry 3824 writes -1 for a skipped ray or a large finite value for a visible surface.
                UINT wanted = x == 1 && !y ? counts[i+2] : x == 1 || x == 2 ? 0x6c4ecb8f : 0xbf800000;
                for (float channel : auxiliary) matched &= std::memcmp(&channel, &wanted, sizeof(wanted)) == 0;
                std::fprintf(receipt, "screen_distance_%u_%u_%u=%.8g\n", entry,x,y,auxiliary[0]);
            } else {
                // Entry 3810 writes separate hair lighting when hair depth and the light's mask both match.
                matched &= auxiliary == (lit_hair ? Pixel{1,1,1,1} : Pixel{});
                std::fprintf(receipt, "screen_auxiliary_%u%s_%u_%u=%.8g,%.8g,%.8g,%.8g\n",
                             entry,variant,x,y,auxiliary[0],auxiliary[1],auxiliary[2],auxiliary[3]);
            }
        }
    }
    output->Unmap(0, &empty);
    if (!matched) throw std::runtime_error("unexpected shadow texture or traversal callbacks");
}
