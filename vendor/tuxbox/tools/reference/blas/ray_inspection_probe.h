// Exercise the Xbox queries Frosty makes immediately after deserializing a collection.
static void check_ray_inspection(ID3D12StateObject* object, UINT expected_exports,
                                 UINT expected_functions, UINT max_vgpr) {
    ComPtr<ID3D12StateObjectProperties> props;
    check(object->QueryInterface(IID_PPV_ARGS(&props)), "Xbox state properties");
    ComPtr<IUnknown> state_id, props_id;
    check(object->QueryInterface(IID_PPV_ARGS(&state_id)), "state identity");
    check(props->QueryInterface(IID_PPV_ARGS(&props_id)), "properties identity");
    if (state_id.Get() != props_id.Get()) throw std::runtime_error("split state identity");

    using Query = HRESULT(WINAPI*)(IUnknown*, UINT64, void*);
    struct Reply { ID3DBlob* blob; UINT64 reserved; } reply{};
    check(method<Query>(props.Get(), 7)(props.Get(), 1, &reply), "Xbox state export info");
    ComPtr<ID3DBlob> exports;
    exports.Attach(reply.blob);
    struct ExportRow { const wchar_t* name; unsigned char id[32]; UINT fields[6]; };
    struct Exports { const ExportRow* rows; UINT count; };
    auto* info = static_cast<const Exports*>(exports->GetBufferPointer());
    if (info->count != expected_exports) throw std::runtime_error("export count differs");
    for (UINT n = 0; n < info->count; ++n) {
        auto const& row = info->rows[n];
        const auto* id = props->GetShaderIdentifier(row.name);
        if (!id || std::memcmp(id, row.id, sizeof(row.id))) throw std::runtime_error("export identifier differs");
    }
    using Inspector = HRESULT(WINAPI*)(IUnknown*, IUnknown**);
    IUnknown* borrowed{};
    check(method<Inspector>(object, 8)(object, &borrowed), "Xbox shader inspector");
    using Symbols = HRESULT(WINAPI*)(IUnknown*, UINT, UINT, ID3DBlob**);
    ComPtr<ID3DBlob> blob;
    check(method<Symbols>(borrowed, 9)(borrowed, 1, 0, blob.GetAddressOf()), "Xbox function symbols");
    struct Function { const char* name; UINT data[3], registers; };
    struct Functions { const Function* rows; UINT64 gpu_address; UINT count; };
    auto* symbols = static_cast<const Functions*>(blob->GetBufferPointer());
    if (symbols->count != expected_functions) throw std::runtime_error("symbol count differs");
    UINT used = 0;
    for (UINT n = 0; n < symbols->count; ++n) {
        if (!symbols->rows[n].name || !*symbols->rows[n].name) throw std::runtime_error("nameless shader");
        auto vgpr = symbols->rows[n].registers & 511;
        if (vgpr > used) used = vgpr;
    }
    if (used != max_vgpr) throw std::runtime_error("VGPR count differs");
    if (method<Symbols>(borrowed, 9)(borrowed, 1, 0, nullptr) != E_POINTER)
        throw std::runtime_error("invalid symbol output accepted");
}
