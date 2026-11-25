// The compacted reflection queue has two jobs and a third inactive lane.
// Queue order differs from pixel order so flat dispatch writes are detectable.
#include "inline_scene_fixture.h"
struct ReflectionOutput {
    std::array<std::array<float, 4>, 3> pixels{};
    std::array<float, 3> distance{};
    std::array<UINT, 15> records{};
};
static ReflectionOutput probe_reflections(ID3D12Device5 *native, ID3D12CommandQueue *queue, ID3D12Resource *scene,
                                          FILE *receipt, UINT entry, bool geometry = false,
                                          InlineSceneFixture *compute = nullptr,
                                          const ReflectionOutput *reference = nullptr, bool masked = false) {
    auto *dev = compute ? compute->dev.Get() : static_cast<ID3D12Device *>(native);
    if (compute) {
        queue = compute->queue.Get();
        scene = compute->scene.Get();
    }
    using Pixel = std::array<float, 4>;
    const bool compact = entry == 434 || entry == 426 || entry == 456 || entry == 421 || entry == 432;
    const bool cache = entry == 426 || entry == 425 || entry == 421;
    const UINT sky = compact ? 4 : 0, normal = compact ? 5 : 4, depth = compact ? 10 : 5;
    const UINT direction = compact ? 11 : 6, trace_distance = compact ? 12 : 7;
    const UINT jobs = compact ? (cache ? 16 : 13) : cache ? 11 : 8, pairs = jobs + 1, scene_index = pairs + 1, srvs = scene_index + 1;
    constexpr UINT width = 3, pitch = D3D12_TEXTURE_DATA_PITCH_ALIGNMENT;
    D3D12_DESCRIPTOR_RANGE ranges[]{{D3D12_DESCRIPTOR_RANGE_TYPE_SRV, srvs, 0, 1, 0},
                                    {D3D12_DESCRIPTOR_RANGE_TYPE_UAV, 2, compact ? 4u : 1u, 1, 0}};
    D3D12_ROOT_PARAMETER params[11]{};
    for (UINT i = 0; i < 2; ++i) {
        params[i].ParameterType = D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE;
        params[i].DescriptorTable = {1, ranges + i};
    }
    for (UINT i = 0; i < 4; ++i) {
        params[2 + i].ParameterType = D3D12_ROOT_PARAMETER_TYPE_CBV;
        params[2 + i].Descriptor = {i, 1};
    }
    params[6].ParameterType = params[7].ParameterType = D3D12_ROOT_PARAMETER_TYPE_UAV;
    params[6].Descriptor = {0, 2};
    params[7].Descriptor = {0, 1};
    for (UINT i = 1; i < 4; ++i) {
        params[7 + i].ParameterType = D3D12_ROOT_PARAMETER_TYPE_UAV;
        params[7 + i].Descriptor = {i, 1};
    }
    D3D12_STATIC_SAMPLER_DESC samplers[4]{};
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
    D3D12_ROOT_SIGNATURE_DESC rs{compact ? 11u : 8u, params, ARRAYSIZE(samplers), samplers,
                                 D3D12_ROOT_SIGNATURE_FLAG_NONE};
    std::optional<ObservedRayPipeline> observed;
    if (!compute)
        observed.emplace(native, rs, entry,
                         entry == 421   ? L"LumenReflectionHardwareRayTracingRGS_8d5b0e96"
                         : entry == 432 ? L"LumenReflectionHardwareRayTracingRGS_78010894"
                         : compact      ? L"LumenReflectionHardwareRayTracingRGS_af06c2ee"
                         : cache        ? L"LumenReflectionHardwareRayTracingRGS_74627350"
                                        : L"LumenReflectionHardwareRayTracingRGS_7f9f89a2",
                         "ray-input-observer.dxil");
    ComPtr<ID3D12CommandAllocator> allocator;
    ComPtr<ID3D12GraphicsCommandList4> list;
    check(dev->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&allocator)),
          "reflection allocator");
    if (compute)
        check(guest_list(dev, allocator.Get()).As(&list), "Xbox reflection list");
    else
        check(dev->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, allocator.Get(), nullptr, IID_PPV_ARGS(&list)),
              "reflection list");
    auto transition = [&](ID3D12Resource *r, D3D12_RESOURCE_STATES from, D3D12_RESOURCE_STATES to) {
        D3D12_RESOURCE_BARRIER b{};
        b.Transition = {r, D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES, from, to};
        list->ResourceBarrier(1, &b);
    };
    D3D12_DESCRIPTOR_HEAP_DESC hd{D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, compute ? 96u : srvs + 2,
                                  D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, 0};
    ComPtr<ID3D12DescriptorHeap> heap;
    check(dev->CreateDescriptorHeap(&hd, IID_PPV_ARGS(&heap)), "reflection descriptors");
    const UINT step = dev->GetDescriptorHandleIncrementSize(hd.Type);
    std::optional<InlineComputeTables> tables;
    if (compute)
        tables.emplace(dev, heap.Get(), 4);
    auto cpu = compute ? tables->view(0) : heap->GetCPUDescriptorHandleForHeapStart();
    auto copy = [&](D3D12_TEXTURE_COPY_LOCATION &dst, D3D12_TEXTURE_COPY_LOCATION &src) {
        if (compute)
            method<frame_abi::Copy>(list.Get(), 83)(list.Get(), &dst, 0, 0, 0, &src, nullptr, 0);
        else
            list->CopyTextureRegion(&dst, 0, 0, 0, &src, nullptr);
    };
    std::vector<ComPtr<ID3D12Resource>> images(srvs + 2), uploads(srvs + 2);
    D3D12_HEAP_PROPERTIES local{};
    local.Type = D3D12_HEAP_TYPE_DEFAULT;
    local.CreationNodeMask = local.VisibleNodeMask = 1;
    for (UINT i = 0; i < srvs + 2; ++i, cpu.ptr += step) {
        D3D12_SHADER_RESOURCE_VIEW_DESC v{};
        v.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
        if (i == srvs && compute) {
            D3D12_SHADER_RESOURCE_VIEW_DESC extra{};
            extra.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
            extra.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
            extra.Buffer.StructureByteStride = 8;
            extra.Buffer.NumElements = 1;
            dev->CreateShaderResourceView(nullptr, &extra, cpu);
            cpu = tables->view(84);
        }
        if (i == scene_index && compute) {
            v.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
            v.Format = DXGI_FORMAT_R32_TYPELESS;
            v.Buffer.NumElements = UINT(scene->GetDesc().Width / 4);
            v.Buffer.Flags = D3D12_BUFFER_SRV_FLAG_RAW;
            dev->CreateShaderResourceView(scene, &v, cpu);
        } else if (i == scene_index) {
            v.ViewDimension = D3D12_SRV_DIMENSION_RAYTRACING_ACCELERATION_STRUCTURE;
            v.RaytracingAccelerationStructure.Location = scene->GetGPUVirtualAddress();
            dev->CreateShaderResourceView(nullptr, &v, cpu);
        } else if (i == jobs || i == pairs) {
            const UINT ray_bounds = geometry ? 0x4c400000u : 0x4c404c40u;
            const std::array<UINT, 4> data =
                i == jobs ? std::array<UINT, 4>{2, 0, 0, 0} : std::array<UINT, 4>{1, ray_bounds, 0, ray_bounds};
            images[i] = upload(dev, data.data(), sizeof(data));
            v.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
            v.Format = i == jobs ? DXGI_FORMAT_R32_UINT : DXGI_FORMAT_R32G32_UINT;
            v.Buffer.NumElements = i == jobs ? 1 : 2;
            dev->CreateShaderResourceView(images[i].Get(), &v, cpu);
        } else if (compact && cache && (i == 13 || i == 14)) {
            v.Format = i == 13 ? DXGI_FORMAT_R32_UINT : DXGI_FORMAT_R32G32B32A32_FLOAT;
            v.ViewDimension = i == 13 ? D3D12_SRV_DIMENSION_TEXTURE3D : D3D12_SRV_DIMENSION_TEXTURE2D;
            if (i == 13) v.Texture3D.MipLevels = 1; else v.Texture2D.MipLevels = 1;
            dev->CreateShaderResourceView(nullptr, &v, cpu);
        } else if (compact && (i <= 3 || i == 6 || (cache && i == 15))) {
            v.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
            v.Buffer.NumElements = 1;
            if (i == 2 || i == 3) {
                v.Format = DXGI_FORMAT_R32_TYPELESS;
                v.Buffer.Flags = D3D12_BUFFER_SRV_FLAG_RAW;
            } else
                v.Buffer.StructureByteStride = 16;
            dev->CreateShaderResourceView(nullptr, &v, cpu);
        } else if (!compact && (i == 1 || i == 2 || i == 3 || (cache && i >= 8 && i <= 10))) {
            if (i == 8 || i == 9) {
                v.ViewDimension = i == 8 ? D3D12_SRV_DIMENSION_TEXTURE3D : D3D12_SRV_DIMENSION_TEXTURE2D;
                v.Format = i == 8 ? DXGI_FORMAT_R32_UINT : DXGI_FORMAT_R32G32B32A32_FLOAT;
                if (i == 8)
                    v.Texture3D.MipLevels = 1;
                else
                    v.Texture2D.MipLevels = 1;
            } else {
                v.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
                v.Buffer.NumElements = 1;
                if (i == 2)
                    v.Format = DXGI_FORMAT_R32_UINT;
                else
                    v.Buffer.StructureByteStride = 16;
            }
            dev->CreateShaderResourceView(nullptr, &v, cpu);
        } else {
            const bool cube = i == sky, out = i >= srvs, scalar = i == trace_distance || i == srvs + 1;
            D3D12_RESOURCE_DESC tex{};
            tex.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
            tex.Width = cube ? 1 : width;
            tex.Height = tex.SampleDesc.Count = tex.MipLevels = 1;
            tex.DepthOrArraySize = cube ? 6 : 1;
            tex.Format = i == trace_distance   ? DXGI_FORMAT_R32_UINT
                         : scalar ? DXGI_FORMAT_R32_FLOAT
                                  : DXGI_FORMAT_R32G32B32A32_FLOAT;
            if (out)
                tex.Flags = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
            check(dev->CreateCommittedResource(&local, D3D12_HEAP_FLAG_NONE, &tex, D3D12_RESOURCE_STATE_COPY_DEST,
                                               nullptr, IID_PPV_ARGS(&images[i])),
                  "reflection image");
            std::array<BYTE, 6 * 512> data{};
            for (UINT face = 0; face < (cube ? 6u : 1u); ++face)
                for (UINT x = 0; x < tex.Width; ++x) {
                    Pixel value = cube     ? Pixel{float(face + 2), float(face + 3), float(face + 4), 1}
                                  : i == normal ? Pixel{.5f, .5f, 1, 0}
                                  : i == depth ? Pixel{1, 0, 0, 0}
                                  : i == direction ? Pixel{0, 0, x == 0 ? 1.f : -1.f, 0}
                                           : Pixel{-99, -99, -99, -99};
                    UINT packed_distance = geometry && (entry == 432 || entry == 434) ? 0x2a66u : 0x4c40u;
                    const size_t bytes = scalar ? 4 : sizeof(value);
                    const void *input = i == trace_distance ? static_cast<const void *>(&packed_distance)
                                                            : static_cast<const void *>(&value);
                    std::memcpy(data.data() + face * 512 + x * bytes, input, bytes);
                }
            uploads[i] = upload(dev, data.data(), sizeof(data));
            D3D12_TEXTURE_COPY_LOCATION src{}, dst{};
            src.pResource = uploads[i].Get();
            src.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
            src.PlacedFootprint.Footprint = {tex.Format, UINT(tex.Width), 1, 1, pitch};
            dst.pResource = images[i].Get();
            for (UINT face = 0; face < (cube ? 6u : 1u); ++face) {
                src.PlacedFootprint.Offset = face * 512;
                dst.SubresourceIndex = face;
                copy(dst, src);
            }
            if (out)
                dev->CreateUnorderedAccessView(images[i].Get(), nullptr, nullptr, cpu);
            else {
                v.ViewDimension = cube ? D3D12_SRV_DIMENSION_TEXTURECUBE : D3D12_SRV_DIMENSION_TEXTURE2D;
                v.Format = tex.Format;
                if (cube)
                    v.TextureCube.MipLevels = 1;
                else
                    v.Texture2D.MipLevels = 1;
                dev->CreateShaderResourceView(images[i].Get(), &v, cpu);
            }
            transition(images[i].Get(), D3D12_RESOURCE_STATE_COPY_DEST,
                       out ? D3D12_RESOURCE_STATE_UNORDERED_ACCESS : D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
        }
    }
    std::array<std::array<UINT, 1024>, 4> cb{};
    auto f = [&](UINT i, UINT offset, float n) { std::memcpy(&cb[i][offset / 4], &n, 4); };
    cb[0][0x1b0 / 4] = 256;
    cb[0][0x190 / 4] = width;
    cb[0][0x194 / 4] = 1;
    cb[0][0x1e0 / 4] = 1;
    cb[0][0x1b4 / 4] = 1; // include the sky on a reflection miss
    f(0, 0x19c, 17);
    f(0, 0x1a0, 17);
    f(0, 0x1f8, 100);
    f(1, 0x330, masked ? 3.f : geometry ? -1.0f : 20.0f);
    f(1, 0x334, geometry ? 0.0f : 20.0f);
    f(1, 0x338, geometry ? .125f : 20.0f);
    f(1, 0x33c, 1);
    f(1, 0x420, 1);
    f(1, 0x424, 1);
    f(1, 0x860, 3);
    f(1, 0x864, 1);
    f(1, 0x880, 3);
    f(1, 0x884, 1);
    f(1, 0x888, 1.f / 3);
    f(1, 0x88c, 1);
    f(1, 0x8b4, 1);
    f(1, 0xbe0, 1);
    f(1, 0xbe4, 1);
    f(1, 0xbe8, 1);
    f(2, 4, 1);
    f(3, 4, 1);
    std::array<UINT, 15> jobs_data{};
    jobs_data[5] = 1;
    if (entry == 432 || entry == 434) {
        for (UINT i = 0; i < 10; ++i)
            if (i % 5 != 0) jobs_data[i] = 0xabcd0000u + i;
    }
    for (UINT i = 10; i < 15; ++i) jobs_data[i] = 0xdeadbeefu;
    auto jobs_src = upload(dev, jobs_data.data(), sizeof(jobs_data));
    auto work = buffer(dev, sizeof(jobs_data), D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COPY_DEST, true);
    list->CopyBufferRegion(work.Get(), 0, jobs_src.Get(), 0, sizeof(jobs_data));
    transition(work.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
    std::array<UINT, width * 12> counts{};
    auto zero = upload(dev, counts.data(), sizeof(counts));
    auto counters = buffer(dev, sizeof(counts), D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COPY_DEST, true);
    list->CopyBufferRegion(counters.Get(), 0, zero.Get(), 0, sizeof(counts));
    transition(counters.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
    std::array<ComPtr<ID3D12Resource>, 4> constants;
    if (!compute) {
        list->SetComputeRootSignature(observed->root.Get());
        list->SetPipelineState1(observed->pipeline.Get());
        ID3D12DescriptorHeap *heaps[]{heap.Get()};
        list->SetDescriptorHeaps(1, heaps);
        auto gpu = heap->GetGPUDescriptorHandleForHeapStart();
        list->SetComputeRootDescriptorTable(0, gpu);
        gpu.ptr += UINT64(srvs) * step;
        list->SetComputeRootDescriptorTable(1, gpu);
    }
    for (UINT i = 0; i < 4; ++i) {
        constants[i] = upload(dev, cb[i].data(), sizeof(cb[i]));
        if (compute)
            tables->cbv(i, constants[i].Get(), sizeof(cb[i]));
        else
            list->SetComputeRootConstantBufferView(2 + i, constants[i]->GetGPUVirtualAddress());
    }
    if (!compute)
        list->SetComputeRootUnorderedAccessView(6, counters->GetGPUVirtualAddress());
    std::array<ComPtr<ID3D12Resource>, 3> feedback;
    if (compact) {
        for (UINT i = 0; i < feedback.size(); ++i) {
            feedback[i] = buffer(dev, sizeof(counts), D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COPY_DEST, true);
            list->CopyBufferRegion(feedback[i].Get(), 0, zero.Get(), 0, sizeof(counts));
            transition(feedback[i].Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
            if (compute)
                tables->buffer_uav(i, feedback[i].Get(), i == 2 ? 8u : 4u, sizeof(counts));
            else
                list->SetComputeRootUnorderedAccessView(7 + i, feedback[i]->GetGPUVirtualAddress());
        }
    }
    if (compute) {
        tables->buffer_uav(3, work.Get(), 20, sizeof(jobs_data));
        tables->dispatch(list.Get(), *compute);
    } else {
        list->SetComputeRootUnorderedAccessView(compact ? 10 : 7, work->GetGPUVirtualAddress());
        auto table = observed->sbt->GetGPUVirtualAddress();
        D3D12_DISPATCH_RAYS_DESC dispatch{};
        dispatch.RayGenerationShaderRecord = {table, 32};
        dispatch.MissShaderTable = {table + 64, 32, 32};
        dispatch.HitGroupTable = {table + 128, 128, 32};
        dispatch.Width = width;
        dispatch.Height = dispatch.Depth = 1;
        list->DispatchRays(&dispatch);
    }
    auto readback = buffer(dev, 1024 + sizeof(counts) + sizeof(jobs_data), D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
    for (UINT i = 0; i < 2; ++i) {
        auto *r = images[srvs + i].Get();
        transition(r, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
        D3D12_TEXTURE_COPY_LOCATION src{}, dst{};
        src.pResource = r;
        dst.pResource = readback.Get();
        dst.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
        dst.PlacedFootprint = {i * 512, {r->GetDesc().Format, width, 1, 1, pitch}};
        copy(dst, src);
    }
    transition(counters.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
    list->CopyBufferRegion(readback.Get(), 1024, counters.Get(), 0, sizeof(counts));
    transition(work.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
    list->CopyBufferRegion(readback.Get(), 1024 + sizeof(counts), work.Get(), 0, sizeof(jobs_data));
    submit(dev, queue, list.Get());
    D3D12_RANGE range{0, 1024 + sizeof(counts) + sizeof(jobs_data)}, empty{};
    void *mem{};
    check(readback->Map(0, &range, &mem), "reflection output");
    const auto *bytes = static_cast<const BYTE *>(mem);
    std::memcpy(counts.data(), bytes + 1024, sizeof(counts));
    std::memcpy(jobs_data.data(), bytes + 1024 + sizeof(counts), sizeof(jobs_data));
    ReflectionOutput result{};
    std::memcpy(result.pixels.data(), bytes, sizeof(result.pixels));
    std::memcpy(result.distance.data(), bytes + 512, sizeof(result.distance));
    std::copy_n(jobs_data.begin(), result.records.size(), result.records.begin());
    if (compute) {
        readback->Unmap(0, &empty);
        // Both packaged writers store RGB. The fourth channel exists only in the test texture.
        bool colors = reference != nullptr;
        if (reference)
            for (UINT x = 0; x < width; ++x)
                colors &= std::equal(result.pixels[x].begin(), result.pixels[x].begin() + (x == 2 ? 4 : 3),
                                     reference->pixels[x].begin());
        const bool ranges = reference && reference->distance == result.distance;
        const bool jobs_ok = reference && reference->records == result.records;
        std::fprintf(receipt,
                     "inline%u_%u=colors %u, distances %u, records %u; rgb [%g,%g,%g] [%g,%g,%g]; distances %g,%g,%g; "
                     "hit record %08x,%08x,%08x,%08x,%08x\n",
                     entry, masked ? 2u : UINT(geometry), colors, ranges, jobs_ok, result.pixels[0][0],
                     result.pixels[0][1], result.pixels[0][2], result.pixels[1][0], result.pixels[1][1],
                     result.pixels[1][2], result.distance[0], result.distance[1], result.distance[2], result.records[0],
                     result.records[1], result.records[2], result.records[3], result.records[4]);
        if (!colors || !ranges || !jobs_ok)
            throw std::runtime_error("reflection differs from packaged ray-generation control");
        return result;
    }
    bool matched = true;
    if (geometry) {
        for (UINT i = 0; i < 2; ++i) {
            Pixel origin{}, ray{};
            std::memcpy(origin.data(), counts.data() + 4 * (width + i), sizeof(origin));
            std::memcpy(ray.data(), counts.data() + 4 * (2 * width + i), sizeof(ray));
            std::fprintf(receipt, "reflection_ray_%u_%u=%.8g,%.8g,%.8g,%.8g;%.8g,%.8g,%.8g,%.8g\n",
                         entry, i, origin[0], origin[1], origin[2], origin[3], ray[0], ray[1], ray[2], ray[3]);
        }
    }
    if (compact) {
        for (UINT row = 0; row < 3; ++row) {
            auto *record = jobs_data.data() + 5 * row;
            if (row < 2 && entry == 432) {
                // This permutation reads the queued record without rewriting it.
                matched &= record[0] == row;
                for (UINT i = 1; i < 5; ++i)
                    matched &= record[i] == 0xabcd0000u + 5 * row + i;
            } else if (row < 2) {
                const bool hit = geometry && row == 0;
                matched &= record[0] == row && record[1] == (hit ? 0u : ~0u) &&
                           record[2] == (hit ? 0x3e333333u : 0x41880001u) &&
                           record[3] == ~0u && record[4] == ~0u;
            }
            std::fprintf(receipt, "reflection_record_%u%s_%u=%08x,%08x,%08x,%08x,%08x\n", entry, geometry ? "_near" : "", row,
                         record[0], record[1], record[2], record[3], record[4]);
        }
        matched &= std::all_of(jobs_data.begin() + 10, jobs_data.end(), [](UINT x) { return x == 0xdeadbeefu; });
    }
    for (UINT x = 0; x < width; ++x) {
        Pixel color{};
        float distance{};
        std::memcpy(&color, bytes + x * sizeof(color), sizeof(color));
        std::memcpy(&distance, bytes + 512 + x * sizeof(float), sizeof(float));
        const UINT invocation = x < 2 ? 1 - x : x;
        auto *callback = counts.data() + invocation * 4;
        const UINT face = callback[3];
        const bool surface = geometry && !masked && x == 1;
        const Pixel expect = x == 2 ? Pixel{-99, -99, -99, -99} : surface ? Pixel{} :
                             Pixel{float(face + 2), float(face + 3), float(face + 4), 1};
        matched &= std::equal(color.begin(), color.begin() + 3, expect.begin()) && callback[0] == (surface ? 1u : 0u) &&
                   callback[1] == (x == 2 || surface ? 0u : 1u);
        matched &= distance == (x == 2 ? -99.0f : surface ? -.175f : -17.0f);
        if (surface) matched &= callback[2] == 0x3e333333u;
        std::fprintf(receipt, "reflection_%u%s_%u=%.8g,%.8g,%.8g;%.8g;hits %u;misses %u;face %u\n", entry,
                     geometry ? "_near" : "", x, color[0], color[1], color[2], distance, callback[0], callback[1],
                     callback[3]);
    }
    readback->Unmap(0, &empty);
    if (!matched)
        throw std::runtime_error("unexpected reflection texture, distance or callbacks");
    return result;
}
