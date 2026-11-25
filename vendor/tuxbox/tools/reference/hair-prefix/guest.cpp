#include "../compute_buffers.h"
#include <fstream>
#include <iterator>
#include <vector>
using namespace compute_buffers;
constexpr UINT block = 512, padded = 1024, guard = 0xcabb1212,
               values = padded * 2 + 12;
static void run(FILE *f, UINT entry, UINT count) {
  auto m = LoadLibraryW(L"d3d12_xs.dll");
  using Device = HRESULT(WINAPI *)(IUnknown *, const void *, REFIID, void **);
  auto make =
      m ? reinterpret_cast<Device>(GetProcAddress(m, "D3D12XboxCreateDevice"))
        : nullptr;
  if (!make)
    throw std::runtime_error("device factory");
  std::array<UINT, 18> profile{0x20004, 0, ~0u, 0, 0, 0, ~0u, ~0u, 0, 1};
  ComPtr<ID3D12Device> guest;
  check(make(nullptr, profile.data(), IID_PPV_ARGS(&guest)), "guest device");
  std::ifstream in(entry == 500 ? "500.dxbc" : "502.dxbc", std::ios::binary);
  std::vector<char> native{std::istreambuf_iterator<char>{in},
                           std::istreambuf_iterator<char>{}};
  ComPtr<ID3D12RootSignature> root;
  check(guest->CreateRootSignature(0, native.data(), native.size(),
                                   IID_PPV_ARGS(&root)),
        "original root");
  D3D12_COMPUTE_PIPELINE_STATE_DESC pd{};
  pd.pRootSignature = root.Get();
  pd.CS = {native.data(), native.size()};
  using Pso = HRESULT(WINAPI *)(ID3D12Device *,
                                const D3D12_COMPUTE_PIPELINE_STATE_DESC *, UINT,
                                const void *, REFIID, void **);
  ComPtr<ID3D12PipelineState> pso;
  check(frame_abi::method<Pso>(guest.Get(), 62)(guest.Get(), &pd, 0, nullptr,
                                                IID_PPV_ARGS(&pso)),
        "Xbox compute slot");
  ComPtr<ID3D12Device> dev;
  check(root->GetDevice(IID_PPV_ARGS(&dev)), "native device");
  auto inbuf = [&](const void *data, UINT size) {
    auto r = buffer(dev.Get(), size, D3D12_HEAP_TYPE_UPLOAD,
                    D3D12_RESOURCE_STATE_GENERIC_READ);
    upload(r.Get(), data, size);
    return r;
  };
  std::array<UINT, padded> input{};
  for (UINT i = 0; i < padded; ++i) {
    input[i] = i % 11 == 0 ? 0u : i % 5 == 0 ? 0x7fcabcddu : 3 * i + 5;
  }
  auto data = inbuf(input.data(), sizeof(input)), length = inbuf(&count, 4);
  std::array<UINT, values> initial;
  initial.fill(guard);
  auto seed = inbuf(initial.data(), sizeof(initial)),
       out = buffer(dev.Get(), sizeof(initial), D3D12_HEAP_TYPE_DEFAULT,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                    D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS),
       read = buffer(dev.Get(), sizeof(initial), D3D12_HEAP_TYPE_READBACK,
                     D3D12_RESOURCE_STATE_COPY_DEST);
  D3D12_DESCRIPTOR_HEAP_DESC hd{D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 96,
                                D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, 0};
  ComPtr<ID3D12DescriptorHeap> heap;
  check(dev->CreateDescriptorHeap(&hd, IID_PPV_ARGS(&heap)), "heap");
  UINT stride = dev->GetDescriptorHandleIncrementSize(hd.Type);
  using GpuHandle = UINT64(WINAPI *)(ID3D12DescriptorHeap *);
  const auto cpuStart = frame_abi::method<frame_abi::CpuHandle>(heap.Get(), 9)(heap.Get());
  const auto gpuStart = frame_abi::method<GpuHandle>(heap.Get(), 10)(heap.Get());
  auto cpu = [&](UINT i) {
    return D3D12_CPU_DESCRIPTOR_HANDLE{cpuStart + i * stride};
  };
  auto gpu = [&](UINT i) {
    return D3D12_GPU_DESCRIPTOR_HANDLE{gpuStart + i * stride};
  };
  for (UINT i = 0; i < 2; ++i) {
    D3D12_SHADER_RESOURCE_VIEW_DESC v{};
    v.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
    v.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
    v.Format = DXGI_FORMAT_R32_UINT;
    v.Buffer.NumElements = i ? padded : 1;
    dev->CreateShaderResourceView(i ? data.Get() : length.Get(), &v, cpu(i));
  }
  constexpr UINT head = 4, totals = head + 2 * padded + 2;
  for (UINT i = 0; i < 2; ++i) {
    D3D12_UNORDERED_ACCESS_VIEW_DESC v{};
    v.ViewDimension = D3D12_UAV_DIMENSION_BUFFER;
    v.Format = i ? DXGI_FORMAT_R32_UINT : DXGI_FORMAT_R32G32_UINT;
    v.Buffer.FirstElement = i ? totals : head / 2;
    v.Buffer.NumElements = i ? 4 : padded;
    dev->CreateUnorderedAccessView(out.Get(), nullptr, &v, cpu(80 + i));
  }
  ComPtr<ID3D12CommandQueue> queue;
  ComPtr<ID3D12CommandAllocator> alloc;
  ComPtr<ID3D12GraphicsCommandList> list;
  D3D12_COMMAND_QUEUE_DESC q{};
  check(dev->CreateCommandQueue(&q, IID_PPV_ARGS(&queue)), "queue");
  check(dev->CreateCommandAllocator(q.Type, IID_PPV_ARGS(&alloc)), "allocator");
  const UINT params[]{static_cast<UINT>(q.Type), 0, 1, 0};
  check(frame_abi::method<frame_abi::CreateList>(dev.Get(), 73)(
            dev.Get(), params, alloc.Get(), pso.Get(), IID_PPV_ARGS(&list)), "list");
  check(list->Reset(alloc.Get(), pso.Get()), "reset list");
  list->CopyBufferRegion(out.Get(), 0, seed.Get(), 0, sizeof(initial));
  transition(list.Get(), out.Get(), D3D12_RESOURCE_STATE_COPY_DEST,
             D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
  using Heaps = void(WINAPI *)(ID3D12GraphicsCommandList *, UINT, ID3D12DescriptorHeap *const *);
  using Root = void(WINAPI *)(ID3D12GraphicsCommandList *, ID3D12RootSignature *);
  using Table = void(WINAPI *)(ID3D12GraphicsCommandList *, UINT, D3D12_GPU_DESCRIPTOR_HANDLE);
  frame_abi::method<Heaps>(list.Get(), 32)(list.Get(), 1, heap.GetAddressOf());
  frame_abi::method<Root>(list.Get(), 33)(list.Get(), root.Get());
  const auto table = frame_abi::method<Table>(list.Get(), 35);
  table(list.Get(), 0, gpu(0));
  table(list.Get(), 3, gpu(80));
  list->Dispatch(entry == 500 ? 1 : 3, 1, 1);
  transition(list.Get(), out.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
             D3D12_RESOURCE_STATE_COPY_SOURCE);
  list->CopyBufferRegion(read.Get(), 0, out.Get(), 0, sizeof(initial));
  submit_and_wait(dev.Get(), queue.Get(), list.Get());
  auto expected = initial;
  UINT sum = 0;
  for (UINT i = 0; i < ((count + block - 1) / block) * block; ++i) {
    if (entry == 502 && i % block == 0)
      sum = 0;
    expected[head + 2 * i] = sum;
    if (i < count)
      sum += input[i];
    expected[head + 2 * i + 1] = sum;
    if (entry == 502 && i % block == block - 1)
      expected[totals + i / block] = sum;
  }
  D3D12_RANGE range{0, sizeof(initial)}, none{};
  void *mapped{};
  check(read->Map(0, &range, &mapped), "readback");
  const auto actual = static_cast<const UINT *>(mapped);
  UINT matched = 0, reported = 0;
  for (UINT i = 0; i < values; ++i) {
    if (actual[i] == expected[i])
      ++matched;
    else if (reported++ < 16)
      std::fprintf(f, "mismatch_%u_%u_%u=%08x expected %08x\n", entry, count, i,
                   actual[i], expected[i]);
  }
  read->Unmap(0, &none);
  std::fprintf(f, "entry_%u_count_%u=%u\n", entry, count, matched);
  std::fflush(f);
  if (matched != values)
    throw std::runtime_error("prefix sum mismatch");
}
int main() {
  FILE *f = nullptr;
  if (fopen_s(&f, "result.txt", "wb"))
    return 2;
  int rc = 0;
  try {
    for (UINT e : {502u, 500u})
      for (UINT n : {769u, 512u, 1u, 0u})
        run(f, e, n);
  } catch (const std::exception &e) {
    std::fprintf(f, "error=%s\n", e.what());
    rc = 1;
  }
  return fclose(f) ? 2 : rc;
}
