#include "../compute_buffers.h"
#include <fstream>
#include <iterator>
#include <vector>
using namespace compute_buffers;
constexpr UINT lanes = 64, slots = 70, total = 6 * slots, sentinel = 0xa5a55757;
static UINT bits(float v) {
  UINT out;
  std::memcpy(&out, &v, 4);
  return out;
}
static bool populated(UINT i) {
  return i == 1 || i == 7 || i == 31 || i == 32 || i == 47 || i == 60;
}
static void run(FILE *f, const char *mode, float forced, bool visible,
                UINT limit, bool frustum = false) {
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
  std::ifstream in("retail.dxbc", std::ios::binary);
  std::vector<char> code{std::istreambuf_iterator<char>{in},
                         std::istreambuf_iterator<char>{}};
  ComPtr<ID3D12RootSignature> root;
  check(guest->CreateRootSignature(0, code.data(), code.size(),
                                   IID_PPV_ARGS(&root)),
        "title root");
  D3D12_COMPUTE_PIPELINE_STATE_DESC desc{};
  desc.pRootSignature = root.Get();
  desc.CS = {code.data(), code.size()};
  using Pso = HRESULT(WINAPI *)(ID3D12Device *,
                                const D3D12_COMPUTE_PIPELINE_STATE_DESC *, UINT,
                                const void *, REFIID, void **);
  auto make = frame_abi::method<Pso>(guest.Get(), 62);
  ComPtr<ID3D12PipelineState> pso;
  check(make(guest.Get(), &desc, 0, nullptr, IID_PPV_ARGS(&pso)),
        "title slot 62");
  ComPtr<ID3D12Device> dev;
  check(root->GetDevice(IID_PPV_ARGS(&dev)), "owner");
  D3D12_DESCRIPTOR_HEAP_DESC hd{D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 96,
                                D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, 0};
  ComPtr<ID3D12DescriptorHeap> heap;
  check(dev->CreateDescriptorHeap(&hd, IID_PPV_ARGS(&heap)), "heap");
  UINT stride = dev->GetDescriptorHandleIncrementSize(hd.Type);
  using GpuHandle = UINT64(WINAPI *)(ID3D12DescriptorHeap *);
  const auto cpuStart = frame_abi::method<frame_abi::CpuHandle>(heap.Get(), 9)(heap.Get());
  const auto gpuStart = frame_abi::method<GpuHandle>(heap.Get(), 10)(heap.Get());
  auto cpu = [&](UINT i) { return D3D12_CPU_DESCRIPTOR_HANDLE{cpuStart + i * stride}; };
  auto gpu = [&](UINT i) { return D3D12_GPU_DESCRIPTOR_HANDLE{gpuStart + i * stride}; };
  auto input = [&](const void *data, UINT size) {
    auto r = buffer(dev.Get(), size, D3D12_HEAP_TYPE_UPLOAD,
                    D3D12_RESOURCE_STATE_GENERIC_READ);
    upload(r.Get(), data, size);
    return r;
  };
  std::array<UINT, 16384> c{};
  c[5 * 4] = bits(1);
  c[6 * 4 + 1] = bits(1);
  c[9 * 4] = limit;
  c[9 * 4 + 1] = bits(forced);
  c[9 * 4 + 2] = visible ? 1 : 0;
  c[9 * 4 + 3] = frustum ? 1 : 0;
  c[11 * 4] = bits(1);
  c[11 * 4 + 3] = bits(50);
  auto uniform = input(c.data(), sizeof(c));
  D3D12_CONSTANT_BUFFER_VIEW_DESC cbv{uniform->GetGPUVirtualAddress(),
                                      sizeof(c)};
  dev->CreateConstantBufferView(&cbv, cpu(64));
  std::array<INT, lanes * 6> boxes{};
  std::array<std::array<UINT, 4>, lanes> info{};
  std::array<std::array<UINT, 4>, 2 * lanes> lod{};
  for (UINT lane = 0; lane < lanes; ++lane) {
    for (UINT k = 0; k < 3; ++k) {
      boxes[6 * lane + k] = INT(lane) - 2;
      boxes[6 * lane + 3 + k] = INT(lane) + 2;
    }
    // LOD 1 contains zero vertices. Outside the plane a cluster chooses
    // this last record; inside it the zero screen-size threshold selects LOD 0.
    info[lane][0] = ((frustum ? 2u : 1u) << 24) | (2 * lane);
    lod[2 * lane] = {200 + 13 * lane, populated(lane) ? 2 + 2 * lane : 0,
                     populated(lane) ? 6 + 2 * lane : 0, 0x40003c00};
  }
  auto bb = input(boxes.data(), sizeof(boxes)),
       metadata = input(info.data(), sizeof(info)),
       details = input(lod.data(), sizeof(lod));
  ID3D12Resource *resources[] = {bb.Get(), metadata.Get(), details.Get()};
  for (UINT i = 0; i < 3; ++i) {
    D3D12_SHADER_RESOURCE_VIEW_DESC v{};
    v.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
    v.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
    v.Format = i ? DXGI_FORMAT_UNKNOWN : DXGI_FORMAT_R32_SINT;
    v.Buffer.NumElements = i == 0 ? lanes * 6 : (i == 1 ? lanes : 2 * lanes);
    v.Buffer.StructureByteStride = i ? 16 : 0;
    dev->CreateShaderResourceView(resources[i], &v, cpu(i));
  }
  std::array<UINT, total> initial;
  initial.fill(sentinel);
  initial[slots * 4] = 2;
  initial[slots * 5] = 17;
  auto uploadBuffer = input(initial.data(), sizeof(initial)),
       out = buffer(dev.Get(), sizeof(initial), D3D12_HEAP_TYPE_DEFAULT,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                    D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS),
       read = buffer(dev.Get(), sizeof(initial), D3D12_HEAP_TYPE_READBACK,
                     D3D12_RESOURCE_STATE_COPY_DEST);
  for (UINT i = 0; i < 6; ++i) {
    D3D12_UNORDERED_ACCESS_VIEW_DESC v{};
    v.ViewDimension = D3D12_UAV_DIMENSION_BUFFER;
    v.Format = i == 3 ? DXGI_FORMAT_R32_FLOAT : DXGI_FORMAT_R32_UINT;
    v.Buffer.FirstElement = i * slots;
    v.Buffer.NumElements = slots;
    dev->CreateUnorderedAccessView(out.Get(), nullptr, &v, cpu(80 + i));
  }
  ComPtr<ID3D12CommandQueue> queue;
  ComPtr<ID3D12CommandAllocator> allocator;
  ComPtr<ID3D12GraphicsCommandList> list;
  D3D12_COMMAND_QUEUE_DESC q{};
  check(dev->CreateCommandQueue(&q, IID_PPV_ARGS(&queue)), "queue");
  check(dev->CreateCommandAllocator(q.Type, IID_PPV_ARGS(&allocator)),
        "allocator");
  const UINT params[]{static_cast<UINT>(q.Type), 0, 1, 0};
  check(frame_abi::method<frame_abi::CreateList>(dev.Get(), 73)(dev.Get(), params, allocator.Get(), pso.Get(),
                                 IID_PPV_ARGS(&list)), "list");
  check(list->Reset(allocator.Get(), pso.Get()), "reset list");
  list->CopyBufferRegion(out.Get(), 0, uploadBuffer.Get(), 0, sizeof(initial));
  transition(list.Get(), out.Get(), D3D12_RESOURCE_STATE_COPY_DEST,
             D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
  using Heaps = void(WINAPI *)(ID3D12GraphicsCommandList *, UINT, ID3D12DescriptorHeap *const *);
  using Root = void(WINAPI *)(ID3D12GraphicsCommandList *, ID3D12RootSignature *);
  using Table = void(WINAPI *)(ID3D12GraphicsCommandList *, UINT, D3D12_GPU_DESCRIPTOR_HANDLE);
  frame_abi::method<Heaps>(list.Get(), 32)(list.Get(), 1, heap.GetAddressOf());
  frame_abi::method<Root>(list.Get(), 33)(list.Get(), root.Get());
  const auto table = frame_abi::method<Table>(list.Get(), 35);
  table(list.Get(), 0, gpu(0));
  table(list.Get(), 1, gpu(64));
  table(list.Get(), 3, gpu(80));
  list->Dispatch(1, 1, 1);
  transition(list.Get(), out.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
             D3D12_RESOURCE_STATE_COPY_SOURCE);
  list->CopyBufferRegion(read.Get(), 0, out.Get(), 0, sizeof(initial));
  submit_and_wait(dev.Get(), queue.Get(), list.Get());
  void *data = nullptr;
  D3D12_RANGE range{0, sizeof(initial)}, none{};
  check(read->Map(0, &range, &data), "readback");
  auto expected = initial;
  auto got = static_cast<const UINT *>(data);
  for (UINT lane = 0; lane < limit && visible; ++lane)
    if (populated(lane) && (!frustum || lane <= 53)) {
      UINT dst = expected[4 * slots]++;
      float blend = std::max(0.f, forced);
      UINT count = 2 + 2 * lane + UINT(4 * blend);
      expected[dst] = lane;
      expected[slots + dst] = 200 + 13 * lane;
      expected[2 * slots + dst] = count;
      expected[3 * slots + dst] = bits(1 + blend);
      expected[5 * slots] += 2 * count;
    }
  UINT matches = 0;
  for (UINT i = 0; i < total; ++i) {
    if (expected[i] == got[i])
      ++matches;
    else
      std::fprintf(f, "diff_%s_%u=%08x expected %08x\n", mode, i, got[i],
                   expected[i]);
  }
  std::fprintf(f, "%s_matched=%u\n%s_cluster_count=%u\n%s_draw_vertices=%u\n",
               mode, matches, mode, got[4 * slots], mode, got[5 * slots]);
  read->Unmap(0, &none);
  if (matches != total)
    throw std::runtime_error("culling mismatch");
}
int main() {
  FILE *f = nullptr;
  if (fopen_s(&f, "result.txt", "wb"))
    return 2;
  int rc = 0;
  try {
    run(f, "lod0", 0, true, 64);
    run(f, "blended", .5f, true, 48);
    run(f, "automatic", -1.f, true, 64);
    run(f, "hidden", 0, false, 64);
    run(f, "frustum", -1.f, true, 64, true);
  } catch (const std::exception &e) {
    std::fprintf(f, "error=%s\n", e.what());
    rc = 1;
  }
  return fclose(f) ? 2 : rc;
}
