#include "../compute_buffers.h"
#include <fstream>
#include <iterator>
#include <vector>

using namespace compute_buffers;
static std::vector<char> bytes(const char *path) {
  std::ifstream file(path, std::ios::binary);
  if (!file)
    throw std::runtime_error(path);
  return {std::istreambuf_iterator<char>(file),
          std::istreambuf_iterator<char>()};
}

static ComPtr<ID3D12DescriptorHeap>
heap(ID3D12Device *dev, D3D12_DESCRIPTOR_HEAP_TYPE type, UINT count) {
  D3D12_DESCRIPTOR_HEAP_DESC d{type, count,
                               D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, 0};
  ComPtr<ID3D12DescriptorHeap> result;
  check(dev->CreateDescriptorHeap(&d, IID_PPV_ARGS(&result)), "heap");
  return result;
}

static void run(FILE *f) {
  auto dll = LoadLibraryW(L"d3d12_xs.dll");
  using Create = HRESULT(WINAPI *)(IUnknown *, const void *, REFIID, void **);
  auto create = dll ? reinterpret_cast<Create>(
                          GetProcAddress(dll, "D3D12XboxCreateDevice"))
                    : nullptr;
  if (!create)
    throw std::runtime_error("device factory");
  std::array<UINT, 18> profile{0x20004, 0, ~0u, 0, 0, 0, ~0u, ~0u, 0, 1};
  ComPtr<ID3D12Device> guest, dev;
  check(create(nullptr, profile.data(), IID_PPV_ARGS(&guest)), "guest device");
  auto retail = bytes("retail.dxbc"), isolated = bytes("exchange.cso"),
       expected = bytes("expected.bin");
  ComPtr<ID3D12RootSignature> root;
  check(guest->CreateRootSignature(0, retail.data(), retail.size(),
                                   IID_PPV_ARGS(&root)),
        "retail root");
  check(root->GetDevice(IID_PPV_ARGS(&dev)), "host device");
  D3D12_COMPUTE_PIPELINE_STATE_DESC desc{};
  desc.pRootSignature = root.Get();
  desc.CS = {retail.data(), retail.size()};
  using Pso = HRESULT(WINAPI *)(ID3D12Device *,
                                const D3D12_COMPUTE_PIPELINE_STATE_DESC *, UINT,
                                const void *, REFIID, void **);
  ComPtr<ID3D12PipelineState> original, control;
  check(frame_abi::method<Pso>(guest.Get(), 62)(guest.Get(), &desc, 0, nullptr,
                                                IID_PPV_ARGS(&original)),
        "original PSO");
  desc.CS = {isolated.data(), isolated.size()};
  check(dev->CreateComputePipelineState(&desc, IID_PPV_ARGS(&control)),
        "exchange PSO");
  auto handles = heap(dev.Get(), D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 112);
  auto samplers = heap(dev.Get(), D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER, 16);
  UINT step = dev->GetDescriptorHandleIncrementSize(
      D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV);
  auto cpu = [&](UINT i) {
    return D3D12_CPU_DESCRIPTOR_HANDLE{
        handles->GetCPUDescriptorHandleForHeapStart().ptr + i * step};
  };
  auto gpu = [&](UINT i) {
    return D3D12_GPU_DESCRIPTOR_HANDLE{
        handles->GetGPUDescriptorHandleForHeapStart().ptr + i * step};
  };
  auto cb = buffer(dev.Get(), 4096, D3D12_HEAP_TYPE_UPLOAD,
                   D3D12_RESOURCE_STATE_GENERIC_READ);
  std::array<UINT, 1024> zero{};
  upload(cb.Get(), zero.data(), sizeof(zero));
  D3D12_CONSTANT_BUFFER_VIEW_DESC cbv{cb->GetGPUVirtualAddress(), 4096};
  dev->CreateConstantBufferView(&cbv, cpu(64));
  dev->CreateConstantBufferView(&cbv, cpu(65));
  D3D12_SHADER_RESOURCE_VIEW_DESC srv{};
  srv.ViewDimension = D3D12_SRV_DIMENSION_TEXTURE2D;
  srv.Format = DXGI_FORMAT_R32G32B32A32_FLOAT;
  srv.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
  srv.Texture2D.MipLevels = 1;
  for (UINT i = 0; i < 10; ++i)
    dev->CreateShaderResourceView(nullptr, &srv, cpu(i));
  D3D12_UNORDERED_ACCESS_VIEW_DESC uav{};
  uav.ViewDimension = D3D12_UAV_DIMENSION_TEXTURE2D;
  uav.Format = srv.Format;
  dev->CreateUnorderedAccessView(nullptr, nullptr, &uav, cpu(80));
  dev->CreateUnorderedAccessView(nullptr, nullptr, &uav, cpu(81));
  D3D12_SAMPLER_DESC samp{};
  samp.Filter = D3D12_FILTER_MIN_MAG_MIP_POINT;
  samp.AddressU = samp.AddressV = samp.AddressW =
      D3D12_TEXTURE_ADDRESS_MODE_CLAMP;
  samp.ComparisonFunc = D3D12_COMPARISON_FUNC_ALWAYS;
  samp.MaxAnisotropy = 1;
  dev->CreateSampler(&samp, samplers->GetCPUDescriptorHandleForHeapStart());
  UINT size = static_cast<UINT>(expected.size());
  auto output = buffer(dev.Get(), size, D3D12_HEAP_TYPE_DEFAULT,
                       D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                       D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
  auto read = buffer(dev.Get(), size, D3D12_HEAP_TYPE_READBACK,
                     D3D12_RESOURCE_STATE_COPY_DEST);
  uav = {};
  uav.ViewDimension = D3D12_UAV_DIMENSION_BUFFER;
  uav.Format = DXGI_FORMAT_R32_UINT;
  uav.Buffer.NumElements = size / 4;
  dev->CreateUnorderedAccessView(output.Get(), nullptr, &uav, cpu(96));
  ComPtr<ID3D12CommandQueue> queue;
  ComPtr<ID3D12CommandAllocator> allocator;
  ComPtr<ID3D12GraphicsCommandList> list;
  D3D12_COMMAND_QUEUE_DESC q{};
  check(dev->CreateCommandQueue(&q, IID_PPV_ARGS(&queue)), "queue");
  check(dev->CreateCommandAllocator(q.Type, IID_PPV_ARGS(&allocator)),
        "allocator");
  check(dev->CreateCommandList(0, q.Type, allocator.Get(), original.Get(),
                               IID_PPV_ARGS(&list)),
        "list");
  ID3D12DescriptorHeap *heaps[]{handles.Get(), samplers.Get()};
  list->SetDescriptorHeaps(2, heaps);
  list->SetComputeRootSignature(root.Get());
  list->SetComputeRootDescriptorTable(0, gpu(0));
  list->SetComputeRootDescriptorTable(1, gpu(64));
  list->SetComputeRootDescriptorTable(
      2, samplers->GetGPUDescriptorHandleForHeapStart());
  list->SetComputeRootDescriptorTable(3, gpu(80));
  list->Dispatch(2, 2, 1);
  list->SetPipelineState(control.Get());
  list->SetComputeRootDescriptorTable(3, gpu(96));
  list->Dispatch(4, 1, 1);
  transition(list.Get(), output.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
             D3D12_RESOURCE_STATE_COPY_SOURCE);
  list->CopyBufferRegion(read.Get(), 0, output.Get(), 0, size);
  submit_and_wait(dev.Get(), queue.Get(), list.Get());
  void *mapped{};
  D3D12_RANGE range{0, size}, none{};
  check(read->Map(0, &range, &mapped), "readback");
  UINT matched = 0;
  for (UINT i = 0; i < size; i += 4)
    matched += std::memcmp(static_cast<char *>(mapped) + i, expected.data() + i,
                           4) == 0;
  read->Unmap(0, &none);
  std::fprintf(f, "retail_dispatch_groups=4\nmatched=%u\n", matched);
  if (matched != size / 4)
    throw std::runtime_error("exchange mismatch");
}

int main() {
  FILE *f = nullptr;
  if (fopen_s(&f, "result.txt", "wb"))
    return 2;
  int status = 0;
  try {
    run(f);
  } catch (const std::exception &e) {
    std::fprintf(f, "error=%s\n", e.what());
    status = 1;
  }
  return fclose(f) ? 2 : status;
}
