#include "../independent-shaders/context.h"
#include <array>
using namespace shader_control;

static constexpr UINT guard = 0xd15ea5ed, slots = 520;
static ComPtr<ID3D12Resource> texture(ID3D12Device *dev, DXGI_FORMAT fmt,
                                      UINT side, UINT samples,
                                      D3D12_RESOURCE_FLAGS flags,
                                      D3D12_RESOURCE_STATES state) {
  D3D12_HEAP_PROPERTIES heap{};
  heap.Type = D3D12_HEAP_TYPE_DEFAULT;
  heap.CreationNodeMask = heap.VisibleNodeMask = 1;
  D3D12_RESOURCE_DESC d{};
  d.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
  d.Width = d.Height = side;
  d.DepthOrArraySize = d.MipLevels = 1;
  d.SampleDesc.Count = samples;
  d.Format = fmt;
  d.Flags = flags;
  ComPtr<ID3D12Resource> result;
  check(dev->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &d, state,
                                     nullptr, IID_PPV_ARGS(&result)),
        "texture");
  return result;
}

static D3D12_TEXTURE_COPY_LOCATION
location(ID3D12Resource *r, DXGI_FORMAT fmt = DXGI_FORMAT_UNKNOWN,
         UINT side = 8) {
  D3D12_TEXTURE_COPY_LOCATION loc{};
  loc.pResource = r;
  if (fmt != DXGI_FORMAT_UNKNOWN) {
    loc.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
    loc.PlacedFootprint.Footprint = {fmt, side, side, 1, 256};
  }
  return loc;
}

