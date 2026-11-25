#include "../compute_buffers.h"
#include <array>
#include <fstream>
#include <iterator>
#include <vector>
using namespace compute_buffers;

static void run(FILE *out, UINT count) {
  auto dll = LoadLibraryW(L"d3d12_xs.dll");
  using Create = HRESULT(WINAPI *)(IUnknown *, const void *, REFIID, void **);
  auto create = dll ? reinterpret_cast<Create>(
                          GetProcAddress(dll, "D3D12XboxCreateDevice"))
                    : nullptr;
  if (!create)
    throw std::runtime_error("device factory");
  std::array<UINT, 18> profile{0x20004, 0, ~0u, 0, 0, 0, ~0u, ~0u, 0, 1};
  ComPtr<ID3D12Device> guest;
  check(create(nullptr, profile.data(), IID_PPV_ARGS(&guest)), "guest device");
  std::ifstream file("retail.dxbc", std::ios::binary);
  std::vector<char> native{std::istreambuf_iterator<char>{file}, {}};
  ComPtr<ID3D12RootSignature> root;
  check(guest->CreateRootSignature(0, native.data(), native.size(),
                                   IID_PPV_ARGS(&root)),
        "root");
  D3D12_COMPUTE_PIPELINE_STATE_DESC desc{};
  desc.pRootSignature = root.Get();
  desc.CS = {native.data(), native.size()};
  using Pso = HRESULT(WINAPI *)(ID3D12Device *,
                                const D3D12_COMPUTE_PIPELINE_STATE_DESC *, UINT,
                                const void *, REFIID, void **);
  ComPtr<ID3D12PipelineState> pso;
  check(frame_abi::method<Pso>(guest.Get(), 62)(guest.Get(), &desc, 0, nullptr,
                                                IID_PPV_ARGS(&pso)),
        "pso");
  ComPtr<ID3D12Device> owner;
  check(root->GetDevice(IID_PPV_ARGS(&owner)), "owner");
  auto dev = owner.Get();
  auto source =
      buffer(dev, 4, D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_STATE_GENERIC_READ);
  upload(source.Get(), &count, 4);
  constexpr UINT guard = 0xdabacafe;
  std::array<UINT, 16> initial;
  initial.fill(guard);
  auto seed = buffer(dev, sizeof(initial), D3D12_HEAP_TYPE_UPLOAD,
                     D3D12_RESOURCE_STATE_GENERIC_READ);
  upload(seed.Get(), initial.data(), sizeof(initial));
  auto dst = buffer(dev, sizeof(initial), D3D12_HEAP_TYPE_DEFAULT,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                    D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
  auto read = buffer(dev, sizeof(initial), D3D12_HEAP_TYPE_READBACK,
                     D3D12_RESOURCE_STATE_COPY_DEST);
  D3D12_DESCRIPTOR_HEAP_DESC hd{D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 96,
                                D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, 0};
  ComPtr<ID3D12DescriptorHeap> heap;
  check(dev->CreateDescriptorHeap(&hd, IID_PPV_ARGS(&heap)), "heap");
  const UINT step = dev->GetDescriptorHandleIncrementSize(hd.Type);
  using GpuHandle = UINT64(WINAPI *)(ID3D12DescriptorHeap *);
  const auto cpuStart =
      frame_abi::method<frame_abi::CpuHandle>(heap.Get(), 9)(heap.Get());
  const auto gpuStart =
      frame_abi::method<GpuHandle>(heap.Get(), 10)(heap.Get());
  auto cpu = [&](UINT i) {
    return D3D12_CPU_DESCRIPTOR_HANDLE{cpuStart + i * step};
  };
  auto gpu = [&](UINT i) {
    return D3D12_GPU_DESCRIPTOR_HANDLE{gpuStart + i * step};
  };
  D3D12_SHADER_RESOURCE_VIEW_DESC srv{};
  srv.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
  srv.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
  srv.Format = DXGI_FORMAT_R32_UINT;
  srv.Buffer.NumElements = 1;
  dev->CreateShaderResourceView(source.Get(), &srv, cpu(0));
  D3D12_UNORDERED_ACCESS_VIEW_DESC uav{};
  uav.ViewDimension = D3D12_UAV_DIMENSION_BUFFER;
  uav.Format = DXGI_FORMAT_R32_UINT;
  uav.Buffer.FirstElement = 4;
  uav.Buffer.NumElements = 3;
  dev->CreateUnorderedAccessView(dst.Get(), nullptr, &uav, cpu(80));
  ComPtr<ID3D12CommandQueue> queue;
  ComPtr<ID3D12CommandAllocator> alloc;
  ComPtr<ID3D12GraphicsCommandList> list;
  D3D12_COMMAND_QUEUE_DESC q{};
  check(dev->CreateCommandQueue(&q, IID_PPV_ARGS(&queue)), "queue");
  check(dev->CreateCommandAllocator(q.Type, IID_PPV_ARGS(&alloc)), "alloc");
  const UINT params[]{static_cast<UINT>(q.Type), 0, 1, 0};
  check(frame_abi::method<frame_abi::CreateList>(dev, 73)(
            dev, params, alloc.Get(), pso.Get(), IID_PPV_ARGS(&list)),
        "list");
  check(list->Reset(alloc.Get(), pso.Get()), "reset");
  list->CopyBufferRegion(dst.Get(), 0, seed.Get(), 0, sizeof(initial));
  transition(list.Get(), dst.Get(), D3D12_RESOURCE_STATE_COPY_DEST,
             D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
  using Heaps = void(WINAPI *)(ID3D12GraphicsCommandList *, UINT,
                               ID3D12DescriptorHeap *const *);
  using Root =
      void(WINAPI *)(ID3D12GraphicsCommandList *, ID3D12RootSignature *);
  using Table = void(WINAPI *)(ID3D12GraphicsCommandList *, UINT,
                               D3D12_GPU_DESCRIPTOR_HANDLE);
  frame_abi::method<Heaps>(list.Get(), 32)(list.Get(), 1, heap.GetAddressOf());
  frame_abi::method<Root>(list.Get(), 33)(list.Get(), root.Get());
  const auto table = frame_abi::method<Table>(list.Get(), 35);
  table(list.Get(), 0, gpu(0));
  table(list.Get(), 3, gpu(80));
  list->Dispatch(1, 1, 1);
  transition(list.Get(), dst.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
             D3D12_RESOURCE_STATE_COPY_SOURCE);
  list->CopyBufferRegion(read.Get(), 0, dst.Get(), 0, sizeof(initial));
  submit_and_wait(dev, queue.Get(), list.Get());
  void *mapped{};
  D3D12_RANGE range{0, sizeof(initial)}, none{};
  check(read->Map(0, &range, &mapped), "map");
  auto expected = initial;
  expected[4] = count;
  expected[5] = expected[6] = 1;
  const auto got = static_cast<const UINT *>(mapped);
  UINT matched = 0;
  for (UINT i = 0; i < initial.size(); ++i) {
    if (got[i] == expected[i])
      ++matched;
    else
      std::fprintf(out, "mismatch_%u_%u=%08x expected %08x\n", count, i, got[i],
                   expected[i]);
  }
  read->Unmap(0, &none);
  std::fprintf(out, "count_%u=%u\n", count, matched);
  if (matched != initial.size())
    throw std::runtime_error("argument mismatch");
}
int main() {
  FILE *f{};
  if (fopen_s(&f, "result.txt", "wb"))
    return 2;
  int status = 0;
  try {
    for (UINT count : {0u, 1u, 7u, 65537u, 0xffffffffu})
      run(f, count);
  } catch (const std::exception &e) {
    std::fprintf(f, "error=%s\n", e.what());
    status = 1;
  }
  return fclose(f) ? 2 : status;
}
