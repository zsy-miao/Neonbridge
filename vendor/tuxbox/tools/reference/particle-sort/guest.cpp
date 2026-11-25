#include "../compute_buffers.h"
#include <fstream>
#include <iterator>
#include <vector>
using namespace compute_buffers;
constexpr UINT stride = 128, base = 6, count_slot = 3, block = 128,
               words = 3 * block, guard = 0x99abcd01, high_key = 0x23050000;
static bool tagged(UINT i) {
  return i == 1 || i == 31 || i == 32 || i == 63 || i == 64 || i == 95 ||
         i == 98;
}
// The last two values retain distinct low bits in a full-width key, but
// round onto existing keys in the half-precision variant.
constexpr float attribute[] = {-2.f, -1.f, 0.f, 1.f, 2.f, 4.f, 1.f + 1.f / 65536, -2.f + 1.f / 32768};
constexpr UINT halves[] = {0xc000u, 0xbc00u, 0u, 0x3c00u, 0x4000u, 0x4400u, 0x3c00u, 0xc000u};
static_assert(std::size(attribute) == std::size(halves));
static UINT bits(float x) {
  UINT v;
  std::memcpy(&v, &x, 4);
  return v;
}
static void run(FILE *f, UINT mode, UINT live, UINT capacity, bool use_tag, bool full_width) {
  auto m = LoadLibraryW(L"d3d12_xs.dll");
  using Device = HRESULT(WINAPI *)(IUnknown *, const void *, REFIID, void **);
  auto make =
      m ? reinterpret_cast<Device>(GetProcAddress(m, "D3D12XboxCreateDevice"))
        : nullptr;
  if (!make)
    throw std::runtime_error("device factory");
  std::array<UINT, 18> profile{0x20004, 0, ~0u, 0, 0, 0, ~0u, ~0u, 0, 1};
  ComPtr<ID3D12Device> dev;
  check(make(nullptr, profile.data(), IID_PPV_ARGS(&dev)), "guest device");
  std::ifstream in("retail.dxbc", std::ios::binary);
  std::vector<char> native{std::istreambuf_iterator<char>{in},
                           std::istreambuf_iterator<char>{}};
  ComPtr<ID3D12RootSignature> root;
  check(dev->CreateRootSignature(0, native.data(), native.size(),
                                   IID_PPV_ARGS(&root)),
        "original root");
  D3D12_COMPUTE_PIPELINE_STATE_DESC pd{};
  pd.pRootSignature = root.Get();
  pd.CS = {native.data(), native.size()};
  using Pso = HRESULT(WINAPI *)(ID3D12Device *,
                                const D3D12_COMPUTE_PIPELINE_STATE_DESC *, UINT,
                                const void *, REFIID, void **);
  ComPtr<ID3D12PipelineState> pso;
  check(frame_abi::method<Pso>(dev.Get(), 62)(dev.Get(), &pd, 0, nullptr,
                                                IID_PPV_ARGS(&pso)),
        "Xbox compute slot");
  auto inbuf = [&](const void *data, UINT size) {
    auto r = buffer(dev.Get(), size, D3D12_HEAP_TYPE_UPLOAD,
                    D3D12_RESOURCE_STATE_GENERIC_READ);
    upload(r.Get(), data, size);
    return r;
  };
  std::array<UINT, 4 * stride> attributes{};
  std::array<UINT, stride> tags{};
  std::array<UINT, 8> counts{};
  counts[1] = live;
  for (UINT i = 0; i < stride; i++) {
    attributes[i] = bits(static_cast<float>(i % 7));
    attributes[stride + i] = bits(static_cast<float>(i % 5));
    attributes[2 * stride + i] = bits(static_cast<float>(i % 3));
    attributes[3 * stride + i] = bits(attribute[i % std::size(attribute)]);
    tags[i] = tagged(i) ? 77 : 13;
  }
  // Attribute strides, capacity, indirect count slot, output count slot, batch
  // and output offset.
  std::array<UINT, 16384> constants{};
  constants[8] = constants[9] = constants[10] = stride;
  constants[11] = capacity;
  constants[12] = 1;
  constants[13] = count_slot;
  constants[14] = full_width ? 0 : high_key;
  constants[15] = base;
  constants[23] = mode;
  constants[24] = 3; // sort method and custom float attribute offset
  if (full_width) {
    constants[25] = ~0u; // reserved key for invisible particles
    constants[26] = 0;   // retain all key bits
    constants[27] = 0x80000000u;
  }
  constants[28] = 0;
  constants[29] = constants[30] = ~0u; // position; default rotation and scale
  constants[72] = 77;
  constants[73] = use_tag ? 0u : ~0u;
  constants[75] = ~0u;
  constants[76] = bits(0);
  constants[77] = bits(1000000.f);
  auto floating = inbuf(attributes.data(), sizeof(attributes)),
       integer = inbuf(tags.data(), sizeof(tags)),
       count = inbuf(counts.data(), sizeof(counts)),
       cb = inbuf(constants.data(), sizeof(constants));
  std::array<UINT, words> initial;
  initial.fill(guard);
  initial[2 * block + count_slot] = 7;
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
  UINT step = dev->GetDescriptorHandleIncrementSize(hd.Type);
  auto cpu = [&](UINT i) {
    return D3D12_CPU_DESCRIPTOR_HANDLE{
        frame_abi::method<frame_abi::CpuHandle>(heap.Get(), 9)(heap.Get()) + i * step};
  };
  using GpuHandle = UINT64(WINAPI *)(ID3D12DescriptorHeap *);
  auto gpu = [&](UINT i) {
    return D3D12_GPU_DESCRIPTOR_HANDLE{
        frame_abi::method<GpuHandle>(heap.Get(), 10)(heap.Get()) + i * step};
  };
  D3D12_CONSTANT_BUFFER_VIEW_DESC cbv{cb->GetGPUVirtualAddress(),
                                      sizeof(constants)};
  dev->CreateConstantBufferView(&cbv, cpu(64));
  ID3D12Resource *inputs[] = {floating.Get(), floating.Get(), integer.Get(),
                              count.Get()};
  UINT lengths[] = {4 * stride, 4 * stride, stride, 8};
  for (UINT i = 0; i < 4; i++) {
    D3D12_SHADER_RESOURCE_VIEW_DESC srv{};
    srv.Format = i < 2 ? DXGI_FORMAT_R32_FLOAT : DXGI_FORMAT_R32_UINT;
    srv.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
    srv.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
    srv.Buffer.NumElements = lengths[i];
    dev->CreateShaderResourceView(inputs[i], &srv, cpu(i));
  }
  for (UINT i = 0; i < 3; i++) {
    D3D12_UNORDERED_ACCESS_VIEW_DESC v{};
    v.ViewDimension = D3D12_UAV_DIMENSION_BUFFER;
    v.Format = DXGI_FORMAT_R32_UINT;
    v.Buffer.FirstElement = i * block;
    v.Buffer.NumElements = block;
    dev->CreateUnorderedAccessView(out.Get(), nullptr, &v, cpu(80 + i));
  }
  ComPtr<ID3D12CommandQueue> queue;
  ComPtr<ID3D12CommandAllocator> alloc;
  ComPtr<ID3D12GraphicsCommandList> list;
  D3D12_COMMAND_QUEUE_DESC q{};
  check(dev->CreateCommandQueue(&q, IID_PPV_ARGS(&queue)), "queue");
  check(dev->CreateCommandAllocator(q.Type, IID_PPV_ARGS(&alloc)), "allocator");
  const UINT list_params[]{static_cast<UINT>(q.Type), 0, 1, 0};
  check(frame_abi::method<frame_abi::CreateList>(dev.Get(), 73)(
            dev.Get(), list_params, alloc.Get(), pso.Get(), IID_PPV_ARGS(&list)), "Xbox command list");
  check(list->Reset(alloc.Get(), pso.Get()), "reset command list");
  list->CopyBufferRegion(out.Get(), 0, seed.Get(), 0, sizeof(initial));
  transition(list.Get(), out.Get(), D3D12_RESOURCE_STATE_COPY_DEST,
             D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
  using Heaps = void(WINAPI *)(ID3D12GraphicsCommandList *, UINT, ID3D12DescriptorHeap *const *);
  using Root = void(WINAPI *)(ID3D12GraphicsCommandList *, ID3D12RootSignature *);
  using Table = void(WINAPI *)(ID3D12GraphicsCommandList *, UINT, D3D12_GPU_DESCRIPTOR_HANDLE);
  frame_abi::method<Heaps>(list.Get(), 32)(list.Get(), 1, heap.GetAddressOf());
  frame_abi::method<Root>(list.Get(), 33)(list.Get(), root.Get());
  auto set_table = frame_abi::method<Table>(list.Get(), 35);
  set_table(list.Get(), 0, gpu(0));
  set_table(list.Get(), 1, gpu(64));
  set_table(list.Get(), 3, gpu(80));
  list->Dispatch(2, 1, 1);
  transition(list.Get(), out.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
             D3D12_RESOURCE_STATE_COPY_SOURCE);
  list->CopyBufferRegion(read.Get(), 0, out.Get(), 0, sizeof(initial));
  submit_and_wait(dev.Get(), queue.Get(), list.Get());
  auto expected = initial;
  UINT visible = 0;
  for (UINT i = 0; i < capacity; i++) {
    bool accepted = i < live && (!use_tag || tagged(i));
    UINT raw = full_width ? bits(attribute[i % std::size(attribute)]) : halves[i % std::size(halves)];
    UINT sign = full_width ? 0x80000000u : 0x8000u;
    UINT mask = full_width ? ~0u : 0xffffu;
    UINT key = (raw & sign) ? (~raw & mask) : raw | sign;
    if (mode == 4)
      key ^= mask;
    expected[base + i] = constants[14] | (accepted ? key : mask);
    expected[block + base + i] = i;
    visible += accepted;
  }
  expected[2 * block + count_slot] += visible;
  D3D12_RANGE range{0, sizeof(initial)}, none{};
  void *mapped{};
  check(read->Map(0, &range, &mapped), "readback");
  const auto actual = static_cast<const UINT *>(mapped);
  UINT matched = 0, printed = 0;
  for (UINT i = 0; i < words; ++i) {
    if (actual[i] == expected[i])
      ++matched;
    else if (printed++ < 8)
      std::fprintf(f, "mismatch_%u_%u=%08x expected %08x\n", live, i, actual[i],
                   expected[i]);
  }
  std::fprintf(f, "live_%u_matched=%u\nlive_%u_count=%u\n", live, matched, live,
               actual[block * 2 + count_slot]);
  read->Unmap(0, &none);
  if (matched != words)
    throw std::runtime_error("particle sort mismatch");
}
int main() {
  FILE *f = nullptr;
  if (fopen_s(&f, "result.txt", "wb"))
    return 2;
  int rc = 0;
  try {
    bool full_width = GetFileAttributesW(L"full-width") != INVALID_FILE_ATTRIBUTES;
    run(f, 3, 67, 103, false, full_width);
    run(f, 4, 99, 103, true, full_width);
    run(f, 3, 0, 103, false, full_width);
  } catch (const std::exception &e) {
    std::fprintf(f, "error=%s\n", e.what());
    rc = 1;
  }
  return fclose(f) ? 2 : rc;
}
