// The compacted work allocator gates traversal independently of the dispatch
// dimensions. Four callbacks have storage, while the queue contains one item.
#include "inline_scene_fixture.h"

struct ScreenGatherOutput {
  std::array<UINT, 2> depth{};
  std::array<UINT, 8> pixel{};
  std::array<UINT, 20> records{};
  ComPtr<ID3D12Resource> queue_records;
};

static ScreenGatherOutput probe_screen_gather(
    ID3D12Device5 *native, ID3D12CommandQueue *queue, ID3D12Resource *scene,
    FILE *receipt, UINT entry, bool active, bool geometry,
    InlineSceneFixture *compute = nullptr,
    const ScreenGatherOutput *reference = nullptr, UINT scenario = 0,
    const ScreenGatherOutput *prior = nullptr) {
  auto *dev =
      compute ? compute->dev.Get() : static_cast<ID3D12Device *>(native);
  if (compute) {
    queue = compute->queue.Get();
    scene = compute->scene.Get();
  }
  const bool far_field = entry == 64 || entry == 58;
  const UINT tlas = far_field ? 16u : 18u;
  constexpr UINT width = 4;
  D3D12_DESCRIPTOR_RANGE ranges[]{
      {D3D12_DESCRIPTOR_RANGE_TYPE_SRV, tlas + 1, 0, 1, 0},
      {D3D12_DESCRIPTOR_RANGE_TYPE_UAV, 2, 1, 1, 0}};
  D3D12_ROOT_PARAMETER params[9]{};
  for (UINT i = 0; i < 2; ++i) {
    params[i].ParameterType = D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE;
    params[i].DescriptorTable = {1, ranges + i};
  }
  for (UINT i = 0; i < 4; ++i) {
    params[2 + i].ParameterType = D3D12_ROOT_PARAMETER_TYPE_CBV;
    params[2 + i].Descriptor = {i, 1};
  }
  for (UINT i = 6; i < 9; ++i)
    params[i].ParameterType = D3D12_ROOT_PARAMETER_TYPE_UAV;
  params[6].Descriptor = {0, 1};
  params[7].Descriptor = {3, 1};
  params[8].Descriptor = {0, 2};
  D3D12_STATIC_SAMPLER_DESC s{};
  s.Filter = D3D12_FILTER_MIN_MAG_MIP_POINT;
  s.AddressU = s.AddressV = s.AddressW = D3D12_TEXTURE_ADDRESS_MODE_CLAMP;
  s.ComparisonFunc = D3D12_COMPARISON_FUNC_ALWAYS;
  s.MaxAnisotropy = 1;
  s.MaxLOD = D3D12_FLOAT32_MAX;
  s.RegisterSpace = 1;
  D3D12_ROOT_SIGNATURE_DESC rs{9, params, 1, &s,
                               D3D12_ROOT_SIGNATURE_FLAG_NONE};
  const auto *name =
      far_field ? L"LumenScreenProbeGatherHardwareRayTracingRGS_611ac55e"
                : L"LumenScreenProbeGatherHardwareRayTracingRGS_3d966f35";
  std::optional<ObservedRayPipeline> observed;
  if (!compute)
    observed.emplace(native, rs, entry, name, "screen-gather-observer.dxil");
  ComPtr<ID3D12CommandAllocator> allocator;
  ComPtr<ID3D12GraphicsCommandList4> list;
  check(dev->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT,
                                    IID_PPV_ARGS(&allocator)),
        "gather allocator");
  if (compute)
    check(guest_list(dev, allocator.Get()).As(&list), "Xbox screen list");
  else
    check(dev->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT,
                                 allocator.Get(), nullptr, IID_PPV_ARGS(&list)),
          "gather list");
  D3D12_DESCRIPTOR_HEAP_DESC hd{D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                                compute ? 96u : tlas + 3,
                                D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, 0};
  ComPtr<ID3D12DescriptorHeap> heap;
  check(dev->CreateDescriptorHeap(&hd, IID_PPV_ARGS(&heap)),
        "gather descriptors");
  UINT step = dev->GetDescriptorHandleIncrementSize(hd.Type);
  std::optional<InlineComputeTables> tables;
  if (compute)
    tables.emplace(dev, heap.Get(), 1);
  auto start =
      compute ? tables->view(0) : heap->GetCPUDescriptorHandleForHeapStart();
  auto cpu = start;
  std::array<ComPtr<ID3D12Resource>, 20> inputs{};
  for (UINT i = 0; i <= tlas + (compute ? 1u : 0u); ++i, cpu.ptr += step) {
    D3D12_SHADER_RESOURCE_VIEW_DESC d{};
    d.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
    if (i == tlas) {
      if (compute) {
        d.Format = DXGI_FORMAT_R32_TYPELESS;
        d.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
        d.Buffer.NumElements = UINT(scene->GetDesc().Width / 4);
        d.Buffer.Flags = D3D12_BUFFER_SRV_FLAG_RAW;
        inputs[i] = scene;
      } else {
        d.ViewDimension = D3D12_SRV_DIMENSION_RAYTRACING_ACCELERATION_STRUCTURE;
        d.RaytracingAccelerationStructure.Location =
            scene->GetGPUVirtualAddress();
      }
    } else if (i == tlas + 1 || i == 0 || i == 1 || i == 5 ||
               (!far_field && (i == 6 || i == 7)) || i == tlas - 2 ||
               i == tlas - 1 || i == tlas - 4 || i == 2 || i == 3) {
      d.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
      d.Buffer.NumElements = 1;
      if (i == 2 || i == 3) {
        d.Format = DXGI_FORMAT_R32_TYPELESS;
        d.Buffer.Flags = D3D12_BUFFER_SRV_FLAG_RAW;
      } else if (i == tlas - 1 || i == tlas - 4) {
        if (compute && i == tlas - 4)
          d.Buffer.StructureByteStride = 4;
        else
          d.Format = DXGI_FORMAT_R32_UINT;
        const UINT count = i == tlas - 1 && active ? 1u : 0u;
        inputs[i] = upload(dev, &count, sizeof(count));
      } else
        d.Buffer.StructureByteStride = i == tlas - 2 ? 8u : 16u;
    } else {
      d.ViewDimension = D3D12_SRV_DIMENSION_TEXTURE2D;
      d.Format = i == tlas - 3 ? DXGI_FORMAT_R32G32B32A32_UINT
                               : DXGI_FORMAT_R32G32B32A32_FLOAT;
      d.Texture2D.MipLevels = 1;
    }
    dev->CreateShaderResourceView(inputs[i].Get(), &d, cpu);
  }
  auto copy_texture = [&](const D3D12_TEXTURE_COPY_LOCATION &dst,
                          const D3D12_TEXTURE_COPY_LOCATION &src) {
    if (compute)
      method<frame_abi::Copy>(list.Get(), 83)(list.Get(), &dst, 0, 0, 0, &src,
                                              nullptr, 0);
    else
      list->CopyTextureRegion(&dst, 0, 0, 0, &src, nullptr);
  };
  auto transition = [&](ID3D12Resource *resource, D3D12_RESOURCE_STATES before,
                        D3D12_RESOURCE_STATES after) {
    D3D12_RESOURCE_BARRIER bar{};
    bar.Transition = {resource, 0, before, after};
    list->ResourceBarrier(1, &bar);
  };
  std::array<ComPtr<ID3D12Resource>, 2> textures;
  std::array<UINT, 64> untouched;
  untouched.fill(0xdeadbeefu);
  auto pixel_seed = upload(dev, untouched.data(), sizeof(untouched));
  if (compute)
    cpu.ptr = start.ptr + 81u * step;
  for (UINT i = 0; i < 2; ++i, cpu.ptr += step) {
    D3D12_UNORDERED_ACCESS_VIEW_DESC out{};
    out.ViewDimension = D3D12_UAV_DIMENSION_TEXTURE2D;
    out.Format = i == 0 ? DXGI_FORMAT_R32_UINT : DXGI_FORMAT_R32G32B32A32_FLOAT;
    D3D12_HEAP_PROPERTIES prop{};
    prop.Type = D3D12_HEAP_TYPE_DEFAULT;
    prop.CreationNodeMask = prop.VisibleNodeMask = 1;
    D3D12_RESOURCE_DESC tex{};
    tex.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
    tex.Width = 2;
    tex.Height = tex.DepthOrArraySize = tex.MipLevels = tex.SampleDesc.Count =
        1;
    tex.Format = out.Format;
    tex.Flags = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
    check(dev->CreateCommittedResource(&prop, D3D12_HEAP_FLAG_NONE, &tex,
                                       D3D12_RESOURCE_STATE_COPY_DEST, nullptr,
                                       IID_PPV_ARGS(&textures[i])),
          "gather output texture");
    D3D12_TEXTURE_COPY_LOCATION src{}, dst{};
    src.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
    src.pResource = pixel_seed.Get();
    src.PlacedFootprint.Footprint = {tex.Format, 2, 1, 1, 256};
    dst.pResource = textures[i].Get();
    copy_texture(dst, src);
    transition(textures[i].Get(), D3D12_RESOURCE_STATE_COPY_DEST,
               D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
    dev->CreateUnorderedAccessView(textures[i].Get(), nullptr, &out, cpu);
  }
  std::array<std::array<UINT, 1024>, 4> cb{};
  auto f = [&](UINT slot, UINT byte_offset, float n) {
    std::memcpy(&cb[slot][byte_offset / 4], &n, 4);
  };
  cb[0][46 * 4 + 2] = 256;
  cb[0][45 * 4] = width;
  cb[0][45 * 4 + 1] = 1;
  cb[0][29 * 4 + 3] = cb[0][30 * 4] = cb[0][34 * 4 + 1] = 1;
  f(0, 0x1ac, far_field ? .1f : 17.f);
  f(0, 0x290, 17);
  f(0, 0x2e0, far_field ? .01f : scenario == 4 ? .1f : 17.f);
  f(0, 0x2f0, 0.01f);
  f(0, 0x2f4, 17);
  f(1, 0x330, scenario == 6 ? 3.f : geometry ? -1.f : 20.f);
  f(1, 0x334, geometry ? 0.f : 20.f);
  f(1, 0x338, geometry ? .5f : 20.f);
  f(1, 0x33c, 1);
  f(1, 0x420, 1);
  f(1, 0x424, 1);
  f(1, 0xbe0, 1);
  f(1, 0xbe4, 1);
  f(1, 0xbe8, 1);
  f(2, 4, 1);
  f(3, 4, 1);
  std::array<ComPtr<ID3D12Resource>, 4> constants{};
  ID3D12DescriptorHeap *heaps[] = {heap.Get()};
  if (!compute) {
    list->SetDescriptorHeaps(1, heaps);
    list->SetComputeRootSignature(observed->root.Get());
    list->SetPipelineState1(observed->pipeline.Get());
    auto gpu = heap->GetGPUDescriptorHandleForHeapStart();
    list->SetComputeRootDescriptorTable(0, gpu);
    gpu.ptr += UINT64(tlas + 1) * step;
    list->SetComputeRootDescriptorTable(1, gpu);
  }
  for (UINT i = 0; i < 4; ++i) {
    constants[i] = upload(dev, cb[i].data(), sizeof(cb[i]));
    if (compute) {
      tables->cbv(i, constants[i].Get(), sizeof(cb[i]));
    } else
      list->SetComputeRootConstantBufferView(
          2 + i, constants[i]->GetGPUVirtualAddress());
  }
  std::array<UINT, width * 12> zeros{};
  auto seed = upload(dev, zeros.data(), sizeof(zeros));
  std::array<ComPtr<ID3D12Resource>, 3> storage;
  for (UINT i = 0; i < storage.size(); ++i) {
    if (i == 1 && prior) {
      storage[i] = prior->queue_records;
      transition(storage[i].Get(), D3D12_RESOURCE_STATE_COPY_SOURCE,
                 D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
    } else {
      storage[i] = buffer(dev, sizeof(zeros), D3D12_HEAP_TYPE_DEFAULT,
                          D3D12_RESOURCE_STATE_COPY_DEST, true);
      list->CopyBufferRegion(storage[i].Get(), 0, seed.Get(), 0, sizeof(zeros));
      transition(storage[i].Get(), D3D12_RESOURCE_STATE_COPY_DEST,
                 D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
    }
    if (compute && i < 2) {
      tables->buffer_uav(i * 3, storage[i].Get(), i == 0 ? 4u : 20u,
                         sizeof(zeros));
    } else if (!compute)
      list->SetComputeRootUnorderedAccessView(
          6 + i, storage[i]->GetGPUVirtualAddress());
  }
  if (compute) {
    tables->dispatch(list.Get(), *compute);
  } else {
    const auto base = observed->sbt->GetGPUVirtualAddress();
    D3D12_DISPATCH_RAYS_DESC dispatch{};
    dispatch.RayGenerationShaderRecord = {base, 32};
    dispatch.MissShaderTable = {base + 64, 32, 32};
    dispatch.HitGroupTable = {base + 128, 128, 32};
    dispatch.Width = width;
    dispatch.Height = dispatch.Depth = 1;
    list->DispatchRays(&dispatch);
  }
  auto readback = buffer(dev, 1536, D3D12_HEAP_TYPE_READBACK,
                         D3D12_RESOURCE_STATE_COPY_DEST);
  for (UINT i = 1; i < 3; ++i) {
    transition(storage[i].Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
               D3D12_RESOURCE_STATE_COPY_SOURCE);
    list->CopyBufferRegion(readback.Get(), (i - 1) * sizeof(zeros),
                           storage[i].Get(), 0, sizeof(zeros));
  }
  for (UINT i = 0; i < 2; ++i) {
    transition(textures[i].Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
               D3D12_RESOURCE_STATE_COPY_SOURCE);
    D3D12_TEXTURE_COPY_LOCATION src{}, dst{};
    src.pResource = textures[i].Get();
    dst.pResource = readback.Get();
    dst.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
    dst.PlacedFootprint = {512ull + i * 512ull,
                           {textures[i]->GetDesc().Format, 2, 1, 1, 256}};
    copy_texture(dst, src);
  }
  submit(dev, queue, list.Get());
  std::array<UINT, width * 12 * 2> result{};
  D3D12_RANGE range{0, 1536}, empty{};
  void *mem{};
  check(readback->Map(0, &range, &mem), "gather callback counts");
  std::memcpy(result.data(), mem, sizeof(result));
  std::array<UINT, 2> depth{};
  std::array<UINT, 8> pixel{};
  std::memcpy(depth.data(), static_cast<const BYTE *>(mem) + 512,
              sizeof(depth));
  std::memcpy(pixel.data(), static_cast<const BYTE *>(mem) + 1024,
              sizeof(pixel));
  readback->Unmap(0, &empty);
  std::fprintf(
      receipt,
      "screen_gather_pixels_%u_%u_%u=%08x,%08x;rgb %08x,%08x,%08x;guard %08x\n",
      entry, active, geometry, depth[0], depth[1], pixel[0], pixel[1], pixel[2],
      pixel[4]);
  ScreenGatherOutput out{depth, pixel, {}, storage[1]};
  std::copy_n(result.begin(), out.records.size(), out.records.begin());
  if (prior && out.records != prior->records)
    throw std::runtime_error(
        "far-field pass modified the input retrace records");
  if (compute) {
    const bool equal = reference && out.depth == reference->depth &&
                       out.pixel == reference->pixel &&
                       out.records == reference->records;
    std::fprintf(receipt,
                 "inline%u_%u=depth %08x,%08x; rgb %08x,%08x,%08x; record "
                 "%08x,%08x,%08x,%08x,%08x; match %u\n",
                 entry, scenario, depth[0], depth[1], pixel[0], pixel[1],
                 pixel[2], result[0], result[1], result[2], result[3],
                 result[4], equal);
    if (!equal)
      throw std::runtime_error(
          "screen gather differs from ray-generation control");
    return out;
  }
  if (depth[1] != 0xdeadbeefu ||
      !std::all_of(pixel.begin() + 4, pixel.end(),
                   [](UINT v) { return v == 0xdeadbeefu; }))
    throw std::runtime_error("screen probe modified adjacent pixel");
  if (scenario >= 3) {
    const bool hit =
        scenario == 4 || (scenario >= 5 && ((scenario == 5) != far_field));
    const UINT misses = scenario == 4 ? 1u : hit ? 0u : far_field ? 1u : 2u;
    if (result[zeros.size()] != UINT(hit) || result[zeros.size() + 1] != misses)
      throw std::runtime_error(
          "unexpected split-interval or masked screen probe callbacks");
    std::fprintf(receipt, "screen_gather_callbacks_%u_%u=hits %u; misses %u\n",
                 entry, scenario, result[zeros.size()],
                 result[zeros.size() + 1]);
    return out;
  }
  const UINT expected_depth = !active ? 0xdeadbeefu : geometry ? 0x40000000u
                                                            : far_field ? 0u : 2u;
  if (depth[0] != expected_depth ||
      !std::all_of(pixel.begin(), pixel.begin() + 3,
                   [active](UINT v) { return v == (active ? 0u : 0xdeadbeefu); }))
    throw std::runtime_error("screen probe depth or radiance mismatch");
  for (UINT i = 0; i < width; ++i) {
    const UINT *callback = result.data() + zeros.size() + 4 * i;
    const UINT *rec = result.data() + 5 * i;
    std::array<float, 8> ray{};
    std::memcpy(ray.data(), result.data() + zeros.size() + width * 4 + 8 * i,
                sizeof(ray));
    if (active && !i)
      std::fprintf(
          receipt,
          "screen_gather_ray_%u_%u=%.8g,%.8g,%.8g,%.8g;%.8g,%.8g,%.8g,%.8g\n",
          entry, geometry, ray[0], ray[1], ray[2], ray[3], ray[4], ray[5],
          ray[6], ray[7]);
    std::fprintf(receipt,
                 "screen_gather_%u_%u_%u_%u=%u,%u,%u,%u;record "
                 "%08x,%08x,%08x,%08x,%08x\n",
                 entry, active, geometry, i, callback[0], callback[1],
                 callback[2], callback[3], rec[0], rec[1], rec[2], rec[3],
                 rec[4]);
    const bool traced = active && i == 0;
    const UINT hits = traced && geometry ? 1u : 0u;
    const UINT misses = traced && !geometry ? (far_field ? 1u : 2u) : 0u;
    if (callback[0] != hits || callback[1] != misses)
      throw std::runtime_error("unexpected screen-probe traversal");
    const std::array<UINT, 5> expected_record =
        traced && !far_field && !geometry
            ? std::array<UINT, 5>{0, ~0u, 0x41880000u, ~0u, ~0u}
            : std::array<UINT, 5>{};
    if (!std::equal(expected_record.begin(), expected_record.end(), rec))
      throw std::runtime_error("unexpected screen-probe retrace record");
    if (traced) {
      const std::array<float, 8> expected_ray{geometry ? -1.f : 20.f,
                                              geometry ? 0.f : 20.f,
                                              geometry ? .49f : 19.99f,
                                              far_field ? .01f : 0.f,
                                              0,
                                              0,
                                              -1,
                                              geometry ? .49f : 17.f};
      for (UINT j = 0; j < ray.size(); ++j)
        if (!(std::abs(ray[j] - expected_ray[j]) < .00005f))
          throw std::runtime_error(
              "unexpected screen-probe ray bounds or geometry");
    }
  }
  return out;
}
