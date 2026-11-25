#include "../compute_buffers.h"
#include <algorithm>
#include <cmath>
#include <fstream>
#include <iterator>
#include <set>
#include <vector>
using namespace compute_buffers;
using Color = std::array<float, 4>;
constexpr UINT width = 16, height = 16, pixels = width * height,
               texBytes = pixels * sizeof(Color), words = 16,
               listBytes = words * sizeof(UINT);
constexpr DXGI_FORMAT format = DXGI_FORMAT_R32G32B32A32_FLOAT;
static unsigned material(UINT x, UINT y) {
  if ((x == 2 && y == 6) || (x == 12 && y == 3))
    return 5;
  if ((x == 13 && y == 14) || (x == 14 && y == 14))
    return 9;
  return 3;
}
static Color color(UINT x, UINT y) {
  return {float(x * x + y), float(2 * y * y + x), float((x ^ y) * 3), 0};
}
static float bitsFloat(UINT u) {
  float v;
  std::memcpy(&v, &u, 4);
  return v;
}
static UINT floatBits(float f) {
  UINT v;
  std::memcpy(&v, &f, 4);
  return v;
}
static float frac(float a) { return a - std::floor(a); }
static Color expected(UINT x, UINT y) {
  if (x >= 15 || y >= 15)
    return {-19, -19, -19, -19};
  if (material(x, y) == 3)
    return {};
  auto c = color(x, y);
  if (((x + y) & 1) == 0) {
    auto left = color(x ? x - 1 : 0, y),
         right = color(std::min(x + 1, width - 1), y);
    for (size_t k = 0; k < 3; ++k)
      c[k] = (left[k] + right[k]) * .5f;
  }
  const float jitter = frac(frac(bitsFloat(0x3bbf4590) * float(y) +
                                 bitsFloat(0x3d897143) * float(x)) *
                            bitsFloat(0x4253ee82));
  const float z = 1.0f + float(x + y) * .25f;
  c[3] = z + z * (1.0f / 1024.0f) * jitter;
  return c;
}
static ComPtr<ID3D12Resource> texture(ID3D12Device *d, bool writable) {
  D3D12_HEAP_PROPERTIES hp{};
  hp.Type = D3D12_HEAP_TYPE_DEFAULT;
  hp.CreationNodeMask = hp.VisibleNodeMask = 1;
  D3D12_RESOURCE_DESC desc{};
  desc.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
  desc.Width = width;
  desc.Height = height;
  desc.MipLevels = desc.DepthOrArraySize = desc.SampleDesc.Count = 1;
  desc.Format = format;
  desc.Flags = writable ? D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS
                        : D3D12_RESOURCE_FLAG_NONE;
  ComPtr<ID3D12Resource> tex;
  check(d->CreateCommittedResource(&hp, D3D12_HEAP_FLAG_NONE, &desc,
                                   D3D12_RESOURCE_STATE_COPY_DEST, nullptr,
                                   IID_PPV_ARGS(&tex)),
        "texture");
  return tex;
}
static ComPtr<ID3D12DescriptorHeap>
heap(ID3D12Device *d, D3D12_DESCRIPTOR_HEAP_TYPE type, UINT count) {
  D3D12_DESCRIPTOR_HEAP_DESC desc{type, count,
                                  D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, 0};
  ComPtr<ID3D12DescriptorHeap> h;
  check(d->CreateDescriptorHeap(&desc, IID_PPV_ARGS(&h)), "heap");
  return h;
}
static D3D12_TEXTURE_COPY_LOCATION imageLocation(ID3D12Resource *tex) {
  D3D12_TEXTURE_COPY_LOCATION l{};
  l.pResource = tex;
  l.Type = D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
  return l;
}
static D3D12_TEXTURE_COPY_LOCATION uploadLocation(ID3D12Resource *b,
                                                  UINT offset) {
  D3D12_TEXTURE_COPY_LOCATION l{};
  l.pResource = b;
  l.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
  l.PlacedFootprint.Offset = offset;
  l.PlacedFootprint.Footprint = {format, width, height, 1,
                                 width * sizeof(Color)};
  return l;
}
static void run(FILE *f, UINT capacity, bool depth_only, bool second) {
  auto module = LoadLibraryW(L"d3d12_xs.dll");
  using Create = HRESULT(WINAPI *)(IUnknown *, const void *, REFIID, void **);
  auto make = module ? reinterpret_cast<Create>(
                           GetProcAddress(module, "D3D12XboxCreateDevice"))
                     : nullptr;
  if (!make)
    throw std::runtime_error("device factory");
  std::array<UINT, 18> profile{0x20004, 0, ~0u, 0, 0, 0, ~0u, ~0u, 0, 1};
  ComPtr<ID3D12Device> xbox;
  check(make(nullptr, profile.data(), IID_PPV_ARGS(&xbox)), "Xbox device");
  std::ifstream in("retail.dxbc", std::ios::binary);
  std::vector<char> code{std::istreambuf_iterator<char>{in},
                         std::istreambuf_iterator<char>{}};
  ComPtr<ID3D12RootSignature> root;
  check(xbox->CreateRootSignature(0, code.data(), code.size(),
                                  IID_PPV_ARGS(&root)),
        "cooked root");
  D3D12_COMPUTE_PIPELINE_STATE_DESC pd{};
  pd.pRootSignature = root.Get();
  pd.CS = {code.data(), code.size()};
  using Pso = HRESULT(WINAPI *)(ID3D12Device *,
                                const D3D12_COMPUTE_PIPELINE_STATE_DESC *, UINT,
                                const void *, REFIID, void **);
  auto create = frame_abi::method<Pso>(xbox.Get(), 62);
  ComPtr<ID3D12PipelineState> pso;
  check(create(xbox.Get(), &pd, 0, nullptr, IID_PPV_ARGS(&pso)),
        "Xbox compute slot 62");
  ComPtr<ID3D12Device> dev;
  check(root->GetDevice(IID_PPV_ARGS(&dev)), "native device");
  auto handles = heap(dev.Get(), D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 96),
       samplers = heap(dev.Get(), D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER, 16);
  auto stride = dev->GetDescriptorHandleIncrementSize(
      D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV);
  using GpuHandle = UINT64(WINAPI *)(ID3D12DescriptorHeap *);
  const auto cpuStart =
      frame_abi::method<frame_abi::CpuHandle>(handles.Get(), 9)(handles.Get());
  const auto gpuStart =
      frame_abi::method<GpuHandle>(handles.Get(), 10)(handles.Get());
  const auto samplerCpu = frame_abi::method<frame_abi::CpuHandle>(
      samplers.Get(), 9)(samplers.Get());
  const auto samplerGpu =
      frame_abi::method<GpuHandle>(samplers.Get(), 10)(samplers.Get());
  auto cpu = [&](UINT i) {
    return D3D12_CPU_DESCRIPTOR_HANDLE{cpuStart + i * stride};
  };
  auto gpu = [&](UINT i) {
    return D3D12_GPU_DESCRIPTOR_HANDLE{gpuStart + i * stride};
  };
  std::array<ComPtr<ID3D12Resource>, 4> tex;
  auto extra = texture(dev.Get(), false);
  dev->CreateShaderResourceView(extra.Get(), nullptr, cpu(3));
  std::array<ComPtr<ID3D12Resource>, 3> cb;
  std::vector<Color> texels(5 * pixels);
  for (UINT y = 0; y < height; ++y)
    for (UINT x = 0; x < width; ++x) {
      auto i = y * width + x;
      texels[i] = {.25f * (x + y), 0, 0, 0};
      texels[pixels + i] = {0, 0, 0, float(material(x, y)) / 255.f};
      texels[2 * pixels + i] = color(x, y);
      texels[3 * pixels + i] = {-19, -19, -19, -19};
      texels[4 * pixels + i] = {0, 0, 0, second ? 0.f : 1.f};
    }
  for (UINT i = 0; i < 4; ++i) {
    tex[i] = texture(dev.Get(), i == 3);
    if (i < 3)
      dev->CreateShaderResourceView(tex[i].Get(), nullptr, cpu(i));
    else
      dev->CreateUnorderedAccessView(tex[i].Get(), nullptr, nullptr, cpu(82));
  }
  for (UINT i = 0; i < 3; ++i) {
    std::vector<UINT> data(i == 1 ? 1024 : 64);
    if (i == 0) {
      data[5 * 4 + 2] = floatBits(1.f / width);
      data[5 * 4 + 3] = floatBits(1.f / height);
      data[7 * 4 + 2] = 15;
      data[7 * 4 + 3] = 15;
    }
    if (i == 1) {
      data[65 * 4] = floatBits(1);
      data[65 * 4 + 3] = floatBits(-1);
      data[136 * 4] = floatBits(width);
      data[136 * 4 + 1] = floatBits(height);
    }
    if (i == 2)
      data[0] = capacity;
    UINT bytes = UINT(data.size()) * 4;
    cb[i] = buffer(dev.Get(), bytes, D3D12_HEAP_TYPE_UPLOAD,
                   D3D12_RESOURCE_STATE_GENERIC_READ);
    upload(cb[i].Get(), data.data(), bytes);
    D3D12_CONSTANT_BUFFER_VIEW_DESC v{cb[i]->GetGPUVirtualAddress(), bytes};
    dev->CreateConstantBufferView(&v, cpu(64 + i));
  }
  D3D12_SAMPLER_DESC sd{};
  sd.Filter = D3D12_FILTER_MIN_MAG_MIP_POINT;
  sd.AddressU = sd.AddressV = sd.AddressW = D3D12_TEXTURE_ADDRESS_MODE_CLAMP;
  sd.MaxAnisotropy = 1;
  sd.ComparisonFunc = D3D12_COMPARISON_FUNC_ALWAYS;
  sd.MaxLOD = D3D12_FLOAT32_MAX;
  for (UINT i = 0; i < 2; ++i)
    dev->CreateSampler(
        &sd, {samplerCpu + i * dev->GetDescriptorHandleIncrementSize(
                                   D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER)});
  std::array<UINT, 2 * words> seed;
  seed.fill(0xabcd9876);
  seed[0] = 1;
  seed[words] = 0;
  auto lists = buffer(dev.Get(), 2 * listBytes, D3D12_HEAP_TYPE_DEFAULT,
                      D3D12_RESOURCE_STATE_COPY_DEST,
                      D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
  for (UINT i = 0; i < 2; ++i) {
    D3D12_UNORDERED_ACCESS_VIEW_DESC v{};
    v.Format = DXGI_FORMAT_R32_UINT;
    v.ViewDimension = D3D12_UAV_DIMENSION_BUFFER;
    v.Buffer.NumElements = words;
    v.Buffer.FirstElement = i * words;
    dev->CreateUnorderedAccessView(lists.Get(), nullptr, &v, cpu(80 + i));
  }
  auto texUp = buffer(dev.Get(), 5 * texBytes, D3D12_HEAP_TYPE_UPLOAD,
                      D3D12_RESOURCE_STATE_GENERIC_READ),
       listUp = buffer(dev.Get(), 2 * listBytes, D3D12_HEAP_TYPE_UPLOAD,
                       D3D12_RESOURCE_STATE_GENERIC_READ),
       read = buffer(dev.Get(), texBytes + 2 * listBytes,
                     D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
  upload(texUp.Get(), texels.data(), 5 * texBytes);
  upload(listUp.Get(), seed.data(), 2 * listBytes);
  ComPtr<ID3D12CommandAllocator> allocator;
  ComPtr<ID3D12CommandQueue> queue;
  ComPtr<ID3D12GraphicsCommandList> list;
  D3D12_COMMAND_QUEUE_DESC q{};
  check(dev->CreateCommandQueue(&q, IID_PPV_ARGS(&queue)), "queue");
  check(dev->CreateCommandAllocator(q.Type, IID_PPV_ARGS(&allocator)),
        "allocator");
  const UINT params[]{static_cast<UINT>(q.Type), 0, 1, 0};
  check(frame_abi::method<frame_abi::CreateList>(dev.Get(), 73)(
            dev.Get(), params, allocator.Get(), pso.Get(), IID_PPV_ARGS(&list)),
        "list");
  check(list->Reset(allocator.Get(), pso.Get()), "reset list");
  for (UINT i = 0; i < 4; ++i) {
    auto src = uploadLocation(texUp.Get(), i * texBytes),
         dst = imageLocation(tex[i].Get());
    frame_abi::method<frame_abi::Copy>(list.Get(), 83)(list.Get(), &dst, 0, 0,
                                                       0, &src, nullptr, 0);
    transition(list.Get(), tex[i].Get(), D3D12_RESOURCE_STATE_COPY_DEST,
               i < 3 ? D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE
                     : D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
  }
  {
    auto src = uploadLocation(texUp.Get(), 4 * texBytes),
         dst = imageLocation(extra.Get());
    frame_abi::method<frame_abi::Copy>(list.Get(), 83)(list.Get(), &dst, 0, 0,
                                                       0, &src, nullptr, 0);
    transition(list.Get(), extra.Get(), D3D12_RESOURCE_STATE_COPY_DEST,
               D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
  }
  list->CopyBufferRegion(lists.Get(), 0, listUp.Get(), 0, 2 * listBytes);
  transition(list.Get(), lists.Get(), D3D12_RESOURCE_STATE_COPY_DEST,
             D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
  ID3D12DescriptorHeap *heaps[] = {handles.Get(), samplers.Get()};
  using Heaps = void(WINAPI *)(ID3D12GraphicsCommandList *, UINT,
                               ID3D12DescriptorHeap *const *);
  using Root =
      void(WINAPI *)(ID3D12GraphicsCommandList *, ID3D12RootSignature *);
  using Table = void(WINAPI *)(ID3D12GraphicsCommandList *, UINT,
                               D3D12_GPU_DESCRIPTOR_HANDLE);
  frame_abi::method<Heaps>(list.Get(), 32)(list.Get(), 2, heaps);
  frame_abi::method<Root>(list.Get(), 33)(list.Get(), root.Get());
  const auto table = frame_abi::method<Table>(list.Get(), 35);
  table(list.Get(), 0, gpu(0));
  table(list.Get(), 1, gpu(64));
  table(list.Get(), 2, D3D12_GPU_DESCRIPTOR_HANDLE{samplerGpu});
  table(list.Get(), 3, gpu(80));
  list->Dispatch(2, 2, 1);
  transition(list.Get(), lists.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
             D3D12_RESOURCE_STATE_COPY_SOURCE);
  transition(list.Get(), tex[3].Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
             D3D12_RESOURCE_STATE_COPY_SOURCE);
  list->CopyBufferRegion(read.Get(), texBytes, lists.Get(), 0, 2 * listBytes);
  auto dst = uploadLocation(read.Get(), 0), src = imageLocation(tex[3].Get());
  frame_abi::method<frame_abi::Copy>(list.Get(), 83)(list.Get(), &dst, 0, 0, 0,
                                                     &src, nullptr, 0);
  submit_and_wait(dev.Get(), queue.Get(), list.Get());
  void *mapped{};
  D3D12_RANGE range{0, texBytes + 2 * listBytes}, none{};
  check(read->Map(0, &range, &mapped), "readback");
  auto values = static_cast<Color *>(mapped);
  UINT checked = 0;
  for (UINT y = 0; y < height; ++y)
    for (UINT x = 0; x < width; ++x) {
      auto c = expected(x, y);
      if (depth_only && x < 15 && y < 15)
        c[0] = c[1] = c[2] = 0;
      for (UINT k = 0; k < 4; ++k) {
        float a = values[y * width + x][k];
        if (std::abs(a - c[k]) <= 0.00001f)
          ++checked;
        else
          std::fprintf(f, "mismatch_at_%u_%u_%u=%.7f expected %.7f\n", x, y, k,
                       a, c[k]);
      }
    }
  auto counters = reinterpret_cast<const UINT *>(values + pixels);
  UINT count = counters[0];
  const UINT other = counters[words];
  const UINT accepted = second          ? std::min(3u, capacity)
                        : capacity == 1 ? 0u
                                        : 3u;
  const UINT offset = second ? words : 0;
  const UINT first = second ? 0 : 1;
  std::set<std::pair<UINT, UINT>> groups;
  for (UINT i = 0; i < accepted; ++i)
    groups.emplace(counters[offset + 2 + (first + i) * 2],
                   counters[offset + 3 + (first + i) * 2]);
  const std::set<std::pair<UINT, UINT>> allowed{{0, 0}, {1, 0}, {1, 1}};
  bool valid = groups.size() == accepted;
  for (auto group : groups)
    if (allowed.find(group) == allowed.end())
      valid = false;
  for (UINT i = 1; i < 2 * words; ++i) {
    if (second && i == words)
      continue;
    if (i >= offset + 2 + first * 2 && i < offset + 2 + (first + accepted) * 2)
      continue;
    if (counters[i] != seed[i])
      valid = false;
  }
  std::fprintf(f,
               "%scapacity_%u_channels=%u\n%scapacity_%u_counter=%u\n%"
               "scapacity_%u_other=%u\n%scapacity_%u_valid_records=%u\n",
               second ? "second_" : "", capacity, checked,
               second ? "second_" : "", capacity, count,
               second ? "second_" : "", capacity, other,
               second ? "second_" : "", capacity, valid);
  read->Unmap(0, &none);
  if (checked != 4 * pixels || count != (second ? 1u : 4u) ||
      other != (second ? 3u : 0u) || !valid)
    throw std::runtime_error("subsurface GPU mismatch");
}
int main() {
  FILE *f = nullptr;
  if (fopen_s(&f, "result.txt", "wb"))
    return 2;
  int status = 0;
  try {
    const bool depth_only =
        GetFileAttributesW(L"depth-only") != INVALID_FILE_ATTRIBUTES;
    run(f, 5, depth_only, false);
    run(f, 1, depth_only, false);
    run(f, 5, depth_only, true);
    run(f, 1, depth_only, true);
  } catch (const std::exception &e) {
    std::fprintf(f, "error=%s\n", e.what());
    status = 1;
  }
  return fclose(f) ? 2 : status;
}
