// Procedural candidates are only a proposed broad phase for a future shadow
// scene. This checks API behavior at zero, not triangle correctness.
static void probe_zero_aabb(ID3D12Device5 *dev, ID3D12CommandQueue *queue, FILE *receipt) {
    constexpr UINT box_count = 3, box_bytes = box_count * sizeof(D3D12_RAYTRACING_AABB);
    auto aabb = buffer(dev, box_bytes, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, true);
    const auto node_words = read_file("sidecar-leaves.bin");
    const auto directory_words = read_file("sidecar-directory.bin");
    if (node_words.size() != 128 || directory_words.size() != 112)
        throw std::runtime_error("unexpected sidecar fixture layout");
    auto nodes_upload = upload(dev, node_words.data(), node_words.size());
    auto nodes = buffer(dev, node_words.size(), D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COPY_DEST);
    auto directory_upload = upload(dev, directory_words.data(), directory_words.size());
    auto directory = buffer(dev, directory_words.size(), D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COPY_DEST);
    D3D12_RAYTRACING_GEOMETRY_DESC geometry{};
    geometry.Type = D3D12_RAYTRACING_GEOMETRY_TYPE_PROCEDURAL_PRIMITIVE_AABBS;
    geometry.AABBs.AABBCount = box_count;
    geometry.AABBs.AABBs = {aabb->GetGPUVirtualAddress(), sizeof(D3D12_RAYTRACING_AABB)};
    Inputs bottom{};
    bottom.Type = D3D12_RAYTRACING_ACCELERATION_STRUCTURE_TYPE_BOTTOM_LEVEL;
    bottom.DescsLayout = D3D12_ELEMENTS_LAYOUT_ARRAY;
    bottom.NumDescs = 1;
    bottom.pGeometryDescs = &geometry;
    Sizes bs{};
    dev->GetRaytracingAccelerationStructurePrebuildInfo(&bottom, &bs);
    auto blas = buffer(dev, bs.ResultDataMaxSizeInBytes, D3D12_HEAP_TYPE_DEFAULT,
                       D3D12_RESOURCE_STATE_RAYTRACING_ACCELERATION_STRUCTURE, true);
    auto bscratch = buffer(dev, bs.ScratchDataSizeInBytes, D3D12_HEAP_TYPE_DEFAULT,
                           D3D12_RESOURCE_STATE_UNORDERED_ACCESS, true);
    std::array<D3D12_RAYTRACING_INSTANCE_DESC, 3> instances{};
    for (UINT i=0; i<3; ++i) {
        for (UINT axis=0; axis<3; ++axis) instances[i].Transform[axis][axis] = 1;
        instances[i].InstanceMask = 0xff;
        instances[i].InstanceID = 7+i;
        instances[i].AccelerationStructure = blas->GetGPUVirtualAddress();
    }
    instances[1].Transform[0][0] = 2;
    instances[1].Transform[0][3] = 4;
    instances[2].Transform[0][3] = 12;
    instances[2].InstanceMask = 0;
    auto instance_buffer = upload(dev, instances.data(), sizeof(instances));
    Inputs top{};
    top.Type = D3D12_RAYTRACING_ACCELERATION_STRUCTURE_TYPE_TOP_LEVEL;
    top.DescsLayout = D3D12_ELEMENTS_LAYOUT_ARRAY;
    top.NumDescs = static_cast<UINT>(instances.size());
    top.InstanceDescs = instance_buffer->GetGPUVirtualAddress();
    Sizes ts{};
    dev->GetRaytracingAccelerationStructurePrebuildInfo(&top, &ts);
    auto tlas = buffer(dev, ts.ResultDataMaxSizeInBytes, D3D12_HEAP_TYPE_DEFAULT,
                       D3D12_RESOURCE_STATE_RAYTRACING_ACCELERATION_STRUCTURE, true);
    auto tscratch = buffer(dev, ts.ScratchDataSizeInBytes, D3D12_HEAP_TYPE_DEFAULT,
                           D3D12_RESOURCE_STATE_UNORDERED_ACCESS, true);
    D3D12_ROOT_PARAMETER params[5]{};
    params[0].ParameterType = D3D12_ROOT_PARAMETER_TYPE_SRV;
    params[1].ParameterType = D3D12_ROOT_PARAMETER_TYPE_UAV;
    params[2].ParameterType = D3D12_ROOT_PARAMETER_TYPE_SRV;
    params[2].Descriptor.ShaderRegister = 1;
    params[3].ParameterType = D3D12_ROOT_PARAMETER_TYPE_SRV;
    params[3].Descriptor.ShaderRegister = 2;
    params[4].ParameterType = D3D12_ROOT_PARAMETER_TYPE_SRV;
    params[4].Descriptor.ShaderRegister = 3;
    D3D12_ROOT_SIGNATURE_DESC root_desc{5, params, 0, nullptr, D3D12_ROOT_SIGNATURE_FLAG_NONE};
    ComPtr<ID3DBlob> blob;
    check(D3D12SerializeRootSignature(&root_desc, D3D_ROOT_SIGNATURE_VERSION_1, &blob, nullptr), "zero AABB root serialize");
    ComPtr<ID3D12RootSignature> root;
    check(dev->CreateRootSignature(0, blob->GetBufferPointer(), blob->GetBufferSize(), IID_PPV_ARGS(&root)), "zero AABB root");
    D3D12_COMPUTE_PIPELINE_STATE_DESC desc{};
    desc.pRootSignature = root.Get();
    desc.CS = {zero_aabb_dxil, sizeof(zero_aabb_dxil)};
    ComPtr<ID3D12PipelineState> pipeline;
    check(dev->CreateComputePipelineState(&desc, IID_PPV_ARGS(&pipeline)), "zero AABB pipeline");
    ComPtr<ID3D12CommandAllocator> alloc;
    ComPtr<ID3D12GraphicsCommandList4> list;
    check(dev->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&alloc)), "zero AABB allocator");
    check(dev->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, alloc.Get(), pipeline.Get(), IID_PPV_ARGS(&list)), "zero AABB list");
    list->CopyBufferRegion(nodes.Get(), 0, nodes_upload.Get(), 0, node_words.size());
    list->CopyBufferRegion(directory.Get(), 0, directory_upload.Get(), 0, directory_words.size());
    D3D12_RESOURCE_BARRIER nodes_ready{};
    nodes_ready.Type = D3D12_RESOURCE_BARRIER_TYPE_TRANSITION;
    nodes_ready.Transition = {nodes.Get(), 0, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE};
    list->ResourceBarrier(1, &nodes_ready);
    nodes_ready.Transition.pResource = directory.Get();
    list->ResourceBarrier(1, &nodes_ready);
    D3D12_ROOT_PARAMETER bounds_params[5]{};
    bounds_params[0].ParameterType = D3D12_ROOT_PARAMETER_TYPE_SRV;
    bounds_params[1].ParameterType = D3D12_ROOT_PARAMETER_TYPE_UAV;
    bounds_params[2].ParameterType = D3D12_ROOT_PARAMETER_TYPE_UAV;
    bounds_params[2].Descriptor.ShaderRegister = 1;
    bounds_params[3].ParameterType = D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS;
    bounds_params[3].Constants = {0, 0, 4};
    bounds_params[4].ParameterType = D3D12_ROOT_PARAMETER_TYPE_SRV;
    bounds_params[4].Descriptor.ShaderRegister = 1;
    D3D12_ROOT_SIGNATURE_DESC bounds_root_desc{5, bounds_params, 0, nullptr, D3D12_ROOT_SIGNATURE_FLAG_NONE};
    ComPtr<ID3DBlob> bounds_blob;
    check(D3D12SerializeRootSignature(&bounds_root_desc, D3D_ROOT_SIGNATURE_VERSION_1, &bounds_blob, nullptr), "bounds root serialize");
    ComPtr<ID3D12RootSignature> bounds_root;
    check(dev->CreateRootSignature(0, bounds_blob->GetBufferPointer(), bounds_blob->GetBufferSize(), IID_PPV_ARGS(&bounds_root)), "bounds root");
    D3D12_COMPUTE_PIPELINE_STATE_DESC bounds_pso_desc{};
    bounds_pso_desc.pRootSignature = bounds_root.Get();
    bounds_pso_desc.CS = {sidecar_bounds_dxil, sizeof(sidecar_bounds_dxil)};
    ComPtr<ID3D12PipelineState> bounds_pso;
    check(dev->CreateComputePipelineState(&bounds_pso_desc, IID_PPV_ARGS(&bounds_pso)), "bounds pipeline");
    const UINT bounds_zero = 0;
    auto bounds_zero_upload = upload(dev, &bounds_zero, sizeof(bounds_zero));
    auto bounds_status = buffer(dev, 4, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COPY_DEST, true);
    auto bounds_readback = buffer(dev, 4, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
    list->CopyBufferRegion(bounds_status.Get(), 0, bounds_zero_upload.Get(), 0, 4);
    D3D12_RESOURCE_BARRIER bounds_status_ready{};
    bounds_status_ready.Type = D3D12_RESOURCE_BARRIER_TYPE_TRANSITION;
    bounds_status_ready.Transition = {bounds_status.Get(), 0, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS};
    list->ResourceBarrier(1, &bounds_status_ready);
    list->SetPipelineState(bounds_pso.Get());
    list->SetComputeRootSignature(bounds_root.Get());
    list->SetComputeRootShaderResourceView(0, nodes->GetGPUVirtualAddress());
    list->SetComputeRootUnorderedAccessView(1, aabb->GetGPUVirtualAddress());
    list->SetComputeRootUnorderedAccessView(2, bounds_status->GetGPUVirtualAddress());
    const UINT bounds_sizes[]{static_cast<UINT>(node_words.size()), box_bytes, box_count, static_cast<UINT>(directory_words.size())};
    list->SetComputeRoot32BitConstants(3, 4, bounds_sizes, 0);
    list->SetComputeRootShaderResourceView(4, directory->GetGPUVirtualAddress());
    list->Dispatch(1, 1, 1);
    // Exercise the converter separately with a malformed selector. Its
    // output is deliberately never used as an AS build input.
    auto invalid_node_words = node_words;
    std::fill(invalid_node_words.begin() + 60, invalid_node_words.begin() + 64, 0);
    auto invalid_nodes = upload(dev, invalid_node_words.data(), invalid_node_words.size());
    std::array<UINT, box_bytes / sizeof(UINT)> sentinel{};
    sentinel.fill(0xdeadbeef);
    auto sentinel_upload = upload(dev, sentinel.data(), sizeof(sentinel));
    auto invalid_bounds = buffer(dev, box_bytes, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COPY_DEST, true);
    auto invalid_status = buffer(dev, 4, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COPY_DEST, true);
    auto invalid_readback = buffer(dev, box_bytes + 4, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
    list->CopyBufferRegion(invalid_bounds.Get(), 0, sentinel_upload.Get(), 0, box_bytes);
    list->CopyBufferRegion(invalid_status.Get(), 0, bounds_zero_upload.Get(), 0, 4);
    D3D12_RESOURCE_BARRIER invalid_ready[2]{};
    for (auto &item : invalid_ready) item.Type = D3D12_RESOURCE_BARRIER_TYPE_TRANSITION;
    invalid_ready[0].Transition = {invalid_bounds.Get(), 0, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS};
    invalid_ready[1].Transition = {invalid_status.Get(), 0, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS};
    list->ResourceBarrier(2, invalid_ready);
    list->SetComputeRootShaderResourceView(0, invalid_nodes->GetGPUVirtualAddress());
    list->SetComputeRootUnorderedAccessView(1, invalid_bounds->GetGPUVirtualAddress());
    list->SetComputeRootUnorderedAccessView(2, invalid_status->GetGPUVirtualAddress());
    list->Dispatch(1, 1, 1);
    D3D12_RESOURCE_BARRIER invalid_copy[2]{};
    for (auto &item : invalid_copy) item.Type = D3D12_RESOURCE_BARRIER_TYPE_TRANSITION;
    invalid_copy[0].Transition = {invalid_bounds.Get(), 0, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE};
    invalid_copy[1].Transition = {invalid_status.Get(), 0, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE};
    list->ResourceBarrier(2, invalid_copy);
    list->CopyBufferRegion(invalid_readback.Get(), 0, invalid_bounds.Get(), 0, box_bytes);
    list->CopyBufferRegion(invalid_readback.Get(), box_bytes, invalid_status.Get(), 0, 4);
    auto short_readback = buffer(dev, box_bytes + 4, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
    D3D12_RESOURCE_BARRIER short_reset[2]{};
    for (auto &item : short_reset) item.Type = D3D12_RESOURCE_BARRIER_TYPE_TRANSITION;
    short_reset[0].Transition = {invalid_bounds.Get(), 0, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_COPY_DEST};
    short_reset[1].Transition = {invalid_status.Get(), 0, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_COPY_DEST};
    list->ResourceBarrier(2, short_reset);
    list->CopyBufferRegion(invalid_bounds.Get(), 0, sentinel_upload.Get(), 0, box_bytes);
    list->CopyBufferRegion(invalid_status.Get(), 0, bounds_zero_upload.Get(), 0, 4);
    list->ResourceBarrier(2, invalid_ready);
    list->SetComputeRootShaderResourceView(0, nodes->GetGPUVirtualAddress());
    const UINT short_sizes[]{63, box_bytes, box_count, static_cast<UINT>(directory_words.size())};
    list->SetComputeRoot32BitConstants(3, 4, short_sizes, 0);
    list->Dispatch(1, 1, 1);
    list->ResourceBarrier(2, invalid_copy);
    list->CopyBufferRegion(short_readback.Get(), 0, invalid_bounds.Get(), 0, box_bytes);
    list->CopyBufferRegion(short_readback.Get(), box_bytes, invalid_status.Get(), 0, 4);
    auto type_readback = buffer(dev, box_bytes + 4, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
    auto invalid_directory_words = directory_words;
    const UINT unsupported_type = 4;
    std::memcpy(invalid_directory_words.data() + 16 + 3 * 16 + 8, &unsupported_type, 4);
    auto invalid_directory = upload(dev, invalid_directory_words.data(), invalid_directory_words.size());
    list->ResourceBarrier(2, short_reset);
    list->CopyBufferRegion(invalid_bounds.Get(), 0, sentinel_upload.Get(), 0, box_bytes);
    list->CopyBufferRegion(invalid_status.Get(), 0, bounds_zero_upload.Get(), 0, 4);
    list->ResourceBarrier(2, invalid_ready);
    list->SetComputeRoot32BitConstants(3, 4, bounds_sizes, 0);
    list->SetComputeRootShaderResourceView(4, invalid_directory->GetGPUVirtualAddress());
    list->Dispatch(1, 1, 1);
    list->ResourceBarrier(2, invalid_copy);
    list->CopyBufferRegion(type_readback.Get(), 0, invalid_bounds.Get(), 0, box_bytes);
    list->CopyBufferRegion(type_readback.Get(), box_bytes, invalid_status.Get(), 0, 4);
    // Optionally use one original user-local paired sphere leaf. Its bytes
    // stay in the temporary fixture directory and are never copied to evidence.
    ComPtr<ID3D12Resource> retail_bounds, retail_status, retail_readback;
    std::array<ComPtr<ID3D12Resource>, 2> retail_uploads{};
    std::vector<char> retail_leaf;
    {
        std::ifstream available("sidecar-retail-leaf.bin", std::ios::binary);
        if (available) {
            retail_leaf = read_file("sidecar-retail-leaf.bin");
            const auto retail_directory_words = read_file("sidecar-retail-directory.bin");
            if (retail_leaf.size() != 64 || retail_directory_words.size() != 64)
                throw std::runtime_error("unexpected retail pair fixture");
            auto retail_nodes = upload(dev, retail_leaf.data(), retail_leaf.size());
            auto retail_directory = upload(dev, retail_directory_words.data(), retail_directory_words.size());
            retail_bounds = buffer(dev, 48, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, true);
            retail_status = buffer(dev, 4, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COPY_DEST, true);
            retail_readback = buffer(dev, 52, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
            // Retain upload resources until both recorded executions complete.
            // They are carried below by explicit local references.
            retail_uploads = {retail_nodes, retail_directory};
            list->CopyBufferRegion(retail_status.Get(), 0, bounds_zero_upload.Get(), 0, 4);
            bounds_status_ready.Transition.pResource = retail_status.Get();
            list->ResourceBarrier(1, &bounds_status_ready);
            list->SetComputeRootShaderResourceView(0, retail_nodes->GetGPUVirtualAddress());
            list->SetComputeRootUnorderedAccessView(1, retail_bounds->GetGPUVirtualAddress());
            list->SetComputeRootUnorderedAccessView(2, retail_status->GetGPUVirtualAddress());
            const UINT retail_sizes[]{64, 48, 2, 64};
            list->SetComputeRoot32BitConstants(3, 4, retail_sizes, 0);
            list->SetComputeRootShaderResourceView(4, retail_directory->GetGPUVirtualAddress());
            list->Dispatch(1, 1, 1);
            D3D12_RESOURCE_BARRIER retail_copy[2]{};
            for (auto &item : retail_copy) item.Type = D3D12_RESOURCE_BARRIER_TYPE_TRANSITION;
            retail_copy[0].Transition = {retail_bounds.Get(), 0, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE};
            retail_copy[1].Transition = {retail_status.Get(), 0, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE};
            list->ResourceBarrier(2, retail_copy);
            list->CopyBufferRegion(retail_readback.Get(), 0, retail_bounds.Get(), 0, 48);
            list->CopyBufferRegion(retail_readback.Get(), 48, retail_status.Get(), 0, 4);
        }
    }
    D3D12_RESOURCE_BARRIER bounds_ready{};
    bounds_ready.Type = D3D12_RESOURCE_BARRIER_TYPE_TRANSITION;
    bounds_ready.Transition = {aabb.Get(), 0, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE};
    list->ResourceBarrier(1, &bounds_ready);
    Build build{};
    build.Inputs = bottom;
    build.DestAccelerationStructureData = blas->GetGPUVirtualAddress();
    build.ScratchAccelerationStructureData = bscratch->GetGPUVirtualAddress();
    list->BuildRaytracingAccelerationStructure(&build, 0, nullptr);
    D3D12_RESOURCE_BARRIER barrier{};
    barrier.Type = D3D12_RESOURCE_BARRIER_TYPE_UAV;
    barrier.UAV.pResource = blas.Get();
    list->ResourceBarrier(1, &barrier);
    build.Inputs = top;
    build.DestAccelerationStructureData = tlas->GetGPUVirtualAddress();
    build.ScratchAccelerationStructureData = tscratch->GetGPUVirtualAddress();
    list->BuildRaytracingAccelerationStructure(&build, 0, nullptr);
    barrier.UAV.pResource = tlas.Get();
    list->ResourceBarrier(1, &barrier);
    auto bounds_geometry_readback = buffer(dev, box_bytes, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
    D3D12_RESOURCE_BARRIER bounds_read{};
    bounds_read.Type = D3D12_RESOURCE_BARRIER_TYPE_TRANSITION;
    bounds_read.Transition = {aabb.Get(), 0, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_COPY_SOURCE};
    list->ResourceBarrier(1, &bounds_read);
    list->CopyBufferRegion(bounds_geometry_readback.Get(), 0, aabb.Get(), 0, box_bytes);
    bounds_read.Transition.StateBefore = D3D12_RESOURCE_STATE_COPY_SOURCE;
    bounds_read.Transition.StateAfter = D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE;
    list->ResourceBarrier(1, &bounds_read);
    const float origins[][4]{{.25f,.25f,0,0}, {2.25f,.25f,0,0}, {4.5f,.25f,0,0},
                              {0,.5f,0,0}, {.5f,0,0,0}, {.75f,.75f,0,0}, {8.5f,.25f,0,0},
                              {20,0,0,0}, {.25f,.25f,-1,0}, {12.25f,.25f,0,0}};
    constexpr UINT result_bytes = sizeof(origins);
    auto source = upload(dev, origins, sizeof(origins));
    auto output = buffer(dev, result_bytes, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, true);
    auto readback = buffer(dev, result_bytes, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
    list->SetPipelineState(pipeline.Get());
    list->SetComputeRootSignature(root.Get());
    list->SetComputeRootShaderResourceView(0, tlas->GetGPUVirtualAddress());
    list->SetComputeRootUnorderedAccessView(1, output->GetGPUVirtualAddress());
    list->SetComputeRootShaderResourceView(2, source->GetGPUVirtualAddress());
    list->SetComputeRootShaderResourceView(3, nodes->GetGPUVirtualAddress());
    list->SetComputeRootShaderResourceView(4, directory->GetGPUVirtualAddress());
    list->Dispatch(static_cast<UINT>(std::size(origins)), 1, 1);
    D3D12_ROOT_PARAMETER private_params[7]{};
    for (UINT i : {0u,1u,2u,5u}) {
        private_params[i].ParameterType = D3D12_ROOT_PARAMETER_TYPE_SRV;
        private_params[i].Descriptor.RegisterSpace = 127;
    }
    private_params[1].Descriptor.ShaderRegister = 1;
    private_params[2].Descriptor.ShaderRegister = 2;
    private_params[5].Descriptor.ShaderRegister = 3;
    for (UINT i : {3u,6u}) {
        private_params[i].ParameterType = D3D12_ROOT_PARAMETER_TYPE_UAV;
        private_params[i].Descriptor.RegisterSpace = 127;
    }
    private_params[6].Descriptor.ShaderRegister = 1;
    private_params[4].ParameterType = D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS;
    private_params[4].Constants = {0, 127, 2};
    D3D12_ROOT_SIGNATURE_DESC private_desc{7, private_params, 0, nullptr, D3D12_ROOT_SIGNATURE_FLAG_NONE};
    ComPtr<ID3DBlob> private_blob;
    check(D3D12SerializeRootSignature(&private_desc, D3D_ROOT_SIGNATURE_VERSION_1, &private_blob, nullptr), "inline candidate root serialize");
    ComPtr<ID3D12RootSignature> private_root;
    check(dev->CreateRootSignature(0, private_blob->GetBufferPointer(), private_blob->GetBufferSize(), IID_PPV_ARGS(&private_root)), "inline candidate root");
    D3D12_COMPUTE_PIPELINE_STATE_DESC private_pso{};
    private_pso.pRootSignature = private_root.Get();
    private_pso.CS = {inline219_dxil, sizeof(inline219_dxil)};
    ComPtr<ID3D12PipelineState> candidate_pipeline;
    check(dev->CreateComputePipelineState(&private_pso, IID_PPV_ARGS(&candidate_pipeline)), "inline candidate pipeline");
    auto inline_output = buffer(dev, result_bytes, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, true);
    auto inline_readback = buffer(dev, result_bytes + 4, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
    const UINT zero = 0;
    auto zero_upload = upload(dev, &zero, sizeof(zero));
    auto status = buffer(dev, 4, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COPY_DEST, true);
    list->CopyBufferRegion(status.Get(), 0, zero_upload.Get(), 0, 4);
    D3D12_RESOURCE_BARRIER status_ready{};
    status_ready.Type = D3D12_RESOURCE_BARRIER_TYPE_TRANSITION;
    status_ready.Transition = {status.Get(), 0, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS};
    list->ResourceBarrier(1, &status_ready);
    list->SetPipelineState(candidate_pipeline.Get());
    list->SetComputeRootSignature(private_root.Get());
    list->SetComputeRootShaderResourceView(0, tlas->GetGPUVirtualAddress());
    list->SetComputeRootShaderResourceView(1, nodes->GetGPUVirtualAddress());
    list->SetComputeRootShaderResourceView(2, directory->GetGPUVirtualAddress());
    list->SetComputeRootUnorderedAccessView(3, status->GetGPUVirtualAddress());
    const UINT extents[]{static_cast<UINT>(directory_words.size()), static_cast<UINT>(node_words.size())};
    list->SetComputeRoot32BitConstants(4, 2, extents, 0);
    list->SetComputeRootShaderResourceView(5, source->GetGPUVirtualAddress());
    list->SetComputeRootUnorderedAccessView(6, inline_output->GetGPUVirtualAddress());
    list->Dispatch(static_cast<UINT>(std::size(origins)), 1, 1);
    auto bad_output = buffer(dev, result_bytes, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, true);
    auto bad_readback = buffer(dev, result_bytes + 4, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
    auto bad_status = buffer(dev, 4, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COPY_DEST, true);
    list->CopyBufferRegion(bad_status.Get(), 0, zero_upload.Get(), 0, 4);
    status_ready.Transition.pResource = bad_status.Get();
    list->ResourceBarrier(1, &status_ready);
    list->SetComputeRootUnorderedAccessView(3, bad_status->GetGPUVirtualAddress());
    const UINT bad_extents[]{15, static_cast<UINT>(node_words.size())};
    list->SetComputeRoot32BitConstants(4, 2, bad_extents, 0);
    list->SetComputeRootUnorderedAccessView(6, bad_output->GetGPUVirtualAddress());
    list->Dispatch(static_cast<UINT>(std::size(origins)), 1, 1);
    D3D12_RESOURCE_BARRIER bad_barriers[2]{};
    for (auto &item : bad_barriers) item.Type = D3D12_RESOURCE_BARRIER_TYPE_TRANSITION;
    bad_barriers[0].Transition = {bad_output.Get(), 0, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE};
    bad_barriers[1].Transition = {bad_status.Get(), 0, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE};
    list->ResourceBarrier(2, bad_barriers);
    list->CopyBufferRegion(bad_readback.Get(), 0, bad_output.Get(), 0, result_bytes);
    list->CopyBufferRegion(bad_readback.Get(), result_bytes, bad_status.Get(), 0, 4);
    D3D12_RESOURCE_BARRIER inline_barriers[2]{};
    for (auto &item : inline_barriers) item.Type = D3D12_RESOURCE_BARRIER_TYPE_TRANSITION;
    inline_barriers[0].Transition = {inline_output.Get(), 0, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE};
    inline_barriers[1].Transition = {status.Get(), 0, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE};
    list->ResourceBarrier(2, inline_barriers);
    list->CopyBufferRegion(inline_readback.Get(), 0, inline_output.Get(), 0, result_bytes);
    list->CopyBufferRegion(inline_readback.Get(), result_bytes, status.Get(), 0, 4);
    D3D12_RESOURCE_BARRIER bounds_status_copy{};
    bounds_status_copy.Type = D3D12_RESOURCE_BARRIER_TYPE_TRANSITION;
    bounds_status_copy.Transition = {bounds_status.Get(), 0, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE};
    list->ResourceBarrier(1, &bounds_status_copy);
    list->CopyBufferRegion(bounds_readback.Get(), 0, bounds_status.Get(), 0, 4);
    barrier.Type = D3D12_RESOURCE_BARRIER_TYPE_TRANSITION;
    barrier.Transition = {output.Get(), 0, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE};
    list->ResourceBarrier(1, &barrier);
    list->CopyBufferRegion(readback.Get(), 0, output.Get(), 0, result_bytes);
    // Restore every repeatedly written resource to its initial state so the
    // same closed list can be replayed after its prior execution completes.
    D3D12_RESOURCE_BARRIER replay[11]{};
    for (auto &item : replay) item.Type = D3D12_RESOURCE_BARRIER_TYPE_TRANSITION;
    replay[0].Transition = {nodes.Get(), 0, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_COPY_DEST};
    replay[1].Transition = {directory.Get(), 0, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_COPY_DEST};
    replay[2].Transition = {aabb.Get(), 0, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_UNORDERED_ACCESS};
    replay[3].Transition = {output.Get(), 0, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_UNORDERED_ACCESS};
    replay[4].Transition = {inline_output.Get(), 0, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_UNORDERED_ACCESS};
    replay[5].Transition = {bad_output.Get(), 0, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_UNORDERED_ACCESS};
    replay[6].Transition = {status.Get(), 0, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_COPY_DEST};
    replay[7].Transition = {bad_status.Get(), 0, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_COPY_DEST};
    replay[8].Transition = {bounds_status.Get(), 0, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_COPY_DEST};
    replay[9].Transition = {invalid_bounds.Get(), 0, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_COPY_DEST};
    replay[10].Transition = {invalid_status.Get(), 0, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_COPY_DEST};
    list->ResourceBarrier(static_cast<UINT>(std::size(replay)), replay);
    if (retail_bounds) {
        D3D12_RESOURCE_BARRIER retail_reset[2]{};
        for (auto &item : retail_reset) item.Type = D3D12_RESOURCE_BARRIER_TYPE_TRANSITION;
        retail_reset[0].Transition = {retail_bounds.Get(), 0, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_UNORDERED_ACCESS};
        retail_reset[1].Transition = {retail_status.Get(), 0, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_COPY_DEST};
        list->ResourceBarrier(2, retail_reset);
    }
    submit(dev, queue, list.Get());
    D3D12_RANGE read{0,result_bytes}, empty{};
    void *data{};
    D3D12_RANGE bounds_range{0,4};
    check(bounds_readback->Map(0, &bounds_range, &data), "bounds status readback");
    UINT bounds_result{};
    std::memcpy(&bounds_result, data, 4);
    bounds_readback->Unmap(0, &empty);
    std::fprintf(receipt, "sidecar_gpu_bounds_status=%u\n", bounds_result);
    if (bounds_result != 0) throw std::runtime_error("GPU bounds conversion failed");
    if (retail_readback) {
        D3D12_RANGE retail_range{0,52};
        check(retail_readback->Map(0, &retail_range, &data), "retail pair bounds readback");
        std::array<D3D12_RAYTRACING_AABB, 2> retail_boxes{};
        std::memcpy(retail_boxes.data(), data, 48);
        UINT retail_result{};
        std::memcpy(&retail_result, static_cast<const BYTE *>(data) + 48, 4);
        retail_readback->Unmap(0, &empty);
        float vertices[4][3]{};
        std::memcpy(vertices, retail_leaf.data(), sizeof(vertices));
        const UINT indices[2][3]{{0,1,2},{1,3,2}};
        bool enclosed = retail_result == 0;
        for (UINT side=0; side<2; ++side) {
            const auto &b = retail_boxes[side];
            const float lo[]{b.MinX,b.MinY,b.MinZ}, hi[]{b.MaxX,b.MaxY,b.MaxZ};
            for (UINT axis=0; axis<3; ++axis) for (UINT corner : indices[side])
                enclosed &= std::isfinite(lo[axis]) && std::isfinite(hi[axis]) &&
                            lo[axis] < vertices[corner][axis] && hi[axis] > vertices[corner][axis];
        }
        std::fprintf(receipt, "sidecar_retail_pair=status %u; enclosed %u\n", retail_result, enclosed);
        if (!enclosed) throw std::runtime_error("retail paired leaf bounds mismatch");
    }
    D3D12_RANGE geometry_range{0, box_bytes};
    check(bounds_geometry_readback->Map(0, &geometry_range, &data), "GPU bounds readback");
    std::array<D3D12_RAYTRACING_AABB, box_count> generated{};
    std::memcpy(generated.data(), data, box_bytes);
    bounds_geometry_readback->Unmap(0, &empty);
    const auto encloses = [](const D3D12_RAYTRACING_AABB &b, float minimum_x, float maximum_x, float maximum_y) {
        return std::isfinite(b.MinX) && std::isfinite(b.MinY) && std::isfinite(b.MinZ) &&
               std::isfinite(b.MaxX) && std::isfinite(b.MaxY) && std::isfinite(b.MaxZ) &&
               b.MinX < minimum_x && b.MaxX > maximum_x && b.MinY < 0 && b.MaxY > maximum_y &&
               b.MinZ < 0 && b.MaxZ > 0;
    };
    if (!encloses(generated[0], 0, 1, 1) || generated[0].MaxX >= 1.25f || generated[0].MaxY >= 1.5f ||
        !encloses(generated[1], 0, 1.25f, 1.5f) || !encloses(generated[2], 2, 3, 1))
        throw std::runtime_error("GPU bounds do not conservatively enclose the triangles");
    std::fprintf(receipt, "sidecar_gpu_bounds_enclose=3\n");
    D3D12_RANGE invalid_range{0, box_bytes + 4};
    check(invalid_readback->Map(0, &invalid_range, &data), "invalid bounds readback");
    std::array<UINT, box_bytes / sizeof(UINT) + 1> invalid_values{};
    std::memcpy(invalid_values.data(), data, box_bytes + 4);
    invalid_readback->Unmap(0, &empty);
    bool invalid_untouched = std::all_of(invalid_values.begin(), invalid_values.begin() + 6,
                                         [](UINT word) { return word == 0xdeadbeef; });
    std::fprintf(receipt, "sidecar_gpu_invalid=status %u; untouched %u\n", invalid_values.back(), invalid_untouched);
    if (invalid_values.back() != 2 || !invalid_untouched)
        throw std::runtime_error("invalid GPU bounds input was accepted");
    check(short_readback->Map(0, &invalid_range, &data), "short bounds readback");
    std::memcpy(invalid_values.data(), data, box_bytes + 4);
    short_readback->Unmap(0, &empty);
    const bool short_untouched = std::all_of(invalid_values.begin(), invalid_values.end() - 1,
                                             [](UINT word) { return word == 0xdeadbeef; });
    std::fprintf(receipt, "sidecar_gpu_short=status %u; untouched %u\n", invalid_values.back(), short_untouched);
    if (invalid_values.back() != 1 || !short_untouched)
        throw std::runtime_error("short GPU bounds input was accepted");
    check(type_readback->Map(0, &invalid_range, &data), "unsupported node type readback");
    std::memcpy(invalid_values.data(), data, box_bytes + 4);
    type_readback->Unmap(0, &empty);
    const bool type_untouched = std::all_of(invalid_values.begin(), invalid_values.begin() + 6,
                                            [](UINT word) { return word == 0xdeadbeef; });
    std::fprintf(receipt, "sidecar_gpu_type=status %u; untouched %u\n", invalid_values.back(), type_untouched);
    if (invalid_values.back() != 1 || !type_untouched)
        throw std::runtime_error("unsupported GPU sidecar node type was accepted");
    check(readback->Map(0, &read, &data), "zero AABB readback");
    std::array<std::array<UINT,4>,std::size(origins)> values{};
    std::memcpy(values.data(), data, result_bytes);
    readback->Unmap(0, &empty);
    D3D12_RANGE inline_range{0, result_bytes + 4};
    check(inline_readback->Map(0, &inline_range, &data), "inline candidate readback");
    std::array<std::array<UINT,4>,std::size(origins)> inline_values{};
    std::memcpy(inline_values.data(), data, result_bytes);
    UINT inline_status{};
    std::memcpy(&inline_status, static_cast<const BYTE *>(data) + result_bytes, 4);
    inline_readback->Unmap(0, &empty);
    std::fprintf(receipt, "inline219_candidate_status=%u\n", inline_status);
    for (UINT i=0; i<std::size(origins); ++i)
        std::fprintf(receipt, "inline219_candidate_%u=%u,%u,%08x,%08x\n", i, inline_values[i][0], inline_values[i][1], inline_values[i][2], inline_values[i][3]);
    bool inline_matches = true;
    for (UINT i=0; i<std::size(origins); ++i) {
        // Candidate enumeration order/count is implementation-dependent; a
        // committed t=0 hit can stop another overlapping AABB being visited.
        inline_matches &= inline_values[i][1] == values[i][1] && inline_values[i][3] == values[i][3];
        const UINT a = inline_values[i][2], b = values[i][2];
        inline_matches &= a == b || ((a & 0x7fffffffu) == 0 && (b & 0x7fffffffu) == 0);
        std::fprintf(receipt, "inline219_reference_%u=%u,%u,%08x,%08x\n", i, values[i][0], values[i][1], values[i][2], values[i][3]);
    }
    if (inline_status != 0 || !inline_matches)
        throw std::runtime_error("inline 219 guarded candidate mismatch");
    check(bad_readback->Map(0, &inline_range, &data), "invalid inline directory readback");
    std::array<std::array<UINT,4>,std::size(origins)> bad_values{};
    std::memcpy(bad_values.data(), data, result_bytes);
    UINT bad_bits{};
    std::memcpy(&bad_bits, static_cast<const BYTE *>(data) + result_bytes, 4);
    bad_readback->Unmap(0, &empty);
    std::fprintf(receipt, "inline219_invalid_directory_status=%u\n", bad_bits);
    if (bad_bits != 2 || std::any_of(bad_values.begin(), bad_values.end(),
        [](const auto &value) { return value[1] != 0 || value[2] != 0 || value[3] != 0; }))
        throw std::runtime_error("inline 219 invalid directory accepted");
    for (UINT i=0; i<std::size(origins); ++i)
        std::fprintf(receipt, "zero_aabb_%u=%u,%u,%08x,%08x\n", i, values[i][0], values[i][1], values[i][2], values[i][3]);
    if (values[0][0] < 1 || values[0][0] > 2 || values[0][1] != 2 || values[0][2] != 0 || values[0][3] != 0x70000 ||
        values[1] != std::array<UINT,4>{1,2,0,0x70002} ||
        values[2][0] < 1 || values[2][0] > 2 || values[2][1] != 2 || values[2][2] != 0 || values[2][3] != 0x80000 ||
        values[3][0] < 1 || values[3][0] > 2 || values[3][1] != 2 ||
        (values[3][2] & 0x7fffffffu) != 0 || values[3][3] != 0x70000 ||
        values[4] != std::array<UINT,4>{2,0,0,0} || values[5][0] < 1 || values[5][0] > 2 ||
        values[5][1] != 2 || (values[5][2] & 0x7fffffffu) != 0 || values[5][3] != 0x70001 ||
        values[6] != std::array<UINT,4>{1,2,0,0x80002} || values[7] != std::array<UINT,4>{0,0,0,0} ||
        values[8][0] < 1 || values[8][0] > 2 || values[8][1] != 2 ||
        values[8][2] != 0x3f800000 || values[8][3] != 0x70000 ||
        values[9] != std::array<UINT,4>{0,0,0,0})
        throw std::runtime_error("zero AABB candidate mismatch");
    // Change the recorded upload only after completion. The replay must copy
    // it, regenerate bounds, rebuild the AS and observe the new geometry.
    auto moved = node_words;
    for (UINT corner=0; corner<4; ++corner) {
        float x{};
        std::memcpy(&x, moved.data() + corner * 12, sizeof(x));
        x += 10.f;
        std::memcpy(moved.data() + corner * 12, &x, sizeof(x));
    }
    write_upload(nodes_upload.Get(), moved.data(), moved.size());
    submit_closed(dev, queue, list.Get());
    check(inline_readback->Map(0, &inline_range, &data), "replayed inline candidate readback");
    std::memcpy(inline_values.data(), data, result_bytes);
    std::memcpy(&inline_status, static_cast<const BYTE *>(data) + result_bytes, 4);
    inline_readback->Unmap(0, &empty);
    check(bounds_readback->Map(0, &bounds_range, &data), "replayed bounds status readback");
    std::memcpy(&bounds_result, data, 4);
    bounds_readback->Unmap(0, &empty);
    check(bounds_geometry_readback->Map(0, &geometry_range, &data), "replayed GPU bounds readback");
    std::memcpy(generated.data(), data, box_bytes);
    bounds_geometry_readback->Unmap(0, &empty);
    std::fprintf(receipt, "sidecar_gpu_replay=status %u; bounds %u; old %u; other %u,%08x; moved %u\n",
                 inline_status, bounds_result, inline_values[0][1], inline_values[1][1],
                 inline_values[1][3], inline_values[2][1]);
    if (inline_status != 0 || bounds_result != 0 || !encloses(generated[0], 10, 11, 1) || generated[0].MaxX >= 11.25f ||
        !encloses(generated[1], 10, 11.25f, 1.5f) || !encloses(generated[2], 2, 3, 1) || inline_values[0][1] != 0 ||
        inline_values[1][1] != 2 || inline_values[1][3] != 0x70002 || inline_values[2][1] != 0)
        throw std::runtime_error("sidecar GPU replay observed stale geometry");
}
