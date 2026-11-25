// Dispatch the two translated DXR ray generators against the completed test TLAS.
// The target layout and export names are the four audited global archive entries.
#include "ray_inspection_probe.h"

static ComPtr<ID3D12StateObject> link_guest_libraries(ID3D12Device* guest, ID3D12RootSignature* root) {
    const IID device8_iid{0x728f1fa2, 0xea11, 0x4a9f, {0x9b,0x52,0x9b,0x42,0xa4,0x84,0xd5,0xc9}};
    ComPtr<IUnknown> device8;
    check(guest->QueryInterface(device8_iid, reinterpret_cast<void**>(device8.GetAddressOf())), "Xbox Device8 query");
    using Deserialize = HRESULT(WINAPI*)(IUnknown*, const void*, UINT, ID3D12RootSignature*, UINT, REFIID, void**);
    const auto deserialize = method<Deserialize>(device8.Get(), 115);
    const char* files[]{"1280.sptr", "1286.sptr", "1282.sptr", "1284.sptr"};
    ComPtr<ID3D12StateObject> objects[4];
    D3D12_EXISTING_COLLECTION_DESC imports[4]{};
    D3D12_STATE_SUBOBJECT subs[5]{};
    for (UINT i = 0; i < ARRAYSIZE(files); ++i) {
        const auto raw = read_file(files[i]);
        check(deserialize(device8.Get(), raw.data(), static_cast<UINT>(raw.size()), root, 0,
                          IID_PPV_ARGS(&objects[i])), "deserialize native ray collection");
        const UINT functions[]{4, 4, 3, 3};
        const UINT vgprs[]{30, 35, 28, 34};
        check_ray_inspection(objects[i].Get(), 1, functions[i], vgprs[i]);
        imports[i].pExistingCollection = objects[i].Get();
        subs[i] = {D3D12_STATE_SUBOBJECT_TYPE_EXISTING_COLLECTION, &imports[i]};
    }
    D3D12_GLOBAL_ROOT_SIGNATURE global{root};
    subs[4] = {D3D12_STATE_SUBOBJECT_TYPE_GLOBAL_ROOT_SIGNATURE, &global};
    D3D12_STATE_OBJECT_DESC desc{D3D12_STATE_OBJECT_TYPE_RAYTRACING_PIPELINE, ARRAYSIZE(subs), subs};
    using Link = HRESULT(WINAPI*)(IUnknown*, const D3D12_STATE_OBJECT_DESC*, REFIID, void**);
    ComPtr<ID3D12StateObject> pipeline;
    check(method<Link>(guest, 101)(guest, &desc, IID_PPV_ARGS(&pipeline)), "Xbox CreateStateObject");
    check_ray_inspection(pipeline.Get(), 4, 14, 35);
    void* rejected = reinterpret_cast<void*>(UINT_PTR{1});
    const auto raw = read_file(files[0]);
    if (deserialize(device8.Get(), raw.data(), static_cast<UINT>(raw.size()), root, 1,
                    __uuidof(ID3D12StateObject), &rejected) != E_NOTIMPL || rejected)
        throw std::runtime_error("unsupported ray state flags accepted");
    return pipeline;
}

