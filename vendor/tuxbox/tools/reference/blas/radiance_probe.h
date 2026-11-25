// Each cache tile contains 8 by 8 rays. The native shader stores four half
// values (radiance RGB and trace distance) in one eight-byte structured row.
enum class RadianceInput {
    empty,
    tile,
    geometry,
    far_geometry = 5
};

struct RadianceInputs {
    std::array<ComPtr<ID3D12Resource>, 15> inputs;
    std::array<ComPtr<ID3D12Resource>, 15> staging;

    RadianceInputs(ID3D12Device* dev, ID3D12GraphicsCommandList* list, ID3D12Resource* scene,
                   D3D12_CPU_DESCRIPTOR_HANDLE cpu, RadianceInput input, bool pack, bool xbox, bool surface = false) {
        using Pixel = std::array<float, 4>;
        const bool active = input != RadianceInput::empty, geometry = input == RadianceInput::geometry || input == RadianceInput::far_geometry;
        const UINT step = dev->GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV);
        for (UINT i = 0; i < (xbox ? 15u : 14u); ++i, cpu.ptr += step) {
            D3D12_SHADER_RESOURCE_VIEW_DESC v{};
            v.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
            if (i == 13) {
                if (xbox) {
                    inputs[i] = scene;
                    v.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
                    v.Format = DXGI_FORMAT_R32_TYPELESS;
                    v.Buffer.Flags = D3D12_BUFFER_SRV_FLAG_RAW;
                    v.Buffer.NumElements = 32;
                } else {
                    v.ViewDimension = D3D12_SRV_DIMENSION_RAYTRACING_ACCELERATION_STRUCTURE;
                    v.RaytracingAccelerationStructure.Location = scene->GetGPUVirtualAddress();
                }
            } else if (i == (pack ? 0u : 4u) || (surface && i >= 9 && i <= 11)) {
                const bool sky = i == (pack ? 0u : 4u);
                D3D12_HEAP_PROPERTIES local{};
                local.Type = D3D12_HEAP_TYPE_DEFAULT;
                local.CreationNodeMask = local.VisibleNodeMask = 1;
                D3D12_RESOURCE_DESC tex{};
                tex.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
                tex.Width = tex.Height = tex.MipLevels = tex.SampleDesc.Count = 1;
                tex.DepthOrArraySize = sky ? 6 : 1;
                tex.Format = DXGI_FORMAT_R32G32B32A32_FLOAT;
                check(dev->CreateCommittedResource(&local, D3D12_HEAP_FLAG_NONE, &tex, D3D12_RESOURCE_STATE_COPY_DEST,
                                                   nullptr, IID_PPV_ARGS(&inputs[i])),
                      "radiance sky cube");
                std::array<BYTE, 6 * 512> pixels{};
                for (UINT face = 0; face < tex.DepthOrArraySize; ++face) {
                    Pixel color{float(face + 1), float(face + 2), float(face + 3), 1};
                    if (!sky) color = i == 9 ? Pixel{2, 3, 4, 1} : Pixel{1, 1, 1, 1};
                    std::memcpy(pixels.data() + face * 512, color.data(), sizeof(color));
                }
                staging[i] = upload(dev, pixels.data(), pixels.size());
                for (UINT face = 0; face < tex.DepthOrArraySize; ++face) {
                    D3D12_TEXTURE_COPY_LOCATION src{}, dst{};
                    src.pResource = staging[i].Get();
                    src.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
                    src.PlacedFootprint = {face * 512ull, {tex.Format, 1, 1, 1, 256}};
                    dst.pResource = inputs[i].Get();
                    dst.SubresourceIndex = face;
                    if (xbox)
                        method<frame_abi::Copy>(list, 83)(list, &dst, 0, 0, 0, &src, nullptr, 0);
                    else
                        list->CopyTextureRegion(&dst, 0, 0, 0, &src, nullptr);
                }
                D3D12_RESOURCE_BARRIER bar{};
                bar.Transition = {inputs[i].Get(), D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                                  D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE};
                list->ResourceBarrier(1, &bar);
                v.ViewDimension = sky ? D3D12_SRV_DIMENSION_TEXTURECUBE : D3D12_SRV_DIMENSION_TEXTURE2D;
                v.Format = tex.Format;
                v.TextureCube.MipLevels = 1;
            } else if (i >= 9 && i <= 11) {
                v.ViewDimension = D3D12_SRV_DIMENSION_TEXTURE2D;
                v.Format = DXGI_FORMAT_R32G32B32A32_FLOAT;
                v.Texture2D.MipLevels = 1;
            } else {
                v.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
                v.Buffer.NumElements = 1;
                UINT words[4]{};
                if (i == (pack ? 2u : 12u)) {
                    v.Format = DXGI_FORMAT_R32_UINT;
                    words[0] = active ? 64u : 0u;
                } else if (pack && i == 1) {
                    v.Format = DXGI_FORMAT_R32_UINT;
                } else if (i == 7) {
                    v.Format = DXGI_FORMAT_R32G32_UINT;
                } else if (i == 6) {
                    v.Format = DXGI_FORMAT_R32G32B32A32_FLOAT;
                    const Pixel origin = geometry ? Pixel{input == RadianceInput::far_geometry ? 3.f : -1.f, 0, -0.125f, 0} : Pixel{20, 20, 20, 0};
                    std::memcpy(words, origin.data(), sizeof(origin));
                } else if (i == 2 || i == 3) {
                    v.Format = DXGI_FORMAT_R32_TYPELESS;
                    v.Buffer.Flags = D3D12_BUFFER_SRV_FLAG_RAW;
                } else {
                    v.Buffer.StructureByteStride = i == 14 ? 8 : 16;
                }
                std::array<UINT, 32> card{};
                if (surface && (i == 0 || i == 1 || i == 2)) {
                    const UINT one = 0x3f800000, four = 0x40800000;
                    if (i == 0) {
                        // Seven float4 records per card: transform, bounds, page table and culling box.
                        card[0] = card[5] = one;
                        card[10] = 0xbf800000; // The card receives radiance on its -Z side.
                        card[12] = card[13] = card[14] = four;
                        card[15] = 1u << 16;
                        card[16] = 0x00010001;
                        card[24] = card[25] = card[26] = four;
                        v.Buffer.NumElements = 7;
                    } else if (i == 1) {
                        // One mesh-card group. Keep the card eligible for both Z directions;
                        // its own projected-normal lighting term rejects the reversed winding.
                        card[0] = card[5] = card[10] = one;
                        card[12] = card[17] = card[22] = one;
                        card[30] = card[31] = 1;
                        v.Buffer.NumElements = 8;
                    } else {
                        card[0] = 0x11000000; // One resident page, X/Y levels both one.
                        card[1] = 4;
                        v.Buffer.NumElements = 2;
                    }
                    inputs[i] = upload(dev, card.data(), sizeof(card));
                } else if (i == 8 && xbox) {
                    // InstanceID 7 addresses the first test mesh's 38-float4 record.
                    std::array<UINT, 9 * 38 * 4> instance_data{};
                    v.Buffer.NumElements = UINT(instance_data.size() / 4);
                    inputs[i] = upload(dev, instance_data.data(), sizeof(instance_data));
                } else {
                    inputs[i] = upload(dev, words, sizeof(words));
                }
            }
            dev->CreateShaderResourceView(inputs[i].Get(), &v, cpu);
        }
    }
};

