#pragma once

struct InlineSceneFixture {
    ComPtr<ID3D12Device> dev = provider_device(true);
    ComPtr<ID3D12CommandQueue> queue;
    ComPtr<ID3D12Resource> scene;
    ComPtr<ID3D12RootSignature> root;
    ComPtr<ID3D12PipelineState> pipeline;

    InlineSceneFixture(const char* shader, UINT first_mask, UINT second_mask, bool both_geometries, bool reversed = false) {
        D3D12_COMMAND_QUEUE_DESC q{};
        check(dev->CreateCommandQueue(&q, IID_PPV_ARGS(&queue)), "Xbox queue");
        const auto original = read_file(shader);
        check(dev->CreateRootSignature(0, original.data(), original.size(), IID_PPV_ARGS(&root)), "Xbox root");
        D3D12_COMPUTE_PIPELINE_STATE_DESC pso{};
        pso.pRootSignature = root.Get();
        pso.CS = {original.data(), original.size()};
        using Create = HRESULT(WINAPI*)(ID3D12Device*, const D3D12_COMPUTE_PIPELINE_STATE_DESC*, UINT, const void*,
                                        REFIID, void**);
        check(method<Create>(dev.Get(), 62)(dev.Get(), &pso, 0, nullptr, IID_PPV_ARGS(&pipeline)),
              "Xbox slot 62");
        float vertices[][3] = {{-1.5f, -.5f, 0}, {-.5f, -.5f, 0}, {-1, .5f, 0},
                                     {.5f, -.5f, 1}, {1.5f, -.5f, 1}, {1, .5f, 1}};
        if (both_geometries)
            for (auto& v : vertices) std::swap(v[1], v[2]);
        auto verts = upload(dev.Get(), vertices, sizeof(vertices));
        D3D12_RAYTRACING_GEOMETRY_DESC geom{};
        geom.Type = D3D12_RAYTRACING_GEOMETRY_TYPE_TRIANGLES;
        geom.Flags = D3D12_RAYTRACING_GEOMETRY_FLAG_OPAQUE;
        geom.Triangles.VertexFormat = DXGI_FORMAT_R32G32B32_FLOAT;
        geom.Triangles.VertexCount = 3;
        geom.Triangles.VertexBuffer = {verts->GetGPUVirtualAddress(), sizeof(vertices[0])};
        ComPtr<ID3D12Resource> indices, transform;
        if (both_geometries) {
            // Exchange Y/Z at build time. Reading untransformed positions would
            // give this triangle a +/-Y normal and leave the -Z card unlit.
            const float yz[3][4]{{1,0,0,0}, {0,0,1,0}, {0,1,0,0}};
            // Record the opposite winding, then write the final indices after Close.
            const UINT16 order[]{0, UINT16(reversed ? 1 : 2), UINT16(reversed ? 2 : 1)};
            indices = upload(dev.Get(), order, sizeof(order));
            transform = upload(dev.Get(), yz, sizeof(yz));
            geom.Triangles.IndexFormat = DXGI_FORMAT_R16_UINT;
            geom.Triangles.IndexCount = 3;
            geom.Triangles.IndexBuffer = indices->GetGPUVirtualAddress();
            geom.Triangles.Transform3x4 = transform->GetGPUVirtualAddress();
        }
        auto build = [&](Inputs in) {
            Sizes sizes{};
            using Prebuild = void(WINAPI*)(ID3D12Device*, const Inputs*, Sizes*);
            method<Prebuild>(dev.Get(), 102)(dev.Get(), &in, &sizes);
            auto dest = buffer(dev.Get(), sizes.ResultDataMaxSizeInBytes, D3D12_HEAP_TYPE_DEFAULT,
                               D3D12_RESOURCE_STATE_RAYTRACING_ACCELERATION_STRUCTURE, true);
            auto scratch = buffer(dev.Get(), sizes.ScratchDataSizeInBytes, D3D12_HEAP_TYPE_DEFAULT,
                                  D3D12_RESOURCE_STATE_UNORDERED_ACCESS, true);
            ComPtr<ID3D12CommandAllocator> alloc;
            check(dev->CreateCommandAllocator(q.Type, IID_PPV_ARGS(&alloc)), "Xbox AS allocator");
            auto list = guest_list(dev.Get(), alloc.Get());
            ComPtr<ID3D12GraphicsCommandList4> list4;
            check(list.As(&list4), "Xbox List4");
            Build b{};
            b.Inputs = in;
            b.DestAccelerationStructureData = dest->GetGPUVirtualAddress();
            b.ScratchAccelerationStructureData = scratch->GetGPUVirtualAddress();
            using Single = void(WINAPI*)(ID3D12GraphicsCommandList4*, const Build*, UINT,
                                         const D3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_DESC*);
            method<Single>(list4.Get(), 117)(list4.Get(), &b, 0, nullptr);
            D3D12_RESOURCE_BARRIER barrier{};
            barrier.Type = D3D12_RESOURCE_BARRIER_TYPE_UAV;
            list->ResourceBarrier(1, &barrier);
            check(list->Close(), "close Xbox AS list");
            if (indices && in.Type == D3D12_RAYTRACING_ACCELERATION_STRUCTURE_TYPE_BOTTOM_LEVEL) {
                // Also replay one closed build after changing its upload. Both
                // the native BLAS and its published face normals must follow.
                if (reversed)
                    submit_closed(dev.Get(), queue.Get(), list.Get());
                const UINT16 order[]{0, UINT16(reversed ? 2 : 1), UINT16(reversed ? 1 : 2)};
                void* dst{};
                D3D12_RANGE no_read{}, written{0, sizeof(order)};
                check(indices->Map(0, &no_read, &dst), "map late indices");
                std::memcpy(dst, order, sizeof(order));
                indices->Unmap(0, &written);
            }
            submit_closed(dev.Get(), queue.Get(), list.Get());
            return dest;
        };
        Inputs bottom{};
        bottom.Type = D3D12_RAYTRACING_ACCELERATION_STRUCTURE_TYPE_BOTTOM_LEVEL;
        D3D12_RAYTRACING_GEOMETRY_DESC geometries[]{geom, geom};
        geometries[1].Triangles.VertexBuffer.StartAddress += 3 * sizeof(vertices[0]);
        bottom.NumDescs = both_geometries ? 2 : 1;
        bottom.pGeometryDescs = geometries;
        const auto blas = build(bottom);
        std::array<D3D12_RAYTRACING_INSTANCE_DESC, 2> instances{};
        for (UINT i = 0; i < instances.size(); ++i) {
            auto& inst = instances[i];
            inst.InstanceID = 7 + i;
            inst.InstanceMask = i == 0 ? first_mask : second_mask;
            for (UINT axis = 0; axis < 3; ++axis)
                inst.Transform[axis][axis] = 1.f;
            inst.Transform[0][3] = float(4 * i);
            inst.AccelerationStructure = blas->GetGPUVirtualAddress();
        }
        auto records = upload(dev.Get(), instances.data(), sizeof(instances));
        Inputs top{};
        top.Type = D3D12_RAYTRACING_ACCELERATION_STRUCTURE_TYPE_TOP_LEVEL;
        top.NumDescs = UINT(instances.size());
        top.InstanceDescs = records->GetGPUVirtualAddress();
        scene = build(top);
        bottom_ = blas;
    }

