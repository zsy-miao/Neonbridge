// Build a desktop DXR control for one translated ray generator. The hit and
// miss records count actual traversal callbacks for every dispatch coordinate.
struct ObservedRayPipeline {
    ComPtr<ID3D12RootSignature> root;
    ComPtr<ID3D12StateObject> pipeline;
    ComPtr<ID3D12Resource> sbt;

    ObservedRayPipeline(ID3D12Device5* dev, const D3D12_ROOT_SIGNATURE_DESC& root_desc,
                        UINT entry, const wchar_t* export_name, const char* observer = "occlusion-observer.dxil") {
        ComPtr<ID3DBlob> serialized;
        check(D3D12SerializeRootSignature(&root_desc, D3D_ROOT_SIGNATURE_VERSION_1, &serialized, nullptr), "serialize shadow root");
        check(dev->CreateRootSignature(0, serialized->GetBufferPointer(), serialized->GetBufferSize(), IID_PPV_ARGS(&root)), "shadow root");

        auto code = read_file((std::to_string(entry) + ".dxil").c_str());
        auto probe = read_file(observer);
        D3D12_DXIL_LIBRARY_DESC libs[2]{};
        libs[0].DXILLibrary = {code.data(), code.size()};
        libs[1].DXILLibrary = {probe.data(), probe.size()};
        D3D12_HIT_GROUP_DESC group{};
        group.HitGroupExport = L"ProbeGroup";
        group.ClosestHitShaderImport = L"ProbeHit";
        D3D12_RAYTRACING_SHADER_CONFIG limits{64, 8};
        D3D12_RAYTRACING_PIPELINE_CONFIG recursion{1};
        D3D12_GLOBAL_ROOT_SIGNATURE global{root.Get()};
        D3D12_STATE_SUBOBJECT subs[]{
            {D3D12_STATE_SUBOBJECT_TYPE_DXIL_LIBRARY, &libs[0]},
            {D3D12_STATE_SUBOBJECT_TYPE_DXIL_LIBRARY, &libs[1]},
            {D3D12_STATE_SUBOBJECT_TYPE_HIT_GROUP, &group},
            {D3D12_STATE_SUBOBJECT_TYPE_RAYTRACING_SHADER_CONFIG, &limits},
            {D3D12_STATE_SUBOBJECT_TYPE_RAYTRACING_PIPELINE_CONFIG, &recursion},
            {D3D12_STATE_SUBOBJECT_TYPE_GLOBAL_ROOT_SIGNATURE, &global}};
        D3D12_STATE_OBJECT_DESC desc{D3D12_STATE_OBJECT_TYPE_RAYTRACING_PIPELINE, ARRAYSIZE(subs), subs};
        check(dev->CreateStateObject(&desc, IID_PPV_ARGS(&pipeline)), "shadow ray pipeline");
        ComPtr<ID3D12StateObjectProperties> props;
        check(pipeline.As(&props), "shadow ray identifiers");
        std::array<unsigned char, 256> records{};
        auto record = [&](size_t offset, const wchar_t* name) {
            auto* id = props->GetShaderIdentifier(name);
            if (!id) throw std::runtime_error("missing shadow export");
            std::memcpy(records.data() + offset, id, D3D12_SHADER_IDENTIFIER_SIZE_IN_BYTES);
        };
        record(0, export_name);
        record(64, L"ProbeMiss");
        for (UINT i = 0; i < 4; ++i) record(128 + 32 * i, L"ProbeGroup");
        sbt = upload(dev, records.data(), records.size());
    }
};
