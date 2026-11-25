// The Xbox shader record has two geometry addresses, four geometry constants
// and four descriptor tables. Exercise that layout through a native DXR SBT.
static void probe_materials(ID3D12Device5 *dev, ID3D12CommandQueue *queue,
                            ID3D12Resource *scene, std::array<D3D12_RAYTRACING_INSTANCE_DESC, 2> instances,
                            FILE *receipt,
                            ID3D12Device *guest) {
  struct Case {
    const char *file;
    const wchar_t *name, *group;
    const char *views;
    const wchar_t *any_hit;
  };
  static const Case cases[]{
#include "material_inputs.h"
  };
  D3D12_ROOT_PARAMETER params[2]{};
  params[0].ParameterType = D3D12_ROOT_PARAMETER_TYPE_SRV;
  params[0].Descriptor = {0, 3};
  params[1].ParameterType = D3D12_ROOT_PARAMETER_TYPE_UAV;
  params[1].Descriptor = {0, 3};
  D3D12_ROOT_SIGNATURE_DESC rs{2, params, 0, nullptr,
                               D3D12_ROOT_SIGNATURE_FLAG_NONE};
  ComPtr<ID3DBlob> serialized;
  check(D3D12SerializeRootSignature(&rs, D3D_ROOT_SIGNATURE_VERSION_1,
                                    &serialized, nullptr),
        "serialize material global root");
  ComPtr<ID3D12RootSignature> global;
  auto api = guest ? guest : static_cast<ID3D12Device *>(dev);
  check(api->CreateRootSignature(0, serialized->GetBufferPointer(),
                                 serialized->GetBufferSize(),
                                 IID_PPV_ARGS(&global)),
        "material global root");
  const UINT stride = dev->GetDescriptorHandleIncrementSize(
      D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV);
  D3D12_DESCRIPTOR_HEAP_DESC desc{D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 112,
                                  D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, 0};
  ComPtr<ID3D12DescriptorHeap> heap, samplers;
  check(api->CreateDescriptorHeap(&desc, IID_PPV_ARGS(&heap)),
        "material descriptor heap");
  desc.Type = D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER;
  desc.NumDescriptors = 16;
  check(api->CreateDescriptorHeap(&desc, IID_PPV_ARGS(&samplers)),
        "material sampler heap");
  D3D12_SAMPLER_DESC sampler{};
  sampler.Filter = D3D12_FILTER_MIN_MAG_MIP_POINT;
  sampler.AddressU = sampler.AddressV = sampler.AddressW =
      D3D12_TEXTURE_ADDRESS_MODE_WRAP;
  sampler.MaxAnisotropy = 1;
  sampler.ComparisonFunc = D3D12_COMPARISON_FUNC_ALWAYS;
  sampler.MaxLOD = D3D12_FLOAT32_MAX;
  using Handle = UINT64(WINAPI *)(IUnknown *);
  auto cpu_start = [&](ID3D12DescriptorHeap *h) {
    return guest ? D3D12_CPU_DESCRIPTOR_HANDLE{method<Handle>(h, 9)(h)}
                 : h->GetCPUDescriptorHandleForHeapStart();
  };
  auto gpu_start = [&](ID3D12DescriptorHeap *h) {
    return guest ? D3D12_GPU_DESCRIPTOR_HANDLE{method<Handle>(h, 10)(h)}
                 : h->GetGPUDescriptorHandleForHeapStart();
  };
  auto cpu_sampler = cpu_start(samplers.Get());
  for (UINT i = 0; i < 16;
       ++i, cpu_sampler.ptr += dev->GetDescriptorHandleIncrementSize(desc.Type))
    dev->CreateSampler(&sampler, cpu_sampler);
  // Supply independent local geometry and packed tangent streams for the
  // triangle at x=-1.
  const std::array<UINT, 4> indices{0, 1, 2, 0};
  const float vertices[][3] = {
      {-1.5f, -0.5f, 0}, {-0.5f, -0.5f, 0}, {-1, 0.5f, 0}};
  const float tangents[][4] = {{1, 0, 0, 1}, {0, 0, 1, 1}, {1, 0, 0, 1},
                               {0, 0, 1, 1}, {1, 0, 0, 1}, {0, 0, 1, 1}};
  auto index = upload(dev, indices.data(), sizeof(indices)),
       vertex = upload(dev, vertices, sizeof(vertices)),
       tangent = upload(dev, tangents, sizeof(tangents));
  std::vector<UINT> zeros(16384);
  auto scene_data = upload(dev, zeros.data(), zeros.size() * sizeof(UINT));
  // Native material 3559 reads b2[1].xyz for color, and b2[2].x for its
  // material parameter. Use finite binary fractions whose half-precision
  // outputs can be checked exactly.
  std::array<float, 64> constants{};
  constants[4] = 0.25f;
  constants[5] = 0.5f;
  constants[6] = 0.75f;
  constants[8] = 0.5f;
  auto material_cb = upload(dev, constants.data(), sizeof(constants));
  struct Record {
    BYTE id[32];
    UINT64 indices, vertices;
    UINT constants[4];
    UINT64 srvs, cbvs, samplers, uavs;
  };
  static_assert(sizeof(Record) == 96 && offsetof(Record, srvs) == 64);
  std::array<ComPtr<ID3D12Resource>, 3> textures, uploads;
  ComPtr<ID3D12CommandAllocator> texture_allocator;
  ComPtr<ID3D12GraphicsCommandList4> setup;
  check(dev->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT,
                                    IID_PPV_ARGS(&texture_allocator)),
        "material setup allocator");
  check(dev->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT,
                               texture_allocator.Get(), nullptr,
                               IID_PPV_ARGS(&setup)),
        "material setup list");
  D3D12_HEAP_PROPERTIES local{};
  local.Type = D3D12_HEAP_TYPE_DEFAULT;
  local.CreationNodeMask = local.VisibleNodeMask = 1;
  for (UINT type = 0; type < textures.size(); ++type) {
    D3D12_RESOURCE_DESC tex{};
    tex.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
    tex.Format =
        type ? DXGI_FORMAT_R32G32B32A32_UINT : DXGI_FORMAT_R32G32B32A32_FLOAT;
    tex.Width = tex.Height = tex.DepthOrArraySize = tex.SampleDesc.Count =
        tex.MipLevels = 1;
    check(dev->CreateCommittedResource(&local, D3D12_HEAP_FLAG_NONE, &tex,
                                       D3D12_RESOURCE_STATE_COPY_DEST, nullptr,
                                       IID_PPV_ARGS(&textures[type])),
          "material texture");
    std::array<BYTE, 256> source{};
    const float color[4] = {.25f, .5f, 1, 1};
    if (!type)
      std::memcpy(source.data(), color, sizeof(color));
    else if (type == 2) {
      // A page entry above 15 enables the sampled local material texture.
      const UINT pages[]{0x10u, 0x10u, 0x10u, 0x10u};
      std::memcpy(source.data(), pages, sizeof(pages));
    }
    uploads[type] = upload(dev, source.data(), source.size());
    D3D12_TEXTURE_COPY_LOCATION from{}, to{};
    from.pResource = uploads[type].Get();
    from.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
    from.PlacedFootprint.Footprint = {tex.Format, 1, 1, 1, 256};
    to.pResource = textures[type].Get();
    setup->CopyTextureRegion(&to, 0, 0, 0, &from, nullptr);
    D3D12_RESOURCE_BARRIER b{};
    b.Transition = {textures[type].Get(), 0, D3D12_RESOURCE_STATE_COPY_DEST,
                    D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE};
    setup->ResourceBarrier(1, &b);
  }
  ComPtr<ID3D12Resource> tilted, scratch, instance_data;
  if (std::any_of(std::begin(cases), std::end(cases),
                  [](const Case &item) { return std::strcmp(item.file, "1238") == 0; })) {
    // Leave the first instance unchanged. Shear and scale the second around
    // its ray intersection. Its object-space -Z normal becomes (.5,.25,-1).
    instances[1].Transform[1][1] = 2;
    instances[1].Transform[2][0] = .5f;
    instances[1].Transform[2][1] = .5f;
    instances[1].Transform[2][3] = .5f;
    instance_data = upload(dev, instances.data(), sizeof(instances));
    D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_DESC top{};
    top.Inputs.Type = D3D12_RAYTRACING_ACCELERATION_STRUCTURE_TYPE_TOP_LEVEL;
    top.Inputs.NumDescs = static_cast<UINT>(instances.size());
    top.Inputs.InstanceDescs = instance_data->GetGPUVirtualAddress();
    D3D12_RAYTRACING_ACCELERATION_STRUCTURE_PREBUILD_INFO size{};
    dev->GetRaytracingAccelerationStructurePrebuildInfo(&top.Inputs, &size);
    tilted = buffer(dev, size.ResultDataMaxSizeInBytes, D3D12_HEAP_TYPE_DEFAULT,
                    D3D12_RESOURCE_STATE_RAYTRACING_ACCELERATION_STRUCTURE, true);
    scratch = buffer(dev, size.ScratchDataSizeInBytes, D3D12_HEAP_TYPE_DEFAULT,
                     D3D12_RESOURCE_STATE_UNORDERED_ACCESS, true);
    top.DestAccelerationStructureData = tilted->GetGPUVirtualAddress();
    top.ScratchAccelerationStructureData = scratch->GetGPUVirtualAddress();
    setup->BuildRaytracingAccelerationStructure(&top, 0, nullptr);
    D3D12_RESOURCE_BARRIER ready{};
    ready.Type = D3D12_RESOURCE_BARRIER_TYPE_UAV;
    ready.UAV.pResource = tilted.Get();
    setup->ResourceBarrier(1, &ready);
  }
  submit(dev, queue, setup.Get());
  auto observer = read_file("material-observer.dxil");
  const auto gpu = gpu_start(heap.Get());
  auto cpu = cpu_start(heap.Get());
  D3D12_CONSTANT_BUFFER_VIEW_DESC cv{scene_data->GetGPUVirtualAddress(), 65536};
  for (UINT i = 0; i < 16; ++i)
    dev->CreateConstantBufferView(&cv, {cpu.ptr + (64 + i) * stride});
  D3D12_CONSTANT_BUFFER_VIEW_DESC material_view{
      material_cb->GetGPUVirtualAddress(), 256};

  D3D12_UNORDERED_ACCESS_VIEW_DESC uv{};
  uv.ViewDimension = D3D12_UAV_DIMENSION_BUFFER;
  uv.Format = DXGI_FORMAT_R32_UINT;
  uv.Buffer.NumElements = 1;
  for (UINT i = 0; i < 16; ++i)
    dev->CreateUnorderedAccessView(nullptr, nullptr, &uv,
                                   {cpu.ptr + (80 + i) * stride});
  for (const auto &item : cases) {
    auto code = read_file((std::string(item.file) + ".dxil").c_str());
    const bool lumen = std::strcmp(item.file, "1238") == 0;
    const bool material2036 = std::strcmp(item.file, "material-2036") == 0;
    const bool opacity_floor = std::strcmp(item.file, "material-31834") == 0;
    const bool local_opacity = opacity_floor || std::strcmp(item.file, "material-5384") == 0;
    const bool paged_material = std::strcmp(item.file, "material-18964") == 0;
    // This shader multiplies the sampled green channel by b2[8].rgb and b2[17].x.
    ComPtr<ID3D12Resource> bound_cb;
    auto bound_view = material_view;
    if (paged_material) {
      std::array<float, 128> page_constants{};
      page_constants[8 * 4] = .25f;
      page_constants[8 * 4 + 1] = .5f;
      page_constants[8 * 4 + 2] = 1.f;
      page_constants[17 * 4] = 1.f;
      bound_cb = upload(dev, page_constants.data(), sizeof(page_constants));
      bound_view = {bound_cb->GetGPUVirtualAddress(), sizeof(page_constants)};
    }
    if (local_opacity) {
      auto geometry_constants = constants;
      geometry_constants[5 * 4 + 3] = 1.f;
      bound_cb = upload(dev, geometry_constants.data(), sizeof(geometry_constants));
      bound_view = {bound_cb->GetGPUVirtualAddress(), sizeof(geometry_constants)};
    }
    dev->CreateConstantBufferView(&bound_view, {cpu.ptr + 66 * stride});
    std::array<ComPtr<ID3D12Resource>, 2> opacity_buffers;
    if (local_opacity) {
      // The second hit-group record changes one material constant. Both share
      // the instance, triangles, scene data and remaining descriptor contents.
      for (UINT slot = 0; slot < 16; ++slot) {
        auto address = D3D12_CPU_DESCRIPTOR_HANDLE{cpu.ptr + (96 + slot) * stride};
        dev->CreateConstantBufferView(slot == 2 ? &bound_view : &cv, address);
      }
      for (UINT variant = 0; variant < 2; ++variant) {
        std::array<std::array<float, 4>, 128> cb{};
        if (opacity_floor) {
          // Start at the low end of the smoothstep. The final max reads a
          // floor from b3[28].x and rejects only values strictly below 0.33.
          cb[26][0] = cb[27][2] = cb[32][0] = 1.f;
          cb[27][0] = cb[27][1] = 32.f;
          cb[28][0] = variant ? 0.33f : std::nextafter(0.33f, 0.f);
        } else {
          // Keep the reciprocal inputs finite in the procedural noise calculation.
          cb[58][1] = cb[59][3] = cb[63][1] = cb[65][3] = 1.f;
          cb[68][1] = cb[69][2] = cb[71][2] = 1.f;
          cb[80][2] = 16384.f;
          cb[60][1] = 1.f;
          cb[94][3] = variant ? -0.66f : -0.67f;
        }
        opacity_buffers[variant] = upload(dev, cb.data(), sizeof(cb));
        D3D12_CONSTANT_BUFFER_VIEW_DESC opacity_view{opacity_buffers[variant]->GetGPUVirtualAddress(), sizeof(cb)};
        const UINT base = variant ? 96 : 64;
        dev->CreateConstantBufferView(&opacity_view, {cpu.ptr + (base + 3) * stride});
      }
    }
    const UINT64 scene_address = (lumen ? tilted.Get() : scene)->GetGPUVirtualAddress();
    const auto miss = lumen ? read_file("1237.dxil") : decltype(code){};
    D3D12_DXIL_LIBRARY_DESC libs[3]{};
    libs[2].DXILLibrary = {miss.data(), miss.size()};
    libs[0].DXILLibrary = {observer.data(), observer.size()};
    libs[1].DXILLibrary = {code.data(), code.size()};
    D3D12_HIT_GROUP_DESC group{};
    group.HitGroupExport = item.group;
    group.ClosestHitShaderImport = item.name;
    group.AnyHitShaderImport = item.any_hit;
    D3D12_RAYTRACING_SHADER_CONFIG config{64, 8};
    D3D12_RAYTRACING_PIPELINE_CONFIG recursion{1};
    D3D12_GLOBAL_ROOT_SIGNATURE root{global.Get()};
    D3D12_STATE_SUBOBJECT subs[]{
        {D3D12_STATE_SUBOBJECT_TYPE_DXIL_LIBRARY, &libs[0]},
        {D3D12_STATE_SUBOBJECT_TYPE_DXIL_LIBRARY, &libs[1]},
        {D3D12_STATE_SUBOBJECT_TYPE_HIT_GROUP, &group},
        {D3D12_STATE_SUBOBJECT_TYPE_RAYTRACING_SHADER_CONFIG, &config},
        {D3D12_STATE_SUBOBJECT_TYPE_RAYTRACING_PIPELINE_CONFIG, &recursion},
        {D3D12_STATE_SUBOBJECT_TYPE_GLOBAL_ROOT_SIGNATURE, &root},
        {D3D12_STATE_SUBOBJECT_TYPE_DXIL_LIBRARY, &libs[2]}};
    D3D12_STATE_OBJECT_DESC pipeline_desc{
        D3D12_STATE_OBJECT_TYPE_RAYTRACING_PIPELINE, lumen ? 7u : 6u, subs};
    ComPtr<ID3D12StateObject> pso;
    if (guest) {
      const IID device8_iid{0x728f1fa2,
                            0xea11,
                            0x4a9f,
                            {0x9b, 0x52, 0x9b, 0x42, 0xa4, 0x84, 0xd5, 0xc9}};
      ComPtr<IUnknown> device8;
      check(guest->QueryInterface(
                device8_iid, reinterpret_cast<void **>(device8.GetAddressOf())),
            "material Xbox Device8");
      auto raw = read_file((std::string(item.file) + ".sptr").c_str());
      ComPtr<ID3D12StateObject> collection;
      using Deserialize =
          HRESULT(WINAPI *)(IUnknown *, const void *, UINT,
                            ID3D12RootSignature *, UINT, REFIID, void **);
      check(method<Deserialize>(device8.Get(), 115)(
                device8.Get(), raw.data(), static_cast<UINT>(raw.size()),
                global.Get(), 0, IID_PPV_ARGS(&collection)),
            "deserialize native material hit state");
      D3D12_EXISTING_COLLECTION_DESC existing{collection.Get(), 0, nullptr};
      ComPtr<ID3D12StateObject> miss_collection;
      D3D12_EXISTING_COLLECTION_DESC miss_existing{};
      if (lumen) {
        auto raw_miss = read_file("1237.sptr");
        check(method<Deserialize>(device8.Get(), 115)(
                  device8.Get(), raw_miss.data(), static_cast<UINT>(raw_miss.size()),
                  global.Get(), 0, IID_PPV_ARGS(&miss_collection)),
              "deserialize native Lumen material miss state");
        miss_existing.pExistingCollection = miss_collection.Get();
      }
      D3D12_STATE_SUBOBJECT linked[]{
          subs[0],
          {D3D12_STATE_SUBOBJECT_TYPE_EXISTING_COLLECTION, &existing},
          subs[3],
          subs[4],
          subs[5],
          {D3D12_STATE_SUBOBJECT_TYPE_EXISTING_COLLECTION, &miss_existing}};
      D3D12_STATE_OBJECT_DESC linked_desc{
          D3D12_STATE_OBJECT_TYPE_RAYTRACING_PIPELINE, lumen ? 6u : 5u,
          linked};
      using Link = HRESULT(WINAPI *)(
          IUnknown *, const D3D12_STATE_OBJECT_DESC *, REFIID, void **);
      check(method<Link>(guest, 101)(guest, &linked_desc, IID_PPV_ARGS(&pso)),
            "link native material and observer");
    } else {
      check(dev->CreateStateObject(&pipeline_desc, IID_PPV_ARGS(&pso)),
            "material hit pipeline");
    }
    ComPtr<ID3D12StateObjectProperties> props;
    check(pso.As(&props), "material hit exports");
    std::array<BYTE, 128 + 2 * sizeof(Record)> table{};
    auto copy_id = [&](BYTE *out, const wchar_t *name) {
      auto *id = props->GetShaderIdentifier(name);
      if (!id)
        throw std::runtime_error("missing material shader identifier");
      std::memcpy(out, id, D3D12_SHADER_IDENTIFIER_SIZE_IN_BYTES);
    };
    copy_id(table.data(), local_opacity ? L"LocalOpacityProbeRay" : item.any_hit ? L"MaskedMaterialProbeRay" : L"MaterialProbeRay");
    copy_id(table.data() + 64, lumen ? L"LumenHardwareRayTracingMaterialMS_3b1ba881" : L"MaterialProbeMiss");
    Record record{};
    copy_id(record.id, item.group);
    record.indices = index->GetGPUVirtualAddress();
    record.vertices = vertex->GetGPUVirtualAddress();
    record.constants[0] = 4u | (12u << 8);
    if (lumen) record.constants[2] = 0x12345678u;
    record.srvs = gpu.ptr;
    record.cbvs = gpu.ptr + 64 * stride;
    record.samplers = gpu_start(samplers.Get()).ptr;
    record.uavs = gpu.ptr + 80 * stride;
    std::memcpy(table.data() + 128, &record, sizeof(record));
    if (local_opacity) {
      record.cbvs = gpu.ptr + 96 * stride;
      std::memcpy(table.data() + 128 + sizeof(record), &record, sizeof(record));
    }
    auto sbt = upload(dev, table.data(), table.size());
    for (UINT slot = 0; slot < 64; ++slot) {
      D3D12_SHADER_RESOURCE_VIEW_DESC v{};
      v.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
      auto view = D3D12_CPU_DESCRIPTOR_HANDLE{cpu.ptr + slot * stride};
      const char kind = item.views[slot];
      if (kind == 'f' || kind == 'i') {
        dev->CreateShaderResourceView(textures[kind == 'i' ? (paged_material ? 2u : 1u) : 0u].Get(), nullptr,
                                      view);
      } else {
        v.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
        if (kind == 'b') {
          v.Format = DXGI_FORMAT_R32G32B32A32_FLOAT;
          v.Buffer.NumElements = ARRAYSIZE(tangents);
          dev->CreateShaderResourceView(tangent.Get(), &v, view);
        } else {
          v.Buffer.StructureByteStride = 16;
          v.Buffer.NumElements = 4096;
          dev->CreateShaderResourceView(scene_data.Get(), &v, view);
        }
      }
    }
    const UINT rays = item.any_hit ? 7u : 5u, bytes = rays * 16 * 4;
    auto output = buffer(dev, bytes, D3D12_HEAP_TYPE_DEFAULT,
                         D3D12_RESOURCE_STATE_UNORDERED_ACCESS, true);
    auto rb = buffer(dev, bytes, D3D12_HEAP_TYPE_READBACK,
                     D3D12_RESOURCE_STATE_COPY_DEST);
    ComPtr<ID3D12CommandAllocator> allocator;
    ComPtr<ID3D12GraphicsCommandList4> list;
    check(dev->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT,
                                      IID_PPV_ARGS(&allocator)),
          "material allocator");
    check(dev->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT,
                                 allocator.Get(), nullptr, IID_PPV_ARGS(&list)),
          "material list");
    ID3D12DescriptorHeap *heaps[]{heap.Get(), samplers.Get()};
    auto address = sbt->GetGPUVirtualAddress();
    D3D12_DISPATCH_RAYS_DESC dispatch{};
    dispatch.RayGenerationShaderRecord = {address, 32};
    dispatch.MissShaderTable = {address + 64, 32, 32};
    dispatch.HitGroupTable = {address + 128, (local_opacity ? 2u : 1u) * sizeof(Record), sizeof(Record)};
    dispatch.Width = rays;
    dispatch.Height = dispatch.Depth = 1;
    if (guest) {
      D3D12_COMMAND_QUEUE_DESC desc{};
      ComPtr<ID3D12CommandQueue> xbox_queue;
      ComPtr<ID3D12CommandAllocator> xbox_alloc;
      check(guest->CreateCommandQueue(&desc, IID_PPV_ARGS(&xbox_queue)),
            "material Xbox queue");
      check(guest->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT,
                                          IID_PPV_ARGS(&xbox_alloc)),
            "material Xbox allocator");
      auto xbox_list = guest_list(guest, xbox_alloc.Get());
      auto *rec = xbox_list.Get();
      using Heaps =
          void(WINAPI *)(IUnknown *, UINT, ID3D12DescriptorHeap *const *);
      using Root = void(WINAPI *)(IUnknown *, ID3D12RootSignature *);
      using State = void(WINAPI *)(IUnknown *, ID3D12StateObject *);
      using Address = void(WINAPI *)(IUnknown *, UINT, UINT64);
      using Dispatch =
          void(WINAPI *)(IUnknown *, const D3D12_DISPATCH_RAYS_DESC *);
      method<Heaps>(rec, 32)(rec, 2, heaps);
      method<Root>(rec, 33)(rec, global.Get());
      method<Address>(rec, 43)(rec, 0, scene_address);
      method<Address>(rec, 45)(rec, 1, output->GetGPUVirtualAddress());
      method<State>(rec, 120)(rec, pso.Get());
      method<Dispatch>(rec, 121)(rec, &dispatch);
      submit(guest, xbox_queue.Get(), rec);
    } else {
      list->SetDescriptorHeaps(2, heaps);
      list->SetComputeRootSignature(global.Get());
      list->SetComputeRootShaderResourceView(0, scene_address);
      list->SetComputeRootUnorderedAccessView(1,
                                              output->GetGPUVirtualAddress());
      list->SetPipelineState1(pso.Get());
      list->DispatchRays(&dispatch);
    }
    D3D12_RESOURCE_BARRIER b{};
    b.Transition = {output.Get(), 0, D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                    D3D12_RESOURCE_STATE_COPY_SOURCE};
    list->ResourceBarrier(1, &b);
    list->CopyBufferRegion(rb.Get(), 0, output.Get(), 0, bytes);
    submit(dev, queue, list.Get());
    D3D12_RANGE range{0, bytes}, empty{};
    void *data{};
    check(rb->Map(0, &range, &data), "material readback");
    auto *words = static_cast<const UINT *>(data);
    bool matched = true;
    for (UINT ray = 0; ray < rays; ++ray) {
      auto *hit = words + 16 * ray;
      std::fprintf(receipt, "hit_%s_%u=", item.file, ray);
      for (UINT i = 0; i < 16; ++i)
        std::fprintf(receipt, "%s%08x", i ? "," : "", hit[i]);
      std::fputc('\n', receipt);
      if (item.any_hit) {
        bool payload_mask = !local_opacity && (ray == 2 || ray == 3);
        bool ignored = ray < 4 && ((local_opacity && ray < 2) || std::strcmp(item.file, "material-520") == 0 ||
                                  (std::strcmp(item.file, "material-31") == 0 && payload_mask));
        if (ray == 4 || ignored) {
          for (UINT i = 0; i < 16; ++i)
            matched &= hit[i] == (i == 0 ? 0xbf800000u : i == 9 ? UINT(payload_mask) << 26u : 0x1000u + i);
        } else {
          matched &= hit[0] == 0x3f800000u;
        }
      } else if (ray == 4) {
        for (UINT i = 0; i < 16; ++i)
          matched &= hit[i] == (i == 0   ? 0xbf800000u
                                : i == 9 ? 1u << 25
                                         : 0x1000 + i);
      } else if (lumen) {
        matched &= hit[0] == 0x3f800000u && hit[1] == 0x12345678u;
        // Object-space -Z folds to (1,1) in the lower octahedron.
        // Under the shear its oct coordinates are (6/7,5/7). The shader
        // places X in bits 31:16 with v_cvt_u32_f32 dst_sel:WORD_1.
        matched &= hit[2] == (ray < 2 ? 0xffffffffu : 0xedb6db6du) && hit[3] == 7u + ray / 2u && hit[4] == ray / 2u;
        for (UINT i = 5; i < 16; ++i)
          matched &= hit[i] == (i == 9 ? (ray < 2 ? 0u : 1u << 25) : 0x1000u + i);
      } else {
        matched &= hit[0] == 0x3f800000u && hit[1] == 0x1001 && hit[2] == 0 &&
                   hit[3] == (paged_material ? 0x34003000u : 0u);
        matched &= (hit[9] & (1u << 23)) == ((ray & 1u) << 23);
        matched &= hit[12] == (ray < 2 ? (material2036 ? 0u : 0x1affu) : 1u) && hit[13] == 0 &&
                   hit[14] == 0 && hit[15] == 0;
        if (std::strcmp(item.file, "material-3559") == 0 ||
            std::strcmp(item.file, "material-28725") == 0)
          matched &= hit[6] == 0x38003400 && hit[7] == 0x3c003a00;
        if (material2036) {
          // This guest permutation emits literal half-NaN sentinels in the normal fields.
          matched &= hit[4] == 0xfe000000u && hit[5] == 0xfe00fe00u &&
                     hit[6] == 0x38003400u && hit[7] == 0x3c003c00u;
        }
        if (paged_material)
          matched &= hit[4] == 0x00003800u && hit[6] == 0x38003400u &&
                     hit[7] == 0x3c003c00u;
      }
    }
    rb->Unmap(0, &empty);
    if (local_opacity) dev->CreateConstantBufferView(&cv, {cpu.ptr + 67 * stride});
    if (!matched)
      throw std::runtime_error("unexpected material hit payload");
  }
}
