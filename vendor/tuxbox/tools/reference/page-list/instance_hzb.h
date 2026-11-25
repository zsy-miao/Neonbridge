// The shadow instance culler samples an R32_FLOAT depth pyramid through s0.
struct InstanceDepth {
    ComPtr<ID3D12Resource> texture, staging;
    ComPtr<ID3D12DescriptorHeap> samplers;
    void bind(ID3D12Device *dev, ID3D12GraphicsCommandList *list, D3D12_CPU_DESCRIPTOR_HANDLE view,
              const std::vector<char> &data) {
        constexpr UINT side = 8, pitch = 256;
        if (data.size() != side*side*sizeof(float)) throw std::runtime_error("expected an 8x8 depth image");
        std::array<char, pitch*side> padded{};
        for (UINT y = 0; y < side; ++y)
            std::memcpy(padded.data() + pitch*y, data.data() + sizeof(float)*side*y, sizeof(float)*side);
        staging = buffer(dev, sizeof(padded), D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_STATE_GENERIC_READ);
        upload(staging.Get(), padded.data(), sizeof(padded));
        D3D12_HEAP_PROPERTIES local{};
        local.Type = D3D12_HEAP_TYPE_DEFAULT;
        local.CreationNodeMask = local.VisibleNodeMask = 1;
        D3D12_RESOURCE_DESC d{};
        d.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
        d.Width = d.Height = side;
        d.MipLevels = d.DepthOrArraySize = d.SampleDesc.Count = 1;
        d.Format = DXGI_FORMAT_R32_FLOAT;
        check(dev->CreateCommittedResource(&local, D3D12_HEAP_FLAG_NONE, &d, D3D12_RESOURCE_STATE_COPY_DEST, nullptr,
                                           IID_PPV_ARGS(&texture)), "VSM HZB texture");
        D3D12_TEXTURE_COPY_LOCATION dst{}, src{};
        dst.pResource = texture.Get();
        src.pResource = staging.Get();
        src.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
        src.PlacedFootprint.Footprint = {d.Format, side, side, 1, pitch};
        list->CopyTextureRegion(&dst, 0, 0, 0, &src, nullptr);
        transition(list, texture.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
        dev->CreateShaderResourceView(texture.Get(), nullptr, view);
        D3D12_DESCRIPTOR_HEAP_DESC sh{D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER, 16, D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, 0};
        check(dev->CreateDescriptorHeap(&sh, IID_PPV_ARGS(&samplers)), "VSM sampler heap");
        D3D12_SAMPLER_DESC s{};
        s.Filter = D3D12_FILTER_MIN_MAG_MIP_POINT;
        s.AddressU = s.AddressV = s.AddressW = D3D12_TEXTURE_ADDRESS_MODE_CLAMP;
        s.MaxAnisotropy = 1;
        s.ComparisonFunc = D3D12_COMPARISON_FUNC_ALWAYS;
        s.MaxLOD = D3D12_FLOAT32_MAX;
        dev->CreateSampler(&s, samplers->GetCPUDescriptorHandleForHeapStart());
    }
};
