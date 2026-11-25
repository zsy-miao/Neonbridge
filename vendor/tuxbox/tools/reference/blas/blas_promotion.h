// Validate two different native placements of one guest address. The original
// two-triangle tree has unused trailing blocks so its native BLAS fits in-pool.
struct PromotionScene {
    ComPtr<ID3D12GraphicsCommandList> list;
    ComPtr<ID3D12Resource> readback, output;
    static constexpr UINT bytes = 3 * 4 * sizeof(UINT);

    PromotionScene(ID3D12Device* device, ID3D12CommandAllocator* allocator, const Entry& scene,
                   ID3D12RootSignature* root, ID3D12PipelineState* pipeline, ID3D12Resource* rays) {
        output = buffer(device, bytes, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, true);
        readback = buffer(device, bytes, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
        list = guest_list(device, allocator);
        guest_batch(list.Get(), 1, &scene);
        D3D12_RESOURCE_BARRIER ready{};
        ready.Type = D3D12_RESOURCE_BARRIER_TYPE_UAV;
        list->ResourceBarrier(1, &ready);
        method<void(WINAPI*)(IUnknown*, ID3D12RootSignature*)>(list.Get(), 33)(list.Get(), root);
        using RootAddress = void(WINAPI*)(IUnknown*, UINT, UINT64);
        method<RootAddress>(list.Get(), 43)(list.Get(), 0, scene.build.DestAccelerationStructureData);
        method<RootAddress>(list.Get(), 45)(list.Get(), 1, output->GetGPUVirtualAddress());
        method<RootAddress>(list.Get(), 43)(list.Get(), 2, rays->GetGPUVirtualAddress());
        list->SetPipelineState(pipeline);
        list->Dispatch(3, 1, 1);
        D3D12_RESOURCE_BARRIER transition{};
        transition.Transition = {output.Get(), 0, D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                                 D3D12_RESOURCE_STATE_COPY_SOURCE};
        list->ResourceBarrier(1, &transition);
        list->CopyBufferRegion(readback.Get(), 0, output.Get(), 0, bytes);
        std::swap(transition.Transition.StateBefore, transition.Transition.StateAfter);
        list->ResourceBarrier(1, &transition);
        list->ResourceBarrier(1, &ready);
        check(list->Close(), "close promoted scene and ray queries");
    }

    void expect(UINT first) const {
        std::array<std::array<UINT, 4>, 3> hit{};
        D3D12_RANGE range{0, bytes}, empty{};
        void* data{};
        check(readback->Map(0, &range, &data), "read promoted scene rays");
        std::memcpy(hit.data(), data, bytes);
        readback->Unmap(0, &empty);
        if (hit[0] != std::array<UINT, 4>{11, 0, first, 0x3f800000} ||
            hit[1] != std::array<UINT, 4>{11, 0, first ^ 1u, 0x3f800000} ||
            hit[2] != std::array<UINT, 4>{~0u, ~0u, ~0u, 0}) {
            std::fprintf(stderr, "expected first primitive %u, got %u/%u/%u/%08x and %u/%u/%u/%08x\n", first, hit[0][0],
                         hit[0][1], hit[0][2], hit[0][3], hit[1][0], hit[1][1], hit[1][2], hit[1][3]);
            throw std::runtime_error("GPU rays differ after BLAS promotion");
        }
    }
};

static void check_blas_promotion(ID3D12Device* device, ID3D12CommandQueue* queue, ID3D12CommandAllocator* allocator,
                                 ID3D12Resource* destination, const Entry& scene, ID3D12Resource* records, UINT offset,
                                 ID3D12Resource* instances, UINT bytes, FILE* receipt) {
    auto init = guest_list(device, allocator);
    init->CopyBufferRegion(records, offset, instances, 0, bytes);
    D3D12_RESOURCE_BARRIER readable{};
    readable.Transition = {records, 0, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE};
    init->ResourceBarrier(1, &readable);
    submit(device, queue, init.Get());

    const auto quad = packed_quad();
    std::vector<UINT> padded(quad.begin(), quad.end());
    constexpr UINT direct_bytes = 2048;
    padded.resize((direct_bytes + 64) / sizeof(UINT));
    padded[8] = direct_bytes + 64;
    padded[10] = direct_bytes;
    padded[17] = direct_bytes / 64 - 1;
    const UINT64 address = destination->GetGPUVirtualAddress();
    CookedWrite direct(device, allocator, address, padded);
    CookedWrite relocated(device, allocator, address, expanded_quads());

    auto root = root_signature(device);
    D3D12_COMPUTE_PIPELINE_STATE_DESC desc{};
    desc.pRootSignature = root.Get();
    desc.CS = {query_dxil, sizeof(query_dxil)};
    ComPtr<ID3D12PipelineState> pipeline;
    using Compute =
        HRESULT(WINAPI*)(ID3D12Device*, const D3D12_COMPUTE_PIPELINE_STATE_DESC*, UINT, const void*, REFIID, void**);
    check(method<Compute>(device, 62)(device, &desc, 0, nullptr, IID_PPV_ARGS(&pipeline)), "promotion query PSO");
    struct Ray {
        float origin[3], minimum, direction[3], maximum;
        UINT flags;
    };
    const Ray probes[] = {{{10.25f, .25f, -1}, 0, {0, 0, 1}, 10, 0},
                          {{10.75f, .75f, -1}, 0, {0, 0, 1}, 10, 0},
                          {{11.5f, .5f, -1}, 0, {0, 0, 1}, 10, 0}};
    auto rays = upload(device, probes, sizeof(probes));
    PromotionScene first(device, allocator, scene, root.Get(), pipeline.Get(), rays.Get());
    PromotionScene second(device, allocator, scene, root.Get(), pipeline.Get(), rays.Get());

    // Both submissions are pending behind the same fence. Snapshots from the
    // second submission must stay independent of the first. Swap consumers on
    // the second pass so both closed TLAS lists must pick up a different map.
    for (UINT pass = 0; pass < 2; ++pass) {
        auto& old_scene = pass ? second : first;
        auto& new_scene = pass ? first : second;
        ComPtr<ID3D12Fence> gate, done;
        check(device->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&gate)), "promotion gate");
        check(device->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&done)), "promotion completion fence");
        check(queue->Wait(gate.Get(), 1), "gate pending scene submissions");
        ID3D12CommandList* old_batch[] = {direct.list.Get(), old_scene.list.Get()};
        ID3D12CommandList* new_batch[] = {relocated.list.Get(), new_scene.list.Get()};
        queue->ExecuteCommandLists(2, old_batch);
        queue->ExecuteCommandLists(2, new_batch);
        const HRESULT submitted = queue->Signal(done.Get(), 1);
        check(gate->Signal(1), "release pending scene submissions");
        check(submitted, "signal both scene submissions");
        check(done->SetEventOnCompletion(1, nullptr), "wait for both promoted scenes");
        old_scene.expect(1);
        new_scene.expect(0);
    }
    // Reuse a private destination for two different in-pool sources. Put both
    // sources at nonzero offsets so accidentally copying the pool base is observable.
    const UINT64 source_offset = 256;
    auto source = buffer(device, direct_bytes + source_offset, D3D12_HEAP_TYPE_DEFAULT,
                         D3D12_RESOURCE_STATE_RAYTRACING_ACCELERATION_STRUCTURE, true);
    const UINT64 source_va = source->GetGPUVirtualAddress() + source_offset;
    CookedWrite cooked_source(device, allocator, source_va, padded);
    submit_closed(device, queue, cooked_source.list.Get());

    auto copy = [&](UINT64 src) {
        auto list = guest_list(device, allocator);
        using Copy = void(WINAPI*)(IUnknown*, UINT64, UINT64, D3D12_RAYTRACING_ACCELERATION_STRUCTURE_COPY_MODE);
        method<Copy>(list.Get(), 119)(list.Get(), address, src, D3D12_RAYTRACING_ACCELERATION_STRUCTURE_COPY_MODE_COMPACT);
        D3D12_RESOURCE_BARRIER ready{};
        ready.Type = D3D12_RESOURCE_BARRIER_TYPE_UAV;
        list->ResourceBarrier(1, &ready);
        check(list->Close(), "close direct to relocated COMPACT");
        submit_closed(device, queue, list.Get());
    };
    copy(source_va);
    submit_closed(device, queue, first.list.Get());
    first.expect(1);

    const float vertices[][3] = {{0,0,0},{1,0,0},{0,1,0},{1,1,0},{0,1,0},{1,0,0}};
    auto mesh = upload(device, vertices, sizeof(vertices));
    D3D12_RAYTRACING_GEOMETRY_DESC triangles{};
    triangles.Type = D3D12_RAYTRACING_GEOMETRY_TYPE_TRIANGLES;
    triangles.Flags = D3D12_RAYTRACING_GEOMETRY_FLAG_OPAQUE;
    triangles.Triangles.VertexFormat = DXGI_FORMAT_R32G32B32_FLOAT;
    triangles.Triangles.VertexCount = ARRAYSIZE(vertices);
    triangles.Triangles.VertexBuffer = {mesh->GetGPUVirtualAddress(), sizeof(vertices[0])};
    Entry native{};
    native.build.Inputs.Type = D3D12_RAYTRACING_ACCELERATION_STRUCTURE_TYPE_BOTTOM_LEVEL;
    native.build.Inputs.Flags = D3D12_RAYTRACING_ACCELERATION_STRUCTURE_BUILD_FLAG_ALLOW_COMPACTION;
    native.build.Inputs.NumDescs = 1;
    native.build.Inputs.pGeometryDescs = &triangles;
    Sizes capacity{};
    using Prebuild = void(WINAPI*)(IUnknown*, const Inputs*, Sizes*);
    method<Prebuild>(device, 102)(device, &native.build.Inputs, &capacity);
    if (!capacity.ResultDataMaxSizeInBytes) throw std::runtime_error("empty native sizing");
    auto built = buffer(device, capacity.ResultDataMaxSizeInBytes + source_offset, D3D12_HEAP_TYPE_DEFAULT,
                        D3D12_RESOURCE_STATE_RAYTRACING_ACCELERATION_STRUCTURE, true);
    auto scratch = buffer(device, capacity.ScratchDataSizeInBytes, D3D12_HEAP_TYPE_DEFAULT,
                          D3D12_RESOURCE_STATE_UNORDERED_ACCESS, true);
    native.build.DestAccelerationStructureData = built->GetGPUVirtualAddress() + source_offset;
    native.build.ScratchAccelerationStructureData = scratch->GetGPUVirtualAddress();
    auto build = guest_list(device, allocator);
    guest_batch(build.Get(), 1, &native);
    D3D12_RESOURCE_BARRIER ready{};
    ready.Type = D3D12_RESOURCE_BARRIER_TYPE_UAV;
    build->ResourceBarrier(1, &ready);
    submit(device, queue, build.Get());
    copy(native.build.DestAccelerationStructureData);
    submit_closed(device, queue, first.list.Get());
    first.expect(0);
    submit_closed(device, queue, relocated.list.Get());
    std::fprintf(receipt, "direct_to_relocated_compact_rays=1,0,miss;0,1,miss\n");

    const auto direct_compacted = direct.compacted.read();
    const auto relocated_compacted = relocated.compacted.read();
    if (!direct_compacted || direct_compacted > direct_bytes || relocated_compacted <= 704)
        throw std::runtime_error("fixture failed to force direct to relocated BLAS");
    std::fprintf(receipt, "promotion_host_compacted=%llu,%llu\npromotion_first_primitive=1,0,1,0\n", direct_compacted,
                 relocated_compacted);
}