static void run(Context &ctx, const std::string &id, UINT samples,
                bool indirect, UINT sortDepth) {
  auto dev = ctx.device.Get();
  auto native = read_shader(id + "-native"), translated = read_shader(id);
  ComPtr<ID3D12RootSignature> root, drawRoot;
  check(dev->CreateRootSignature(0, native.data(), native.size(),
                                 IID_PPV_ARGS(&root)),
        "original root");
  D3D12_COMPUTE_PIPELINE_STATE_DESC cs{};
  cs.pRootSignature = root.Get();
  cs.CS = {translated.data(), translated.size()};
  ComPtr<ID3D12PipelineState> compute, graphics;
  check(dev->CreateComputePipelineState(&cs, IID_PPV_ARGS(&compute)),
        "compute PSO");
  D3D12_ROOT_SIGNATURE_DESC rd{};
  ComPtr<ID3DBlob> blob;
  check(D3D12SerializeRootSignature(&rd, D3D_ROOT_SIGNATURE_VERSION_1, &blob,
                                    nullptr),
        "serialize draw root");
  check(dev->CreateRootSignature(0, blob->GetBufferPointer(),
                                 blob->GetBufferSize(),
                                 IID_PPV_ARGS(&drawRoot)),
        "draw root");
  auto vs = read_shader("vs"), ps = read_shader("ps");
  D3D12_GRAPHICS_PIPELINE_STATE_DESC pd{};
  pd.pRootSignature = drawRoot.Get();
  pd.VS = {vs.data(), vs.size()};
  pd.PS = {ps.data(), ps.size()};
  pd.PrimitiveTopologyType = D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE;
  pd.RasterizerState.FillMode = D3D12_FILL_MODE_SOLID;
  pd.RasterizerState.CullMode = D3D12_CULL_MODE_NONE;
  pd.RasterizerState.DepthClipEnable = TRUE;
  pd.RasterizerState.MultisampleEnable = TRUE;
  pd.SampleDesc.Count = samples;
  pd.SampleMask = UINT_MAX;
  pd.NumRenderTargets = 2;
  pd.RTVFormats[0] = DXGI_FORMAT_R32_UINT;
  pd.RTVFormats[1] = DXGI_FORMAT_R32_FLOAT;
  pd.BlendState.RenderTarget[0].RenderTargetWriteMask =
      D3D12_COLOR_WRITE_ENABLE_ALL;
  check(dev->CreateGraphicsPipelineState(&pd, IID_PPV_ARGS(&graphics)),
        "multisample graphics PSO");

  ComPtr<ID3D12CommandAllocator> a;
  ComPtr<ID3D12GraphicsCommandList> list;
  check(dev->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT,
                                    IID_PPV_ARGS(&a)),
        "allocator");
  check(dev->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, a.Get(),
                               graphics.Get(), IID_PPV_ARGS(&list)),
        "list");
  auto heaps = [&](D3D12_DESCRIPTOR_HEAP_TYPE type, UINT n,
                   D3D12_DESCRIPTOR_HEAP_FLAGS flags) {
    D3D12_DESCRIPTOR_HEAP_DESC desc{type, n, flags, 0};
    ComPtr<ID3D12DescriptorHeap> h;
    check(dev->CreateDescriptorHeap(&desc, IID_PPV_ARGS(&h)),
          "descriptor heap");
    return h;
  };
  auto rtv =
      heaps(D3D12_DESCRIPTOR_HEAP_TYPE_RTV, 2, D3D12_DESCRIPTOR_HEAP_FLAG_NONE);
  auto views = heaps(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 96,
                     D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE);
  UINT step = dev->GetDescriptorHandleIncrementSize(
      D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV);
  auto cpu = [&](UINT n) {
    return D3D12_CPU_DESCRIPTOR_HANDLE{
        views->GetCPUDescriptorHandleForHeapStart().ptr + n * step};
  };
  auto gpu = [&](UINT n) {
    return D3D12_GPU_DESCRIPTOR_HANDLE{
        views->GetGPUDescriptorHandleForHeapStart().ptr + n * step};
  };
  std::array<ComPtr<ID3D12Resource>, 2> ms;
  for (UINT i = 0; i < 2; i++) {
    ms[i] = texture(dev, pd.RTVFormats[i], 8, samples,
                    D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
                    D3D12_RESOURCE_STATE_RENDER_TARGET);
    auto handle = rtv->GetCPUDescriptorHandleForHeapStart();
    handle.ptr += i * dev->GetDescriptorHandleIncrementSize(
                          D3D12_DESCRIPTOR_HEAP_TYPE_RTV);
    dev->CreateRenderTargetView(ms[i].Get(), nullptr, handle);
    dev->CreateShaderResourceView(ms[i].Get(), nullptr, cpu(1 + i));
  }
  auto first = rtv->GetCPUDescriptorHandleForHeapStart();
  D3D12_VIEWPORT viewport{0, 0, 8, 8, 0, 1};
  D3D12_RECT scissor{0, 0, 8, 8};
  list->SetGraphicsRootSignature(drawRoot.Get());
  list->RSSetViewports(1, &viewport);
  list->RSSetScissorRects(1, &scissor);
  list->OMSetRenderTargets(2, &first, TRUE, nullptr);
  list->IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
  list->DrawInstanced(3, 1, 0, 0);
  for (auto &r : ms)
    transition(list.Get(), r.Get(), D3D12_RESOURCE_STATE_RENDER_TARGET,
               D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
  std::vector<ComPtr<ID3D12Resource>> keep;
  auto uploadWords = [&](const std::vector<UINT> &words) {
    auto r = buffer(dev, UINT(words.size() * 4), D3D12_HEAP_TYPE_UPLOAD,
                    D3D12_RESOURCE_STATE_GENERIC_READ);
    upload(r.Get(), words.data(), words.size() * 4);
    keep.push_back(r);
    return r;
  };
  auto seededTex = [&](DXGI_FORMAT fmt, UINT side, UINT value, bool uav) {
    auto r = texture(dev, fmt, side, 1,
                     uav ? D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS
                         : D3D12_RESOURCE_FLAG_NONE,
                     D3D12_RESOURCE_STATE_COPY_DEST);
    auto seed = uploadWords(std::vector<UINT>(64 * side, value));
    auto dest = location(r.Get()), src = location(seed.Get(), fmt, side);
    list->CopyTextureRegion(&dest, 0, 0, 0, &src, nullptr);
    transition(list.Get(), r.Get(), D3D12_RESOURCE_STATE_COPY_DEST,
               uav ? D3D12_RESOURCE_STATE_UNORDERED_ACCESS
                   : D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
    keep.push_back(r);
    return r;
  };
  auto scene = seededTex(DXGI_FORMAT_R32_FLOAT, 8, 0x3e800000, false);
  auto transmittance = seededTex(DXGI_FORMAT_R32_FLOAT, 8, 0x3e800000, false);
  dev->CreateShaderResourceView(scene.Get(), nullptr, cpu(0));
  dev->CreateShaderResourceView(transmittance.Get(), nullptr,
                                cpu(indirect ? 5 : 3));
  if (indirect) {
    for (UINT i = 0; i < 2; i++) {
      auto r = uploadWords(i ? std::vector<UINT>{0, 0} : std::vector<UINT>{1});
      D3D12_SHADER_RESOURCE_VIEW_DESC v{};
      v.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
      v.Format = i ? DXGI_FORMAT_R32G32_UINT : DXGI_FORMAT_R32_UINT;
      v.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
      v.Buffer.NumElements = 1;
      dev->CreateShaderResourceView(r.Get(), &v, cpu(3 + i));
    }
  }
  std::vector<UINT> constants(64);
  constants[0] = constants[1] = 8;
  constants[4] = slots;
  constants[5] = sortDepth;
  constants[8] = 0x3f800000;
  constants[10] = 1;
  constants[12] = 8;
  auto cb = uploadWords(constants);
  D3D12_CONSTANT_BUFFER_VIEW_DESC cv{cb->GetGPUVirtualAddress(), 256};
  dev->CreateConstantBufferView(&cv, cpu(64));
  std::array<ComPtr<ID3D12Resource>, 5> out, readback;
  for (UINT i = 0; i < 5; i++) {
    auto fmt = i == 4 ? DXGI_FORMAT_R32_FLOAT : DXGI_FORMAT_R32_UINT;
    UINT side = i ? 8 : 1;
    const bool isTexture = i != 2 && i != 3;
    if (isTexture) {
      out[i] = seededTex(fmt, side, i ? guard : 5, true);
      dev->CreateUnorderedAccessView(out[i].Get(), nullptr, nullptr,
                                     cpu(80 + i));
    } else {
      auto seed = uploadWords(std::vector<UINT>(slots * 2, guard));
      out[i] = buffer(dev, slots * 8, D3D12_HEAP_TYPE_DEFAULT,
                      D3D12_RESOURCE_STATE_COPY_DEST,
                      D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
      list->CopyBufferRegion(out[i].Get(), 0, seed.Get(), 0, slots * 8);
      transition(list.Get(), out[i].Get(), D3D12_RESOURCE_STATE_COPY_DEST,
                 D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
      D3D12_UNORDERED_ACCESS_VIEW_DESC u{};
      u.ViewDimension = D3D12_UAV_DIMENSION_BUFFER;
      u.Buffer.NumElements = slots;
      if (i == 2)
        u.Buffer.StructureByteStride = 8;
      else
        u.Format = DXGI_FORMAT_R32G32_UINT;
      dev->CreateUnorderedAccessView(out[i].Get(), nullptr, &u, cpu(80 + i));
    }
    readback[i] =
        buffer(dev, isTexture ? 256 * side : slots * 8,
               D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
  }
  list->SetDescriptorHeaps(1, views.GetAddressOf());
  list->SetComputeRootSignature(root.Get());
  list->SetPipelineState(compute.Get());
  list->SetComputeRootDescriptorTable(0, gpu(0));
  list->SetComputeRootDescriptorTable(1, gpu(64));
  list->SetComputeRootDescriptorTable(3, gpu(80));
  list->Dispatch(1, 1, 1);
  for (UINT i = 0; i < 5; i++) {
    transition(list.Get(), out[i].Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
               D3D12_RESOURCE_STATE_COPY_SOURCE);
    if (i == 2 || i == 3)
      list->CopyBufferRegion(readback[i].Get(), 0, out[i].Get(), 0, slots * 8);
    else {
      auto dest =
               location(readback[i].Get(),
                        i == 4 ? DXGI_FORMAT_R32_FLOAT : DXGI_FORMAT_R32_UINT,
                        i ? 8 : 1),
           src = location(out[i].Get());
      list->CopyTextureRegion(&dest, 0, 0, 0, &src, nullptr);
    }
  }
  submit_and_wait(dev, ctx.queue.Get(), list.Get());
  for (UINT i = 0; i < 5; i++)
    write_buffer(readback[i].Get(),
                 id + "-" + std::to_string(sortDepth) + "-u" +
                     std::to_string(i),
                 i == 2 || i == 3 ? slots * 8 : (i ? 2048 : 256));
}
int main() {
  FILE *f{};
  if (fopen_s(&f, "result.txt", "w"))
    return 1;
  try {
    Context ctx(f);
    std::ifstream cases("cases.txt");
    std::string id;
    UINT samples, indirect;
    while (cases >> id >> samples >> indirect) {
      for (UINT order = 0; order < 2; order++)
        run(ctx, id, samples, indirect != 0, order);
      fprintf(f, "completed=%s\n", id.c_str());
      fflush(f);
    }
  } catch (const std::exception &e) {
    fprintf(f, "error=%s\n", e.what());
    fclose(f);
    return 1;
  }
  fclose(f);
  return 0;
}