  private:
    ComPtr<ID3D12Resource> bottom_;
};

// Packaged compute root: 64 SRVs, 16 CBVs, a sampler table, then UAVs.
struct InlineComputeTables {
    ID3D12Device* dev;
    ID3D12DescriptorHeap* heap;
    ComPtr<ID3D12DescriptorHeap> samplers;
    UINT step;
    using Gpu = UINT64(WINAPI*)(ID3D12DescriptorHeap*);
    InlineComputeTables(ID3D12Device* device, ID3D12DescriptorHeap* views, UINT count)
        : dev(device), heap(views),
          step(dev->GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV)) {
        D3D12_DESCRIPTOR_HEAP_DESC hd{D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER, 16, D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                                      0};
        check(dev->CreateDescriptorHeap(&hd, IID_PPV_ARGS(&samplers)), "Xbox sampler table");
        auto cpu = method<frame_abi::CpuHandle>(samplers.Get(), 9)(samplers.Get());
        D3D12_SAMPLER_DESC d{};
        d.Filter = D3D12_FILTER_MIN_MAG_MIP_POINT;
        d.AddressU = d.AddressV = d.AddressW = D3D12_TEXTURE_ADDRESS_MODE_CLAMP;
        d.MaxLOD = D3D12_FLOAT32_MAX;
        d.MaxAnisotropy = 1;
        d.ComparisonFunc = D3D12_COMPARISON_FUNC_ALWAYS;
        for (UINT i = 0; i < count; ++i) {
            dev->CreateSampler(&d, {cpu});
            cpu += dev->GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER);
        }
    }
    D3D12_CPU_DESCRIPTOR_HANDLE view(UINT slot) { return {method<frame_abi::CpuHandle>(heap, 9)(heap) + slot * step}; }
    void cbv(UINT slot, ID3D12Resource* buffer, UINT bytes) {
        D3D12_CONSTANT_BUFFER_VIEW_DESC d{buffer->GetGPUVirtualAddress(), bytes};
        dev->CreateConstantBufferView(&d, view(64 + slot));
    }
    void buffer_uav(UINT slot, ID3D12Resource* buffer, UINT stride, UINT bytes) {
        D3D12_UNORDERED_ACCESS_VIEW_DESC d{};
        d.ViewDimension = D3D12_UAV_DIMENSION_BUFFER;
        d.Buffer.StructureByteStride = stride;
        d.Buffer.NumElements = bytes / stride;
        dev->CreateUnorderedAccessView(buffer, nullptr, &d, view(80 + slot));
    }
    void dispatch(ID3D12GraphicsCommandList* list, InlineSceneFixture& fixture) {
        list->SetPipelineState(fixture.pipeline.Get());
        using Heaps = void(WINAPI*)(ID3D12GraphicsCommandList*, UINT, ID3D12DescriptorHeap* const*);
        using Root = void(WINAPI*)(ID3D12GraphicsCommandList*, ID3D12RootSignature*);
        using Table = void(WINAPI*)(ID3D12GraphicsCommandList*, UINT, D3D12_GPU_DESCRIPTOR_HANDLE);
        ID3D12DescriptorHeap* bound[]{heap, samplers.Get()};
        method<Heaps>(list, 32)(list, 2, bound);
        method<Root>(list, 33)(list, fixture.root.Get());
        auto table = method<Table>(list, 35);
        auto gpu = method<Gpu>(heap, 10)(heap);
        table(list, 0, {gpu});
        table(list, 1, {gpu + 64u * step});
        table(list, 2, {method<Gpu>(samplers.Get(), 10)(samplers.Get())});
        table(list, 3, {gpu + 80u * step});
        list->Dispatch(1, 1, 1);
    }
};
