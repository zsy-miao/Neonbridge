#include "../compute_buffers.h"
#include <fstream>
#include <iterator>
#include <vector>
using namespace compute_buffers;
constexpr UINT variants = 16, increment = 16, region = 1024, view = 768,
               words = view / sizeof(UINT);
static ComPtr<ID3D12DescriptorHeap> descriptorHeap(ID3D12Device *d,
                                                   bool visible) {
  D3D12_DESCRIPTOR_HEAP_DESC desc{
      D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 2 * variants,
      visible ? D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE
              : D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
      0};
  ComPtr<ID3D12DescriptorHeap> heap;
  check(d->CreateDescriptorHeap(&desc, IID_PPV_ARGS(&heap)), "heap");
  return heap;
}
static UINT bits(float f) {
  UINT value;
  std::memcpy(&value, &f, 4);
  return value;
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
  ComPtr<ID3D12Device> guest;
  check(create(nullptr, profile.data(), IID_PPV_ARGS(&guest)), "Frosty device");
  D3D12_DESCRIPTOR_RANGE ranges[] = {
      {D3D12_DESCRIPTOR_RANGE_TYPE_CBV, 1, 0, 0, 0},
      {D3D12_DESCRIPTOR_RANGE_TYPE_UAV, 1, 0, 0, 1}};
  D3D12_ROOT_PARAMETER table{};
  table.ParameterType = D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE;
  table.DescriptorTable = {2, ranges};
  D3D12_ROOT_SIGNATURE_DESC rd{1, &table, 0, nullptr,
                               D3D12_ROOT_SIGNATURE_FLAG_NONE};
  ComPtr<ID3DBlob> blob;
  check(D3D12SerializeRootSignature(&rd, D3D_ROOT_SIGNATURE_VERSION_1, &blob,
                                    nullptr),
        "serialize root");
  ComPtr<ID3D12RootSignature> root;
  check(guest->CreateRootSignature(0, blob->GetBufferPointer(),
                                   blob->GetBufferSize(), IID_PPV_ARGS(&root)),
        "root");
  std::ifstream in("film-cbv.cso", std::ios::binary);
  std::vector<char> shader{std::istreambuf_iterator<char>{in},
                           std::istreambuf_iterator<char>{}};
  D3D12_COMPUTE_PIPELINE_STATE_DESC pd{};
  pd.pRootSignature = root.Get();
  pd.CS = {shader.data(), shader.size()};
  ComPtr<ID3D12PipelineState> pso;
  using Pipeline = HRESULT(WINAPI *)(ID3D12Device *, const D3D12_COMPUTE_PIPELINE_STATE_DESC *,
                                     UINT, const void *, REFIID, void **);
  check(frame_abi::method<Pipeline>(guest.Get(), 62)(guest.Get(), &pd, 0, nullptr, IID_PPV_ARGS(&pso)),
        "guest compute pipeline");
  auto cpuHeap = descriptorHeap(guest.Get(), false),
       gpuHeap = descriptorHeap(guest.Get(), true);
  using Handle = UINT64(WINAPI *)(ID3D12DescriptorHeap *);
  auto start = [&](ID3D12DescriptorHeap *h, UINT slot) {
    return frame_abi::method<Handle>(h, slot)(h);
  };
  auto cpu = start(cpuHeap.Get(), 9), gpuCpu = start(gpuHeap.Get(), 9),
       gpu = start(gpuHeap.Get(), 10);
  auto type = D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV;
  UINT step = guest->GetDescriptorHandleIncrementSize(type);
  auto cb = buffer(guest.Get(), variants * region, D3D12_HEAP_TYPE_UPLOAD,
                   D3D12_RESOURCE_STATE_GENERIC_READ);
  auto output = buffer(guest.Get(), variants * view, D3D12_HEAP_TYPE_DEFAULT,
                       D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                       D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
  auto read = buffer(guest.Get(), variants * view, D3D12_HEAP_TYPE_READBACK,
                     D3D12_RESOURCE_STATE_COPY_DEST);
  std::array<UINT, variants * region / sizeof(UINT)> values{};
  values.fill(0xcade0123);
  upload(cb.Get(), values.data(), sizeof(values));
  for (UINT i = 0; i < variants; ++i) {
    UINT off = i * increment;
    auto gpuva = cb->GetGPUVirtualAddress() + region * i + off;
    if ((gpuva & 255) != off)
      throw std::runtime_error("unexpected base alignment");
    D3D12_CONSTANT_BUFFER_VIEW_DESC cbv{gpuva, view};
    guest->CreateConstantBufferView(&cbv, {cpu + step * 2 * i});
    D3D12_UNORDERED_ACCESS_VIEW_DESC uav{};
    uav.ViewDimension = D3D12_UAV_DIMENSION_BUFFER;
    uav.Format = DXGI_FORMAT_R32_UINT;
    uav.Buffer.FirstElement = words * i;
    uav.Buffer.NumElements = words;
    guest->CreateUnorderedAccessView(output.Get(), nullptr, &uav,
                                     {cpu + step * (2 * i + 1)});
    D3D12_CPU_DESCRIPTOR_HANDLE to{gpuCpu + step * i * 2},
        from{cpu + step * i * 2};
    UINT two = 2;
    guest->CopyDescriptors(1, &to, &two, 1, &from, &two, type);
  }
  ComPtr<ID3D12CommandQueue> queue;
  ComPtr<ID3D12CommandAllocator> allocator;
  ComPtr<ID3D12GraphicsCommandList> list;
  D3D12_COMMAND_QUEUE_DESC q{};
  check(guest->CreateCommandQueue(&q, IID_PPV_ARGS(&queue)), "queue");
  check(guest->CreateCommandAllocator(q.Type, IID_PPV_ARGS(&allocator)),
        "allocator");
  const UINT list_desc[] = {D3D12_COMMAND_LIST_TYPE_DIRECT, 0, 1, 0};
  check(frame_abi::method<frame_abi::CreateList>(guest.Get(), 73)(
            guest.Get(), list_desc, allocator.Get(), nullptr, IID_PPV_ARGS(&list)),
        "guest command list");
  check(allocator->Reset(), "reset allocator");
  check(list->Reset(allocator.Get(), pso.Get()), "reset command list");
  using BindHeaps = void(WINAPI *)(ID3D12GraphicsCommandList *, UINT, ID3D12DescriptorHeap *const *);
  using BindRoot = void(WINAPI *)(ID3D12GraphicsCommandList *, ID3D12RootSignature *);
  using BindTable = void(WINAPI *)(ID3D12GraphicsCommandList *, UINT, D3D12_GPU_DESCRIPTOR_HANDLE);
  frame_abi::method<BindHeaps>(list.Get(), 32)(list.Get(), 1, gpuHeap.GetAddressOf());
  frame_abi::method<BindRoot>(list.Get(), 33)(list.Get(), root.Get());
  auto bind_table = frame_abi::method<BindTable>(list.Get(), 35);
  for (UINT i = 0; i < variants; ++i) {
    bind_table(list.Get(), 0, {gpu + step * i * 2});
    list->Dispatch(1, 1, 1);
  }
  transition(list.Get(), output.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
             D3D12_RESOURCE_STATE_COPY_SOURCE);
  list->CopyBufferRegion(read.Get(), 0, output.Get(), 0, variants * view);
  // Write after descriptors were created/copied and all dispatches recorded.
  for (UINT i = 0; i < variants; ++i) {
    UINT first = i * (region + increment) / 4;
    for (UINT j = 0; j < words; ++j)
      values[first + j] = 0x13570000u + i * 257 + j;
    values[first + 4 * 37] = values[first + 4 * 37 + 1] = 0;
    values[first + 4 * 38 + 2] = bits(1.f / 2976);
    values[first + 4 * 38 + 3] = bits(1.f / 1240);
  }
  upload(cb.Get(), values.data(), sizeof(values));
  submit_and_wait(guest.Get(), queue.Get(), list.Get());
  void *data{};
  D3D12_RANGE range{0, variants * view}, none{};
  check(read->Map(0, &range, &data), "readback");
  auto result = static_cast<const UINT *>(data);
  UINT total = 0;
  for (UINT i = 0; i < variants; ++i) {
    UINT matches = 0;
    for (UINT j = 0; j < words; ++j)
      matches += result[i * words + j] == values[i * (region + increment) / 4 + j];
    total += matches;
    auto b = result + i * words;
    std::fprintf(f,
                 "offset_%u_words=%u\noffset_%u_fields=%08x,%08x,%08x,%08x\n",
                 i * increment, matches, i * increment, b[37 * 4], b[37 * 4 + 1],
                 b[38 * 4 + 2], b[38 * 4 + 3]);
  }
  read->Unmap(0, &none);
  if (total != variants * words)
    throw std::runtime_error("CBV data mismatch");
}
int main() {
  FILE *f = nullptr;
  if (fopen_s(&f, "result.txt", "wb"))
    return 2;
  int code = 0;
  try {
    run(f);
  } catch (const std::exception &e) {
    std::fprintf(f, "error=%s\n", e.what());
    code = 1;
  }
  return fclose(f) ? 2 : code;
}