static void probe_ray_libraries(ID3D12Device5* owner, ID3D12CommandQueue* queue,
                                ID3D12Resource* scene, FILE* receipt, ID3D12Device* guest, bool indirect) {
    D3D12_ROOT_PARAMETER params[4]{};
    for (UINT i = 0; i < 4; ++i) {
        params[i].ParameterType = i < 2 ? D3D12_ROOT_PARAMETER_TYPE_SRV : D3D12_ROOT_PARAMETER_TYPE_UAV;
        params[i].Descriptor = {i % 2, 1};
    }
    D3D12_ROOT_SIGNATURE_DESC root_desc{4, params, 0, nullptr, D3D12_ROOT_SIGNATURE_FLAG_NONE};
    ComPtr<ID3DBlob> serialized;
    check(D3D12SerializeRootSignature(&root_desc, D3D_ROOT_SIGNATURE_VERSION_1, &serialized, nullptr), "serialize ray root");
    ComPtr<ID3D12RootSignature> root;
    auto* root_device = guest ? guest : static_cast<ID3D12Device*>(owner);
    check(root_device->CreateRootSignature(0, serialized->GetBufferPointer(), serialized->GetBufferSize(),
                                     IID_PPV_ARGS(&root)), "create ray root");

    const wchar_t* names[]{L"OcclusionMainRG_d7cd43b4", L"IntersectionMainRG_d7cd43b4",
                           L"DefaultPayloadMS_1b832205", L"DefaultMainCHS_eb1ce8e7"};
    const char* files[]{"1280.dxil", "1286.dxil", "1282.dxil", "1284.dxil"};
    const wchar_t* group = L"HitGroup_0dd42e58_eb1ce8e7";
    std::array<std::vector<char>, 4> code;
    D3D12_DXIL_LIBRARY_DESC libs[4]{};
    D3D12_STATE_SUBOBJECT subs[8]{};
    for (size_t i = 0; i < code.size(); ++i) {
        code[i] = read_file(files[i]);
        libs[i].DXILLibrary = {code[i].data(), code[i].size()};
        subs[i] = {D3D12_STATE_SUBOBJECT_TYPE_DXIL_LIBRARY, &libs[i]};
    }
    D3D12_HIT_GROUP_DESC hit_group{};
    hit_group.HitGroupExport = group;
    hit_group.ClosestHitShaderImport = names[3];
    D3D12_RAYTRACING_SHADER_CONFIG shader_cfg{24, 8};
    D3D12_RAYTRACING_PIPELINE_CONFIG pipeline_cfg{1};
    D3D12_GLOBAL_ROOT_SIGNATURE global{root.Get()};
    subs[4] = {D3D12_STATE_SUBOBJECT_TYPE_HIT_GROUP, &hit_group};
    subs[5] = {D3D12_STATE_SUBOBJECT_TYPE_RAYTRACING_SHADER_CONFIG, &shader_cfg};
    subs[6] = {D3D12_STATE_SUBOBJECT_TYPE_RAYTRACING_PIPELINE_CONFIG, &pipeline_cfg};
    subs[7] = {D3D12_STATE_SUBOBJECT_TYPE_GLOBAL_ROOT_SIGNATURE, &global};
    D3D12_STATE_OBJECT_DESC state_desc{D3D12_STATE_OBJECT_TYPE_RAYTRACING_PIPELINE, 8, subs};
    ComPtr<ID3D12StateObject> pipeline;
    if (guest) pipeline = link_guest_libraries(guest, root.Get());
    else check(owner->CreateStateObject(&state_desc, IID_PPV_ARGS(&pipeline)), "link ray libraries");
    ComPtr<ID3D12StateObjectProperties> properties;
    check(pipeline.As(&properties), "get ray shader identifiers");

    std::array<unsigned char, 320> table{};
    auto record = [&](size_t offset, const wchar_t* name) {
        const void* id = properties->GetShaderIdentifier(name);
        if (!id) throw std::runtime_error("missing ray dispatch export");
        std::memcpy(table.data() + offset, id, D3D12_SHADER_IDENTIFIER_SIZE_IN_BYTES);
    };
    record(0, names[0]);
    record(64, names[1]);
    record(128, names[2]);
    for (size_t i = 0; i < 3; ++i) record(192 + i * 32, group);
    const auto sbt = upload(owner, table.data(), table.size());
    const auto address = sbt->GetGPUVirtualAddress();

    struct Ray { float x, y, z, unused, dx, dy, dz, maximum; };
    const Ray inputs[]{{-1,0,-1,99,0,0,1,10}, {1,0,-1,99,0,0,1,10},
                       {3,0,-1,99,0,0,1,10}, {5.125f,0,-1,99,0,0,1,10},
                       {7,0,-1,99,0,0,1,10}, {1,0,-1,99,0,0,1,1}};
    const auto rays = upload(owner, inputs, sizeof(inputs));
    struct Hit { float t; UINT primitive, instance; float u, v; };
    static_assert(sizeof(Hit) == 20 && sizeof(Ray) == 32);
    constexpr UINT count = ARRAYSIZE(inputs);
    constexpr UINT sizes[]{count * sizeof(UINT), count * sizeof(Hit)};
    std::array<ComPtr<ID3D12Resource>, 2> output, copied;
    ComPtr<ID3D12CommandAllocator> alloc;
    check(owner->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&alloc)), "ray allocator");
    ComPtr<ID3D12GraphicsCommandList4> list;
    check(owner->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, alloc.Get(), nullptr, IID_PPV_ARGS(&list)), "ray list");
    ComPtr<ID3D12GraphicsCommandList> xbox_list;
    ComPtr<ID3D12CommandAllocator> xbox_allocator;
    ComPtr<ID3D12CommandQueue> xbox_queue;
    using BindRoot = void(WINAPI*)(IUnknown*, ID3D12RootSignature*);
    using BindState = void(WINAPI*)(IUnknown*, ID3D12StateObject*);
    using BindAddress = void(WINAPI*)(IUnknown*, UINT, D3D12_GPU_VIRTUAL_ADDRESS);
    if (guest) {
        D3D12_COMMAND_QUEUE_DESC qdesc{};
        check(guest->CreateCommandQueue(&qdesc, IID_PPV_ARGS(&xbox_queue)), "Xbox ray queue");
        check(guest->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&xbox_allocator)), "Xbox ray allocator");
        xbox_list = guest_list(guest, xbox_allocator.Get());
        auto* recorder = xbox_list.Get();
        method<BindRoot>(recorder, 33)(recorder, root.Get());
        method<BindState>(recorder, 120)(recorder, pipeline.Get());
        method<BindAddress>(recorder, 43)(recorder, 0, scene->GetGPUVirtualAddress());
        method<BindAddress>(recorder, 43)(recorder, 1, rays->GetGPUVirtualAddress());
    } else {
        list->SetComputeRootSignature(root.Get());
        list->SetPipelineState1(pipeline.Get());
        list->SetComputeRootShaderResourceView(0, scene->GetGPUVirtualAddress());
        list->SetComputeRootShaderResourceView(1, rays->GetGPUVirtualAddress());
    }
    for (UINT i = 0; i < 2; ++i) {
        output[i] = buffer(owner, sizes[i], D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, true);
        copied[i] = buffer(owner, sizes[i], D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
        if (guest)
            method<BindAddress>(xbox_list.Get(), 45)(xbox_list.Get(), 2 + i, output[i]->GetGPUVirtualAddress());
        else list->SetComputeRootUnorderedAccessView(2 + i, output[i]->GetGPUVirtualAddress());
    }
    ComPtr<ID3D12CommandSignature> signature;
    std::array<ComPtr<ID3D12Resource>, 2> argument_buffers;
    if (indirect) {
        D3D12_INDIRECT_ARGUMENT_DESC argument{};
        argument.Type = D3D12_INDIRECT_ARGUMENT_TYPE_DISPATCH_RAYS;
        D3D12_COMMAND_SIGNATURE_DESC signature_desc{sizeof(D3D12_DISPATCH_RAYS_DESC), 1, &argument, 1};
        check(root_device->CreateCommandSignature(&signature_desc, guest ? root.Get() : nullptr,
                                                   IID_PPV_ARGS(&signature)), "indirect ray signature");
    }
    for (UINT i = 0; i < 2; ++i) {
        D3D12_DISPATCH_RAYS_DESC dispatch{};
        dispatch.RayGenerationShaderRecord = {address + 64 * i, 32};
        dispatch.MissShaderTable = {address + 128, 32, 32};
        dispatch.HitGroupTable = {address + 192, 96, 32};
        dispatch.Width = count;
        dispatch.Height = dispatch.Depth = 1;
        if (indirect) {
            // Frosty passes a nonzero byte offset into its argument buffer.
            std::array<D3D12_DISPATCH_RAYS_DESC, 2> args{};
            args[1] = dispatch;
            argument_buffers[i] = upload(root_device, args.data(), sizeof(args));
            if (guest) {
                using Execute = void(WINAPI*)(IUnknown*, ID3D12CommandSignature*, UINT, ID3D12Resource*,
                                              UINT64, ID3D12Resource*, UINT64);
                method<Execute>(xbox_list.Get(), 59)(xbox_list.Get(), signature.Get(), 1, argument_buffers[i].Get(),
                                                     sizeof(dispatch), nullptr, 0);
            } else list->ExecuteIndirect(signature.Get(), 1, argument_buffers[i].Get(), sizeof(dispatch), nullptr, 0);
        } else if (guest) {
            using Dispatch = void(WINAPI*)(IUnknown*, const D3D12_DISPATCH_RAYS_DESC*);
            method<Dispatch>(xbox_list.Get(), 121)(xbox_list.Get(), &dispatch);
        } else list->DispatchRays(&dispatch);
    }
    if (guest) submit(guest, xbox_queue.Get(), xbox_list.Get());
    for (UINT i = 0; i < 2; ++i) {
        D3D12_RESOURCE_BARRIER transition{};
        transition.Type = D3D12_RESOURCE_BARRIER_TYPE_TRANSITION;
        transition.Transition = {output[i].Get(), 0, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE};
        list->ResourceBarrier(1, &transition);
        list->CopyBufferRegion(copied[i].Get(), 0, output[i].Get(), 0, sizes[i]);
    }
    submit(owner, queue, list.Get());
    const Hit expected[]{{1,0,0,.25f,.5f}, {2,0,0,.25f,.5f}, {1,0,1,.25f,.5f},
                         {2,0,1,.375f,.5f}, {-1,0,0,0,0}, {-1,0,0,0,0}};
    for (UINT i = 0; i < 2; ++i) {
        D3D12_RANGE range{0, sizes[i]}, empty{};
        void* data{};
        check(copied[i]->Map(0, &range, &data), "map translated ray outputs");
        bool valid = true;
        for (UINT j = 0; j < count; ++j) {
            if (i == 0) valid &= static_cast<const UINT*>(data)[j] == (expected[j].t < 0 ? 0u : ~0u);
            else valid &= std::memcmp(static_cast<const Hit*>(data) + j, expected + j, sizeof(Hit)) == 0;
        }
        copied[i]->Unmap(0, &empty);
        if (!valid) throw std::runtime_error(i ? "translated intersection readback differs" : "translated occlusion readback differs");
    }
    if (indirect) {
        std::fprintf(receipt, "indirect_ray_dispatches=2\nindirect_ray_hits_per_dispatch=4\nindirect_ray_misses_per_dispatch=2\n");
        return;
    }
    if (guest) std::fprintf(receipt, "guest_deserialized_collections=4\nguest_linked_pipelines=1\n");
    if (guest) std::fprintf(receipt, "linked_inspection_exports=4\nlinked_inspection_functions=14\nlinked_inspection_max_vgpr=35\n");
    std::fprintf(receipt, "ray_library_dispatches=2\nray_library_hits_per_dispatch=4\nray_library_misses_per_dispatch=2\n");
}
