#include "frame_abi.h"
#include <array>
#include <cstdio>
#include <cstring>
#include <fstream>
#include <iterator>
#include <vector>
using frame_abi::check;
using frame_abi::ComPtr;
static ComPtr<ID3D12Resource> buffer(ID3D12Device *dev, UINT64 bytes,
                                     D3D12_HEAP_TYPE type,
                                     D3D12_RESOURCE_STATES state) {
  D3D12_HEAP_PROPERTIES props{};
  props.Type = type;
  props.CreationNodeMask = props.VisibleNodeMask = 1;
  D3D12_RESOURCE_DESC desc{};
  desc.Dimension = D3D12_RESOURCE_DIMENSION_BUFFER;
  desc.Width = bytes;
  desc.Height = desc.DepthOrArraySize = desc.MipLevels = desc.SampleDesc.Count =
      1;
  desc.Layout = D3D12_TEXTURE_LAYOUT_ROW_MAJOR;
  ComPtr<ID3D12Resource> out;
  check(dev->CreateCommittedResource(&props, D3D12_HEAP_FLAG_NONE, &desc, state,
                                     nullptr, IID_PPV_ARGS(&out)),
        "buffer");
  return out;
}
static void draw(ID3D12Device *dev, const char *file,
                 std::array<float, 4> expected) {
  D3D12_DESCRIPTOR_RANGE ranges[2] = {
      {D3D12_DESCRIPTOR_RANGE_TYPE_CBV, 5, 0, 0, 0},
      {D3D12_DESCRIPTOR_RANGE_TYPE_SRV, 5, 0, 0, 5}};
  D3D12_ROOT_PARAMETER param{};
  param.ParameterType = D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE;
  param.DescriptorTable = {2, ranges};
  param.ShaderVisibility = D3D12_SHADER_VISIBILITY_VERTEX;
  D3D12_STATIC_SAMPLER_DESC sampler{};
  sampler.AddressU = sampler.AddressV = sampler.AddressW =
      D3D12_TEXTURE_ADDRESS_MODE_CLAMP;
  sampler.Filter = D3D12_FILTER_MIN_MAG_MIP_POINT;
  sampler.ComparisonFunc = D3D12_COMPARISON_FUNC_ALWAYS;
  sampler.MaxLOD = D3D12_FLOAT32_MAX;
  sampler.ShaderVisibility = D3D12_SHADER_VISIBILITY_VERTEX;
  D3D12_ROOT_SIGNATURE_DESC desc{
      1, &param, 1, &sampler,
      D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT |
          D3D12_ROOT_SIGNATURE_FLAG_ALLOW_STREAM_OUTPUT};
  ComPtr<ID3DBlob> blob;
  check(D3D12SerializeRootSignature(&desc, D3D_ROOT_SIGNATURE_VERSION_1, &blob,
                                    nullptr),
        "serialize");
  ComPtr<ID3D12RootSignature> root;
  check(dev->CreateRootSignature(0, blob->GetBufferPointer(),
                                 blob->GetBufferSize(), IID_PPV_ARGS(&root)),
        "root");
  std::ifstream stream(file, std::ios::binary);
  std::vector<char> code{std::istreambuf_iterator<char>(stream), {}};
  D3D12_INPUT_ELEMENT_DESC elems[7]{};
  const UINT attributes[] = {0, 1, 2, 3, 4, 5, 13};
  for (UINT i = 0; i < 7; ++i)
    elems[i] = {"ATTRIBUTE",
                attributes[i],
                i == 6 ? DXGI_FORMAT_R32_UINT : DXGI_FORMAT_R32G32B32A32_FLOAT,
                0,
                i * 16,
                D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
                0};
  D3D12_SO_DECLARATION_ENTRY outputs[] = {{0, "SV_Position", 0, 0, 4, 0},
                                          {0, "TEXCOORD", 6, 0, 4, 0},
                                          {0, "PRIMITIVE_ID", 0, 0, 1, 0}};
  UINT stride = 36;
  D3D12_GRAPHICS_PIPELINE_STATE_DESC p{};
  p.pRootSignature = root.Get();
  p.VS = {code.data(), code.size()};
  p.StreamOutput = {outputs, 3, &stride, 1, D3D12_SO_NO_RASTERIZED_STREAM};
  p.InputLayout = {elems, 7};
  p.PrimitiveTopologyType = D3D12_PRIMITIVE_TOPOLOGY_TYPE_POINT;
  p.SampleMask = ~0u;
  p.SampleDesc.Count = 1;
  p.RasterizerState.FillMode = D3D12_FILL_MODE_SOLID;
  p.RasterizerState.CullMode = D3D12_CULL_MODE_NONE;
  ComPtr<ID3D12PipelineState> state;
  check(dev->CreateGraphicsPipelineState(&p, IID_PPV_ARGS(&state)), "pipeline");
  auto zero = buffer(dev, 65536, D3D12_HEAP_TYPE_UPLOAD,
                     D3D12_RESOURCE_STATE_GENERIC_READ);
  auto cb = buffer(dev, 65536, D3D12_HEAP_TYPE_UPLOAD,
                   D3D12_RESOURCE_STATE_GENERIC_READ);
  auto inputs = buffer(dev, 65536, D3D12_HEAP_TYPE_UPLOAD,
                       D3D12_RESOURCE_STATE_GENERIC_READ);
  for (auto resource : {zero.Get(), cb.Get(), inputs.Get()}) {
    void *map{};
    check(resource->Map(0, nullptr, &map), "upload");
    std::memset(map, 0, 65536);
    if (resource == cb.Get())
      std::memcpy(static_cast<float *>(map) + 20 * 4, expected.data(), 16);
    if (resource == inputs.Get()) {
      // The native positive ATTRIBUTE13 branch reads t2[attr + instance].
      // Its low 28 bits index t1, where the native 20-bit primitive ID resides.
      auto words = static_cast<UINT *>(map);
      words[96 / 4] = 3;
      words[512 / 4 + 3] = 2;
      words[512 / 4 + 4] = 3;
      words[1024 / 4 + 2 * 4] = 17;
      words[1024 / 4 + 3 * 4] = 29;
    }
    resource->Unmap(0, nullptr);
  }
  D3D12_DESCRIPTOR_HEAP_DESC heapdesc{
      D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 10,
      D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, 0};
  ComPtr<ID3D12DescriptorHeap> heap;
  check(dev->CreateDescriptorHeap(&heapdesc, IID_PPV_ARGS(&heap)), "heap");
  auto handle = heap->GetCPUDescriptorHandleForHeapStart();
  const auto step = dev->GetDescriptorHandleIncrementSize(heapdesc.Type);
  for (UINT i = 0; i < 5; i++) {
    D3D12_CONSTANT_BUFFER_VIEW_DESC view{
        (i == 2 ? cb : zero)->GetGPUVirtualAddress(), 65536};
    dev->CreateConstantBufferView(&view, handle);
    handle.ptr += step;
  }
  for (UINT i = 0; i < 5; i++) {
    D3D12_SHADER_RESOURCE_VIEW_DESC view{};
    view.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
    view.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
    view.Buffer.NumElements = 256;
    if (i == 4 || (i == 3 && !strcmp(file, "mat31788.cso"))) {
      view.ViewDimension = D3D12_SRV_DIMENSION_TEXTURE2D;
      view.Texture2D = {};
      view.Texture2D.MipLevels = 1;
      view.Format = i == 3 ? DXGI_FORMAT_R32G32B32A32_UINT
                           : DXGI_FORMAT_R32G32B32A32_FLOAT;
      dev->CreateShaderResourceView(nullptr, &view, handle);
      handle.ptr += step;
      continue;
    }
    if (i == 3)
      view.Format = DXGI_FORMAT_R32G32B32A32_FLOAT;
    else
      view.Buffer.StructureByteStride = i == 2 ? 4 : 16;
    if (i == 1)
      view.Buffer.FirstElement = 1024 / 16;
    if (i == 2)
      view.Buffer.FirstElement = 512 / 4;
    dev->CreateShaderResourceView(i == 1 || i == 2 ? inputs.Get() : zero.Get(),
                                  &view, handle);
    handle.ptr += step;
  }
  auto so =
      buffer(dev, 256, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COPY_DEST);
  auto rb = buffer(dev, 256, D3D12_HEAP_TYPE_READBACK,
                   D3D12_RESOURCE_STATE_COPY_DEST);
  ComPtr<ID3D12CommandAllocator> alloc;
  check(dev->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT,
                                    IID_PPV_ARGS(&alloc)),
        "allocator");
  ComPtr<ID3D12GraphicsCommandList> cmd;
  check(dev->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, alloc.Get(),
                               state.Get(), IID_PPV_ARGS(&cmd)),
        "command list");
  cmd->CopyBufferRegion(so.Get(), 0, zero.Get(), 0, 256);
  D3D12_RESOURCE_BARRIER tr{};
  tr.Transition = {so.Get(), 0, D3D12_RESOURCE_STATE_COPY_DEST,
                   D3D12_RESOURCE_STATE_STREAM_OUT};
  cmd->ResourceBarrier(1, &tr);
  ID3D12DescriptorHeap *heaps[] = {heap.Get()};
  cmd->SetDescriptorHeaps(1, heaps);
  cmd->SetGraphicsRootSignature(root.Get());
  cmd->SetGraphicsRootDescriptorTable(
      0, heap->GetGPUDescriptorHandleForHeapStart());
  D3D12_VERTEX_BUFFER_VIEW vb{inputs->GetGPUVirtualAddress(), 256, 112};
  cmd->IASetVertexBuffers(0, 1, &vb);
  cmd->IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_POINTLIST);
  D3D12_STREAM_OUTPUT_BUFFER_VIEW out{so->GetGPUVirtualAddress(), 112,
                                      so->GetGPUVirtualAddress() + 128};
  cmd->SOSetTargets(0, 1, &out);
  cmd->DrawInstanced(1, 2, 0, 0);
  tr.Transition.StateBefore = D3D12_RESOURCE_STATE_STREAM_OUT;
  tr.Transition.StateAfter = D3D12_RESOURCE_STATE_COPY_SOURCE;
  cmd->ResourceBarrier(1, &tr);
  cmd->CopyBufferRegion(rb.Get(), 0, so.Get(), 0, 256);
  check(cmd->Close(), "close");
  ComPtr<ID3D12CommandQueue> q;
  D3D12_COMMAND_QUEUE_DESC qd{};
  check(dev->CreateCommandQueue(&qd, IID_PPV_ARGS(&q)), "queue");
  ID3D12CommandList *lists[] = {cmd.Get()};
  q->ExecuteCommandLists(1, lists);
  ComPtr<ID3D12Fence> f;
  check(dev->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&f)), "fence");
  check(q->Signal(f.Get(), 1), "signal");
  check(f->SetEventOnCompletion(1, nullptr), "wait");
  void *data{};
  check(rb->Map(0, nullptr, &data), "readback");
  const auto values = static_cast<float *>(data);
  UINT bytes{};
  memcpy(&bytes, static_cast<char *>(data) + 128, sizeof(bytes));
  bool ok = bytes == 72 &&
            memcmp(values + 18, std::array<float, 4>{}.data(), 16) == 0;
  for (UINT i = 0; i < 2; ++i) {
    const float *record = values + i * 9;
    UINT id;
    memcpy(&id, record + 8, 4);
    printf(
        "%s instance %u position=(%g,%g,%g,%g) TEXCOORD6=(%g,%g,%g,%g) id=%u\n",
        file, i, record[0], record[1], record[2], record[3], record[4],
        record[5], record[6], record[7], id);
    ok = ok && memcmp(record, expected.data(), 16) == 0 &&
         memcmp(record + 4, expected.data(), 16) == 0 && id == (i ? 29u : 17u);
  }
  rb->Unmap(0, nullptr);
  if (!ok)
    throw std::runtime_error("incorrect vertex output");
}
int main() {
  if (!freopen("vertex.txt", "w", stdout))
    return 2;
  try {
    ComPtr<ID3D12Device> dev;
    check(
        D3D12CreateDevice(nullptr, D3D_FEATURE_LEVEL_11_0, IID_PPV_ARGS(&dev)),
        "device");
    ComPtr<IDXGIFactory4> factory;
    ComPtr<IDXGIAdapter1> adapter;
    DXGI_ADAPTER_DESC1 gpu{};
    check(CreateDXGIFactory1(IID_PPV_ARGS(&factory)), "create DXGI factory");
    check(factory->EnumAdapterByLuid(dev->GetAdapterLuid(),
                                     IID_PPV_ARGS(&adapter)),
          "select device adapter");
    check(adapter->GetDesc1(&gpu), "get adapter description");
    printf("adapter=%04x:%04x\n", gpu.VendorId, gpu.DeviceId);
    for (auto file : {"mat6113.cso", "mat31788.cso"})
      for (auto expected : {std::array<float, 4>{.25f, -.5f, .75f, 2.f},
                            std::array<float, 4>{-.875f, .125f, 1.f, 1.5f}})
        draw(dev.Get(), file, expected);
    return 0;
  } catch (const std::exception &e) {
    printf("%s\n", e.what());
    return 1;
  }
}