#ifdef TUXBOX_INLINE_RADIANCE
#include "inline318_replay.h"
#endif

static void probe_radiance_cache(ID3D12Device5* dev, ID3D12CommandQueue* queue, ID3D12Resource* scene, FILE* receipt,
                                 RadianceInput input, UINT entry = 323,
                                 const std::array<UINT, 325>* supplied_retrace = nullptr,
                                 ID3D12Resource* produced_records = nullptr) {
    (void)produced_records;
    const bool pack = entry == 266;
    const bool active = input != RadianceInput::empty, geometry = input == RadianceInput::geometry || input == RadianceInput::far_geometry;
    const char* label = input == RadianceInput::far_geometry ? "far_geometry" : geometry ? "geometry" : active ? "tile" : "empty";
    constexpr UINT width = 64, records = 65;
    using Pixel = std::array<float, 4>;
    D3D12_DESCRIPTOR_RANGE range{D3D12_DESCRIPTOR_RANGE_TYPE_SRV, 14, 0, 1, 0};
    D3D12_ROOT_PARAMETER params[9]{};
    params[0].ParameterType = D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE;
    params[0].DescriptorTable = {1, &range};
    for (UINT i = 0; i < 4; ++i) {
        params[1 + i].ParameterType = D3D12_ROOT_PARAMETER_TYPE_CBV;
        params[1 + i].Descriptor = {i, 1};
    }
    for (UINT i = 0; i < 4; ++i) {
        params[5 + i].ParameterType = D3D12_ROOT_PARAMETER_TYPE_UAV;
        params[5 + i].Descriptor = {i % 3, i == 3 ? 2u : 1u};
    }
    D3D12_STATIC_SAMPLER_DESC samplers[2]{};
    for (UINT i = 0; i < 2; ++i) {
        auto& s = samplers[i];
        s.Filter = D3D12_FILTER_MIN_MAG_MIP_POINT;
        s.AddressU = s.AddressV = s.AddressW = D3D12_TEXTURE_ADDRESS_MODE_CLAMP;
        s.ComparisonFunc = D3D12_COMPARISON_FUNC_ALWAYS;
        s.MaxAnisotropy = 1;
        s.MaxLOD = D3D12_FLOAT32_MAX;
        s.ShaderRegister = i;
        s.RegisterSpace = 1;
    }
    D3D12_ROOT_SIGNATURE_DESC rs{ARRAYSIZE(params), params, 2, samplers, D3D12_ROOT_SIGNATURE_FLAG_NONE};
    const wchar_t* name = pack           ? L"LumenRadianceCacheHardwareRayTracingRGS_43a1a5a6"
                          : entry == 299 ? L"LumenRadianceCacheHardwareRayTracingRGS_02684120"
                          : entry == 311 ? L"LumenRadianceCacheHardwareRayTracingRGS_b3809f47"
                          : entry == 264 ? L"LumenRadianceCacheHardwareRayTracingRGS_0ae72f63"
                                         : L"LumenRadianceCacheHardwareRayTracingRGS_8c299e02";
    ObservedRayPipeline observed(dev, rs, entry, name, "ray-input-observer.dxil");
    ComPtr<ID3D12CommandAllocator> alloc;
    ComPtr<ID3D12GraphicsCommandList4> list;
    check(dev->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&alloc)), "radiance allocator");
    check(dev->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, alloc.Get(), nullptr, IID_PPV_ARGS(&list)),
          "radiance list");
    auto transition = [&](ID3D12Resource* res, D3D12_RESOURCE_STATES from, D3D12_RESOURCE_STATES to) {
        D3D12_RESOURCE_BARRIER bar{};
        bar.Transition = {res, D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES, from, to};
        list->ResourceBarrier(1, &bar);
    };
    ComPtr<ID3D12DescriptorHeap> heap;
    D3D12_DESCRIPTOR_HEAP_DESC hd{D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 14, D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                                  0};
    check(dev->CreateDescriptorHeap(&hd, IID_PPV_ARGS(&heap)), "radiance descriptors");
    RadianceInputs resources(dev, list.Get(), scene, heap->GetCPUDescriptorHandleForHeapStart(), input, pack, false);
    std::array<std::array<UINT, 1024>, 4> cb{};
    auto f = [&](UINT c, UINT offset, float x) { std::memcpy(&cb[c][offset / 4], &x, sizeof(x)); };
    cb[0][0x398 / 4] = 1;
    cb[0][0x1f0 / 4] = 16;
    cb[0][0x3b4 / 4] = 1;
    f(0, 0x1a8, geometry ? 0.001f : 17.0f);
    f(0, 0x1ac, 17);
    // The far-field retrace starts at the end of the near-field sphere.
    // Give it a positive far range and a zero-radius near field in this scene.
    f(0, 0x3a0, entry == 311 ? 100.0f : 0.0f);
    f(0, 0x3a4, entry == 311 ? 0.0f : 100.0f);
    f(1, 0xbe0, 1);
    f(1, 0xbe4, 1);
    f(1, 0xbe8, 1);
    f(2, 4, 1);
    f(3, 4, 1);
    std::array<ComPtr<ID3D12Resource>, 4> constants;
    ID3D12DescriptorHeap* heaps[]{heap.Get()};
    list->SetDescriptorHeaps(1, heaps);
    list->SetComputeRootSignature(observed.root.Get());
    list->SetComputeRootDescriptorTable(0, heap->GetGPUDescriptorHandleForHeapStart());
    for (UINT i = 0; i < 4; ++i) {
        constants[i] = upload(dev, cb[i].data(), sizeof(cb[i]));
        list->SetComputeRootConstantBufferView(i + 1, constants[i]->GetGPUVirtualAddress());
    }
    std::array<UINT, records * 2> initial{};
    initial.fill(0xdeadbeefu);
    std::array<UINT, width * 12> counts{};
    auto initial_gpu = upload(dev, initial.data(), sizeof(initial)), zeros = upload(dev, counts.data(), sizeof(counts));
    std::array<UINT, records * 5> retrace_words{};
    retrace_words.fill(0xdeadbeefu);
    if (supplied_retrace)
        retrace_words = *supplied_retrace;
    else if (entry == 264 || entry == 311)
        for (UINT i = 0; i < width; ++i)
            retrace_words[5 * i] = i;
    auto retrace_src = upload(dev, retrace_words.data(), sizeof(retrace_words));
    auto retrace = buffer(dev, sizeof(retrace_words), D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COPY_DEST, true);
    list->CopyBufferRegion(retrace.Get(), 0, retrace_src.Get(), 0, sizeof(retrace_words));
    transition(retrace.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
    auto work = buffer(dev, sizeof(initial), D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COPY_DEST, true);
    auto calls = buffer(dev, sizeof(counts), D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COPY_DEST, true);
    auto page_usage = buffer(dev, 256, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, true);
    list->CopyBufferRegion(work.Get(), 0, initial_gpu.Get(), 0, sizeof(initial));
    list->CopyBufferRegion(calls.Get(), 0, zeros.Get(), 0, sizeof(counts));
    transition(work.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
    transition(calls.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
    list->SetComputeRootUnorderedAccessView(5, (pack ? work : page_usage)->GetGPUVirtualAddress());
    list->SetComputeRootUnorderedAccessView(6, (pack ? retrace : work)->GetGPUVirtualAddress());
    list->SetComputeRootUnorderedAccessView(7, retrace->GetGPUVirtualAddress());
    list->SetComputeRootUnorderedAccessView(8, calls->GetGPUVirtualAddress());
    list->SetPipelineState1(observed.pipeline.Get());
    const auto addr = observed.sbt->GetGPUVirtualAddress();
    D3D12_DISPATCH_RAYS_DESC dispatch{};
    dispatch.RayGenerationShaderRecord = {addr, 32};
    dispatch.MissShaderTable = {addr + 64, 32, 32};
    dispatch.HitGroupTable = {addr + 128, 128, 32};
    dispatch.Width = width;
    dispatch.Height = dispatch.Depth = 1;
    list->DispatchRays(&dispatch);
    auto readback = buffer(dev, sizeof(initial) + sizeof(counts) + sizeof(retrace_words), D3D12_HEAP_TYPE_READBACK,
                           D3D12_RESOURCE_STATE_COPY_DEST);
    transition(work.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
    transition(calls.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
    list->CopyBufferRegion(readback.Get(), 0, work.Get(), 0, sizeof(initial));
    list->CopyBufferRegion(readback.Get(), sizeof(initial), calls.Get(), 0, sizeof(counts));
    transition(retrace.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
    list->CopyBufferRegion(readback.Get(), sizeof(initial) + sizeof(counts), retrace.Get(), 0, sizeof(retrace_words));
    submit(dev, queue, list.Get());
    D3D12_RANGE r{0, sizeof(initial) + sizeof(counts) + sizeof(retrace_words)}, none{};
    void* data{};
    check(readback->Map(0, &r, &data), "radiance readback");
    std::memcpy(initial.data(), data, sizeof(initial));
    std::memcpy(counts.data(), static_cast<BYTE*>(data) + sizeof(initial), sizeof(counts));
    std::memcpy(retrace_words.data(), static_cast<BYTE*>(data) + sizeof(initial) + sizeof(counts),
                sizeof(retrace_words));
    readback->Unmap(0, &none);
    const UINT half[]{0, 0x3c00, 0x4000, 0x4200, 0x4400, 0x4500, 0x4600, 0x4700, 0x4800};
    bool matched = true;
    UINT faces = 0, hit_count = 0;
    for (UINT i = 0; i < records; ++i) {
        // The retrace list encodes (Y,X) in its low six bits. Its outputs use (X,Y).
        const UINT invocation = !supplied_retrace && (entry == 264 || entry == 311) ? (i % 8) * 8 + i / 8 : i;
        UINT hits = i < width ? counts[4 * invocation] : 0, misses = i < width ? counts[4 * invocation + 1] : 0,
             face = i < width ? counts[4 * invocation + 3] : 0;
        const UINT rg = initial[2 * i], bd = initial[2 * i + 1];
        float t = 0.0f;
        if (i < width)
            std::memcpy(&t, counts.data() + 4 * invocation + 2, sizeof(t));
        std::fprintf(receipt, "radiance_%u_%s_%u=%08x,%08x;hits %u;misses %u;face %u;t %.8g\n", entry, label, i, rg, bd,
                     hits, misses, face, t);
        if ((entry == 299 || entry == 311) && geometry && i < width) {
            Pixel origin{}, ray{};
            std::memcpy(origin.data(), counts.data() + 4 * (width + invocation), sizeof(origin));
            std::memcpy(ray.data(), counts.data() + 4 * (2 * width + invocation), sizeof(ray));
            std::fprintf(receipt, "radiance_ray_%u_%u=%.8g,%.8g,%.8g,%.8g;%.8g,%.8g,%.8g,%.8g\n",
                         entry, i, origin[0], origin[1], origin[2], origin[3], ray[0], ray[1], ray[2], ray[3]);
        }
        if (entry == 299 || entry == 311) {
            auto* rec = retrace_words.data() + 5 * i;
            std::fprintf(receipt, "radiance_record_%u_%s_%u=%08x,%08x,%08x,%08x,%08x\n", entry, label, i,
                         rec[0], rec[1], rec[2], rec[3], rec[4]);
            if (supplied_retrace) {
                matched &= std::equal(rec, rec + 5, supplied_retrace->data() + 5*i);
            } else if (!active || i == width) {
                matched &= std::all_of(rec, rec + 5, [](UINT w) { return w == 0xdeadbeefu; });
            } else if (entry == 311) {
                matched &= rec[0] == i && std::all_of(rec + 1, rec + 5, [](UINT w) { return w == 0xdeadbeefu; });
            } else {
                matched &= rec[0] == (i % 8) * 8 + i / 8 && rec[3] == ~0u && rec[4] == ~0u;
                if (misses) matched &= rec[1] == ~0u && rec[2] == 0x41880000u;
            }
        }
        if (!active || i == width) {
            matched &= rg == 0xdeadbeefu && bd == 0xdeadbeefu && !hits && !misses;
        } else if (pack) {
            matched &= !hits && !misses;
            auto* rec = retrace_words.data() + 5 * i;
            // Decode the octahedral tile independently of the native packer.
            // The cube has a different constant RGB on each face.
            const UINT x = i % 8, y = i / 8;
            float dx = (float(x) + .5f) * .25f - 1.0f;
            float dy = (float(y) + .5f) * .25f - 1.0f;
            const float dz = 1.0f - std::fabs(dx) - std::fabs(dy);
            const float fold = dz < 0 ? -dz : 0.0f;
            dx += dx >= 0 ? -fold : fold;
            dy += dy >= 0 ? -fold : fold;
            const UINT sky_face = std::fabs(dx) > std::fabs(dy) && std::fabs(dx) > std::fabs(dz) ? (dx < 0 ? 1u : 0u)
                                : std::fabs(dy) > std::fabs(dz) ? (dy < 0 ? 3u : 2u) : (dz < 0 ? 5u : 4u);
            matched &= rg == (half[sky_face + 1] | (half[sky_face + 2] << 16)) &&
                       bd == (half[sky_face + 3] | (0x4c40u << 16));
            matched &= rec[0] == x * 8 + y && rec[1] == ~0u && rec[2] == 0x41880000u && rec[3] == ~0u && rec[4] == ~0u;
            std::fprintf(receipt, "radiance_pack_%u=%08x,%08x,%08x,%08x,%08x\n", i, rec[0], rec[1], rec[2], rec[3],
                         rec[4]);
        } else if (!hits) {
            matched &= misses == (entry == 264 ? 2u : 1u) && face < 6;
            if (face < 6) {
                faces |= 1u << face;
                matched &= rg == (half[face + 1] | (half[face + 2] << 16)) && bd == (half[face + 3] | (0x4c40u << 16));
            }
        } else {
            hit_count += hits;
            matched &= geometry && hits == 1 && misses == 0 && rg == 0 && (bd & 0xffffu) == 0;
            // The fixture's positive, normal hit distances fit binary16. Round
            // the independent traversal callback to the cache record's precision.
            if (t > 0.01f && t < 2.0f) {
                int exponent;
                float fraction = std::frexp(t, &exponent);
                UINT significand = static_cast<UINT>(std::nearbyint(fraction * 2048.0f));
                matched &= (bd >> 16) == (static_cast<UINT>(exponent + 13) << 10) + significand;
            } else
                matched = false;
        }
    }
    if (!geometry && active && !pack)
        matched &= faces == 63;
    if (geometry) {
        UINT expected_hits = 18;
#ifdef TUXBOX_INLINE313_REPLAY
        if ((entry == 311) != (input == RadianceInput::far_geometry)) expected_hits = 0;
#endif
        matched &= hit_count == expected_hits;
    }
    if (!matched)
        throw std::runtime_error("radiance tile differs from its ray callbacks");
#ifdef TUXBOX_INLINE_RADIANCE
    if (entry != 311) {
        ComPtr<ID3D12Resource> produced;
        std::array<UINT, 325> actual_retrace{};
        replay_inline_radiance(input, cb, initial, retrace_words, hit_count, receipt, UINT(input), actual_retrace, produced);
        if constexpr (TUXBOX_INLINE_RADIANCE == 306)
            probe_radiance_cache(dev, queue, scene, receipt, input, 311, &actual_retrace, produced.Get());
        if (input == RadianceInput::geometry) {
            cb[2][0] = cb[2][1] = 1; // One card and one mesh-card group.
            f(2, 24, 1);
            f(2, 28, 1);
            cb[0][0xe8 / 4] = 1;
            cb[0][0xfc / 4] = 123; // Feedback frame marker.
            replay_inline_radiance(input, cb, initial, retrace_words, hit_count, receipt, 3, actual_retrace, produced);
            replay_inline_radiance(input, cb, initial, retrace_words, hit_count, receipt, 4, actual_retrace, produced);
        }
    }
#ifdef TUXBOX_INLINE313_REPLAY
    else if (supplied_retrace && produced_records) {
        ComPtr<ID3D12Resource> same_allocation = produced_records;
        std::array<UINT, 325> untouched{};
        replay_inline_radiance(input, cb, initial, *supplied_retrace, hit_count, receipt, UINT(input),
                               untouched, same_allocation, true);
    }
#endif
#endif
    std::fprintf(receipt, "radiance_%u_%s_triangle_hits=%u\n", entry, label, hit_count);
}
