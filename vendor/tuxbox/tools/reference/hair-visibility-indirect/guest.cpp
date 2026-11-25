#include "../independent-shaders/context.h"
#include <array>
using namespace shader_control;
static std::vector<char> load(const char *n) {
  std::ifstream f(n, std::ios::binary);
  if (!f)
    throw std::runtime_error(n);
  return {std::istreambuf_iterator<char>(f), {}};
}
int main() {
  FILE *f{};
  if (fopen_s(&f, "result.txt", "w"))
    return 2;
  try {
    auto dll = LoadLibraryW(L"d3d12_xs.dll");
    using Create = HRESULT(WINAPI *)(IUnknown *, const void *, REFIID, void **);
    auto create = dll ? reinterpret_cast<Create>(
                            GetProcAddress(dll, "D3D12XboxCreateDevice"))
                      : nullptr;
    if (!create)
      throw std::runtime_error("device factory");
    std::array<UINT, 18> profile{0x20004, 0, ~0u, 0, 0, 0, ~0u, ~0u, 0, 1};
    ComPtr<ID3D12Device> guest;
    check(create(nullptr, profile.data(), IID_PPV_ARGS(&guest)),
          "guest device");
    auto rootbytes = load("retail.dxbc");
    ComPtr<ID3D12RootSignature> root;
    check(guest->CreateRootSignature(0, rootbytes.data(), rootbytes.size(),
                                     IID_PPV_ARGS(&root)),
          "root");
    D3D12_COMPUTE_PIPELINE_STATE_DESC pd{};
    pd.pRootSignature = root.Get();
    pd.CS = {rootbytes.data(), rootbytes.size()};
    using Pso = HRESULT(WINAPI *)(ID3D12Device *,
                                  const D3D12_COMPUTE_PIPELINE_STATE_DESC *,
                                  UINT, const void *, REFIID, void **);
    ComPtr<ID3D12PipelineState> pso;
    check(frame_abi::method<Pso>(guest.Get(), 62)(guest.Get(), &pd, 0, nullptr,
                                                  IID_PPV_ARGS(&pso)),
          "pso");
    ComPtr<ID3D12Device> owner;
    check(root->GetDevice(IID_PPV_ARGS(&owner)), "owner");
    auto dev = owner.Get();
    ComPtr<ID3D12CommandQueue> queue;
    D3D12_COMMAND_QUEUE_DESC q{};
    check(dev->CreateCommandQueue(&q, IID_PPV_ARGS(&queue)), "queue");
    ComPtr<ID3D12CommandAllocator> a;
    ComPtr<ID3D12GraphicsCommandList> l;
    check(dev->CreateCommandAllocator(q.Type, IID_PPV_ARGS(&a)), "alloc");
    const UINT params[]{static_cast<UINT>(q.Type), 0, 1, 0};
    check(frame_abi::method<frame_abi::CreateList>(dev, 73)(
              dev, params, a.Get(), pso.Get(), IID_PPV_ARGS(&l)),
          "list");
    check(l->Reset(a.Get(), pso.Get()), "reset list");
    D3D12_DESCRIPTOR_HEAP_DESC hd{D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 96,
                                  D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, 0};
    ComPtr<ID3D12DescriptorHeap> heap;
    check(dev->CreateDescriptorHeap(&hd, IID_PPV_ARGS(&heap)), "heap");
    UINT step = dev->GetDescriptorHandleIncrementSize(hd.Type);
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
    auto input = [&](const std::vector<char> &v) {
      auto b = buffer(dev, static_cast<UINT>(v.size()), D3D12_HEAP_TYPE_UPLOAD,
                      D3D12_RESOURCE_STATE_GENERIC_READ);
      upload(b.Get(), v.data(), v.size());
      return b;
    };
    auto cb = input(load("cb.bin"));
    D3D12_CONSTANT_BUFFER_VIEW_DESC cv{cb->GetGPUVirtualAddress(), 256};
    dev->CreateConstantBufferView(&cv, cpu(64));
    std::array<ComPtr<ID3D12Resource>, 8> in, staging;
    for (UINT i = 0; i < 6; ++i) {
      const auto bytes =
          load((std::string("t") + std::to_string(i) + ".bin").c_str());
      if (i == 3 || i == 4) {
        in[i] = input(bytes);
        D3D12_SHADER_RESOURCE_VIEW_DESC v{};
        v.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
        v.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
        v.Format = i == 3 ? DXGI_FORMAT_R32_UINT : DXGI_FORMAT_R32G32_UINT;
        v.Buffer.NumElements = 1;
        dev->CreateShaderResourceView(in[i].Get(), &v, cpu(i));
        continue;
      }
      const bool scalar = i != 1;
      const DXGI_FORMAT fmt =
          scalar ? DXGI_FORMAT_R32_FLOAT : DXGI_FORMAT_R32G32_UINT;
      const UINT row = scalar ? 64 : 128;
      std::vector<char> padded(4096);
      for (UINT y = 0; y < 16; y++)
        std::memcpy(padded.data() + y * 256, bytes.data() + y * row, row);
      staging[i] = input(padded);
      D3D12_HEAP_PROPERTIES hp{};
      hp.Type = D3D12_HEAP_TYPE_DEFAULT;
      hp.CreationNodeMask = hp.VisibleNodeMask = 1;
      D3D12_RESOURCE_DESC d{};
      d.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
      d.Width = d.Height = 16;
      d.DepthOrArraySize = d.MipLevels = d.SampleDesc.Count = 1;
      d.Format = fmt;
      check(dev->CreateCommittedResource(&hp, D3D12_HEAP_FLAG_NONE, &d,
                                         D3D12_RESOURCE_STATE_COPY_DEST,
                                         nullptr, IID_PPV_ARGS(&in[i])),
            "input texture");
      D3D12_TEXTURE_COPY_LOCATION dst{}, src{};
      dst.pResource = in[i].Get();
      src.pResource = staging[i].Get();
      src.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
      src.PlacedFootprint.Footprint = {fmt, 16, 16, 1, 256};
      frame_abi::method<frame_abi::Copy>(l.Get(), 83)(l.Get(), &dst, 0, 0, 0,
                                                      &src, nullptr, 0);
      transition(l.Get(), in[i].Get(), D3D12_RESOURCE_STATE_COPY_DEST,
                 D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
      dev->CreateShaderResourceView(in[i].Get(), nullptr, cpu(i));
    }
    for (UINT i = 6; i < 8; i++) {
      in[i] =
          input(load((std::string("t") + std::to_string(i) + ".bin").c_str()));
      D3D12_SHADER_RESOURCE_VIEW_DESC v{};
      v.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
      v.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
      v.Format = i == 6 ? DXGI_FORMAT_R32_UINT : DXGI_FORMAT_R32G32_UINT;
      v.Buffer.NumElements = 1;
      dev->CreateShaderResourceView(in[i].Get(), &v, cpu(i));
    }
    std::array<ComPtr<ID3D12Resource>, 5> out, seed, read;
    for (UINT i = 0; i < 5; i++) {
      auto bytes =
          load((std::string("u") + std::to_string(i) + ".bin").c_str());
      const bool texture = i == 0 || i == 1 || i == 4;
      if (texture) {
        const DXGI_FORMAT fmt =
            i == 4 ? DXGI_FORMAT_R32_FLOAT : DXGI_FORMAT_R32_UINT;
        const UINT side = i == 0 ? 1 : 16;
        std::vector<char> padded(256 * side);
        for (UINT y = 0; y < side; y++)
          std::memcpy(padded.data() + 256 * y, bytes.data() + side * 4 * y,
                      side * 4);
        seed[i] = input(padded);
        read[i] = buffer(dev, UINT(padded.size()), D3D12_HEAP_TYPE_READBACK,
                         D3D12_RESOURCE_STATE_COPY_DEST);
        D3D12_HEAP_PROPERTIES hp{};
        hp.Type = D3D12_HEAP_TYPE_DEFAULT;
        hp.CreationNodeMask = hp.VisibleNodeMask = 1;
        D3D12_RESOURCE_DESC d{};
        d.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
        d.Width = d.Height = side;
        d.DepthOrArraySize = d.MipLevels = d.SampleDesc.Count = 1;
        d.Format = fmt;
        d.Flags = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
        check(dev->CreateCommittedResource(&hp, D3D12_HEAP_FLAG_NONE, &d,
                                           D3D12_RESOURCE_STATE_COPY_DEST,
                                           nullptr, IID_PPV_ARGS(&out[i])),
              "out texture");
        D3D12_TEXTURE_COPY_LOCATION dst{}, src{};
        dst.pResource = out[i].Get();
        src.pResource = seed[i].Get();
        src.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
        src.PlacedFootprint.Footprint = {fmt, side, side, 1, 256};
        frame_abi::method<frame_abi::Copy>(l.Get(), 83)(l.Get(), &dst, 0, 0, 0,
                                                        &src, nullptr, 0);
      } else {
        seed[i] = input(bytes);
        out[i] = buffer(dev, UINT(bytes.size()), D3D12_HEAP_TYPE_DEFAULT,
                        D3D12_RESOURCE_STATE_COPY_DEST,
                        D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
        read[i] = buffer(dev, UINT(bytes.size()), D3D12_HEAP_TYPE_READBACK,
                         D3D12_RESOURCE_STATE_COPY_DEST);
        l->CopyBufferRegion(out[i].Get(), 0, seed[i].Get(), 0, bytes.size());
      }
      transition(l.Get(), out[i].Get(), D3D12_RESOURCE_STATE_COPY_DEST,
                 D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
      if (texture)
        dev->CreateUnorderedAccessView(out[i].Get(), nullptr, nullptr,
                                       cpu(80 + i));
      else {
        D3D12_UNORDERED_ACCESS_VIEW_DESC u{};
        u.ViewDimension = D3D12_UAV_DIMENSION_BUFFER;
        u.Buffer.NumElements = UINT(bytes.size() / 8);
        if (i == 2)
          u.Buffer.StructureByteStride = 8;
        else
          u.Format = DXGI_FORMAT_R32G32_UINT;
        dev->CreateUnorderedAccessView(out[i].Get(), nullptr, &u, cpu(80 + i));
      }
    }
    ID3D12DescriptorHeap *heaps[] = {heap.Get()};
    using Heaps = void(WINAPI *)(ID3D12GraphicsCommandList *, UINT,
                                 ID3D12DescriptorHeap *const *);
    using Root =
        void(WINAPI *)(ID3D12GraphicsCommandList *, ID3D12RootSignature *);
    using Table = void(WINAPI *)(ID3D12GraphicsCommandList *, UINT,
                                 D3D12_GPU_DESCRIPTOR_HANDLE);
    frame_abi::method<Heaps>(l.Get(), 32)(l.Get(), 1, heaps);
    frame_abi::method<Root>(l.Get(), 33)(l.Get(), root.Get());
    const auto table = frame_abi::method<Table>(l.Get(), 35);
    table(l.Get(), 0, gpu(0));
    table(l.Get(), 1, gpu(64));
    table(l.Get(), 3, gpu(80));
    l->Dispatch(1, 1, 1);
    for (UINT i = 0; i < 5; i++) {
      transition(l.Get(), out[i].Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                 D3D12_RESOURCE_STATE_COPY_SOURCE);
      if (i == 0 || i == 1 || i == 4) {
        UINT side = i == 0 ? 1 : 16;
        DXGI_FORMAT fmt = i == 4 ? DXGI_FORMAT_R32_FLOAT : DXGI_FORMAT_R32_UINT;
        D3D12_TEXTURE_COPY_LOCATION src{}, dst{};
        src.pResource = out[i].Get();
        dst.pResource = read[i].Get();
        dst.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
        dst.PlacedFootprint.Footprint = {fmt, side, side, 1, 256};
        frame_abi::method<frame_abi::Copy>(l.Get(), 83)(l.Get(), &dst, 0, 0, 0,
                                                        &src, nullptr, 0);
      } else
        l->CopyBufferRegion(
            read[i].Get(), 0, out[i].Get(), 0,
            load((std::string("u") + std::to_string(i) + ".bin").c_str())
                .size());
    }
    submit_and_wait(dev, queue.Get(), l.Get());
    for (UINT i = 0; i < 5; i++) {
      SIZE_T n =
          (i == 0 ? 256
                  : (i == 1 || i == 4
                         ? 4096
                         : load((std::string("u") + std::to_string(i) + ".bin")
                                    .c_str())
                               .size()));
      write_buffer(read[i].Get(), std::string("out") + std::to_string(i), n);
    }
    std::fprintf(f, "status=ok\n");
  } catch (const std::exception &e) {
    std::fprintf(f, "error=%s\n", e.what());
    std::fclose(f);
    return 1;
  }
  std::fclose(f);
  return 0;
}
