// anyxboxx_gpu_run.exe <shader.dxil> <mémoire.bin> [groupes]
// anyxboxx_gpu_run.exe --draw <vs.dxil> <ps.dxil|-> <mémoire.bin> <sommets> <largeur> <hauteur>
// anyxboxx_gpu_run.exe --adaptateur
//
// Exécute sur le GPU du PC un programme produit par le recompilateur RDNA 2 → HLSL, avec un seul
// buffer brut en u0 (visible de tous les étages) : la mémoire du programme. Le fichier mémoire est
// envoyé au GPU, le programme tourne, puis le fichier est réécrit avec le résultat.
// - Calcul : Dispatch(groupes, 1, 1) d'un shader de calcul (emu/rdna2_hlsl.py).
// - Dessin : DrawInstanced(sommets) d'une liste de triangles (emu/rdna2_hlsl_graphics.py), sans
//   input layout (le VS lit SV_VertexID), cible de rendu R32G32B32A32_FLOAT si un PS est donné.
// Sans GPU D3D12 (ou avec ANYXBOXX_GPU=warp), le rendu logiciel WARP de Windows est utilisé.
// ANYXBOXX_GPU=<partie du nom> : l'adaptateur matériel dont le nom la contient (aucun : échec, jamais WARP).
// --adaptateur décrit l'adaptateur choisi (adapter.cpp).
#include <windows.h>
#include <d3d12.h>
#include <dxgi1_6.h>
#include <wrl/client.h>

#include <climits>
#include <cstdio>
#include <cstring>
#include <vector>

#include "adapter.h"

using Microsoft::WRL::ComPtr;

namespace {

constexpr UINT64 kAlignment = 4;  // RWByteAddressBuffer : taille multiple de 4 octets
constexpr DXGI_FORMAT kTargetFormat = DXGI_FORMAT_R32G32B32A32_FLOAT;

int Fail(const char* what, HRESULT result = E_FAIL) {
  std::fprintf(stderr, "[gpu] %s (0x%08lx)\n", what, static_cast<unsigned long>(result));
  return 1;
}

bool ReadAll(const wchar_t* path, std::vector<char>& out) {
  FILE* file = nullptr;
  if (_wfopen_s(&file, path, L"rb") != 0 || file == nullptr) return false;
  char chunk[65536];
  for (size_t read; (read = std::fread(chunk, 1, sizeof chunk, file)) > 0;) out.insert(out.end(), chunk, chunk + read);
  std::fclose(file);
  return true;
}

bool WriteAll(const wchar_t* path, const char* data, size_t size) {
  FILE* file = nullptr;
  if (_wfopen_s(&file, path, L"wb") != 0 || file == nullptr) return false;
  const bool ok = std::fwrite(data, 1, size, file) == size;
  std::fclose(file);
  return ok;
}

bool WarpRequested() {
  wchar_t value[8] = {};
  return GetEnvironmentVariableW(L"ANYXBOXX_GPU", value, 8) > 0 && std::wcscmp(value, L"warp") == 0;
}

ComPtr<ID3D12Device> CreateDevice(const char** kind) {
  std::wstring filter;
  if (NamedAdapterRequested(filter)) {
    *kind = "matériel";
    return CreateNamedDevice(filter);
  }
  ComPtr<ID3D12Device> device;
  if (!WarpRequested() && SUCCEEDED(D3D12CreateDevice(nullptr, D3D_FEATURE_LEVEL_11_0, IID_PPV_ARGS(&device)))) {
    *kind = "matériel";
    return device;
  }
  ComPtr<IDXGIFactory4> factory;
  ComPtr<IDXGIAdapter> warp;
  if (SUCCEEDED(CreateDXGIFactory2(0, IID_PPV_ARGS(&factory))) && SUCCEEDED(factory->EnumWarpAdapter(IID_PPV_ARGS(&warp)))) {
    D3D12CreateDevice(warp.Get(), D3D_FEATURE_LEVEL_11_0, IID_PPV_ARGS(&device));
  }
  *kind = "WARP";
  return device;
}

ComPtr<ID3D12Resource> Buffer(ID3D12Device* device, D3D12_HEAP_TYPE type, UINT64 size, D3D12_RESOURCE_FLAGS flags,
                              D3D12_RESOURCE_STATES state) {
  D3D12_HEAP_PROPERTIES heap = {};
  heap.Type = type;
  D3D12_RESOURCE_DESC desc = {};
  desc.Dimension = D3D12_RESOURCE_DIMENSION_BUFFER;
  desc.Width = size;
  desc.Height = 1;
  desc.DepthOrArraySize = 1;
  desc.MipLevels = 1;
  desc.SampleDesc.Count = 1;
  desc.Layout = D3D12_TEXTURE_LAYOUT_ROW_MAJOR;
  desc.Flags = flags;
  ComPtr<ID3D12Resource> resource;
  device->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, state, nullptr, IID_PPV_ARGS(&resource));
  return resource;
}

ComPtr<ID3D12Resource> RenderTarget(ID3D12Device* device, UINT width, UINT height) {
  D3D12_HEAP_PROPERTIES heap = {};
  heap.Type = D3D12_HEAP_TYPE_DEFAULT;
  D3D12_RESOURCE_DESC desc = {};
  desc.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
  desc.Width = width;
  desc.Height = height;
  desc.DepthOrArraySize = 1;
  desc.MipLevels = 1;
  desc.Format = kTargetFormat;
  desc.SampleDesc.Count = 1;
  desc.Flags = D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET;
  D3D12_CLEAR_VALUE clear = {};
  clear.Format = kTargetFormat;
  ComPtr<ID3D12Resource> resource;
  device->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_RENDER_TARGET, &clear,
                                  IID_PPV_ARGS(&resource));
  return resource;
}

void Transition(ID3D12GraphicsCommandList* list, ID3D12Resource* resource, D3D12_RESOURCE_STATES before,
                D3D12_RESOURCE_STATES after) {
  D3D12_RESOURCE_BARRIER barrier = {};
  barrier.Type = D3D12_RESOURCE_BARRIER_TYPE_TRANSITION;
  barrier.Transition.pResource = resource;
  barrier.Transition.Subresource = D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES;
  barrier.Transition.StateBefore = before;
  barrier.Transition.StateAfter = after;
  list->ResourceBarrier(1, &barrier);
}

ComPtr<ID3D12RootSignature> RootSignature(ID3D12Device* device) {
  D3D12_ROOT_PARAMETER parameter = {};
  parameter.ParameterType = D3D12_ROOT_PARAMETER_TYPE_UAV;  // u0 : la mémoire, buffer brut
  parameter.ShaderVisibility = D3D12_SHADER_VISIBILITY_ALL;
  D3D12_ROOT_SIGNATURE_DESC desc = {1, &parameter, 0, nullptr, D3D12_ROOT_SIGNATURE_FLAG_NONE};
  ComPtr<ID3DBlob> blob;
  ComPtr<ID3DBlob> error;
  ComPtr<ID3D12RootSignature> root;
  if (FAILED(D3D12SerializeRootSignature(&desc, D3D_ROOT_SIGNATURE_VERSION_1, &blob, &error)) ||
      FAILED(device->CreateRootSignature(0, blob->GetBufferPointer(), blob->GetBufferSize(), IID_PPV_ARGS(&root)))) {
    return nullptr;
  }
  return root;
}

ComPtr<ID3D12PipelineState> ComputePipeline(ID3D12Device* device, ID3D12RootSignature* root,
                                            const std::vector<char>& shader) {
  D3D12_COMPUTE_PIPELINE_STATE_DESC pipeline = {};
  pipeline.pRootSignature = root;
  pipeline.CS = {shader.data(), shader.size()};
  ComPtr<ID3D12PipelineState> state;
  device->CreateComputePipelineState(&pipeline, IID_PPV_ARGS(&state));
  return state;
}

ComPtr<ID3D12PipelineState> GraphicsPipeline(ID3D12Device* device, ID3D12RootSignature* root,
                                             const std::vector<char>& vs, const std::vector<char>& ps) {
  D3D12_GRAPHICS_PIPELINE_STATE_DESC pipeline = {};
  pipeline.pRootSignature = root;
  pipeline.VS = {vs.data(), vs.size()};
  if (!ps.empty()) pipeline.PS = {ps.data(), ps.size()};
  pipeline.BlendState.RenderTarget[0].RenderTargetWriteMask = D3D12_COLOR_WRITE_ENABLE_ALL;
  pipeline.SampleMask = UINT_MAX;
  pipeline.RasterizerState.FillMode = D3D12_FILL_MODE_SOLID;
  pipeline.RasterizerState.CullMode = D3D12_CULL_MODE_NONE;
  pipeline.RasterizerState.DepthClipEnable = TRUE;
  pipeline.PrimitiveTopologyType = D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE;
  pipeline.NumRenderTargets = ps.empty() ? 0 : 1;
  pipeline.RTVFormats[0] = ps.empty() ? DXGI_FORMAT_UNKNOWN : kTargetFormat;
  pipeline.SampleDesc.Count = 1;
  ComPtr<ID3D12PipelineState> state;
  device->CreateGraphicsPipelineState(&pipeline, IID_PPV_ARGS(&state));
  return state;
}

struct Draw {
  UINT vertices = 0;
  UINT width = 0;
  UINT height = 0;
  ID3D12Resource* target = nullptr;
  D3D12_CPU_DESCRIPTOR_HANDLE view = {};
};

// Envoie la mémoire, exécute le calcul ou le dessin, relit la mémoire dans `memory`.
int Execute(ID3D12Device* device, ID3D12RootSignature* root, ID3D12PipelineState* pipeline, std::vector<char>& memory,
            UINT groups, const Draw* draw) {
  const UINT64 size = memory.size();
  ComPtr<ID3D12Resource> gpu = Buffer(device, D3D12_HEAP_TYPE_DEFAULT, size, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
                                      D3D12_RESOURCE_STATE_COPY_DEST);
  ComPtr<ID3D12Resource> upload = Buffer(device, D3D12_HEAP_TYPE_UPLOAD, size, D3D12_RESOURCE_FLAG_NONE,
                                         D3D12_RESOURCE_STATE_GENERIC_READ);
  ComPtr<ID3D12Resource> readback = Buffer(device, D3D12_HEAP_TYPE_READBACK, size, D3D12_RESOURCE_FLAG_NONE,
                                           D3D12_RESOURCE_STATE_COPY_DEST);
  if (!gpu || !upload || !readback) return Fail("allocation des buffers impossible");
  void* mapped = nullptr;
  if (FAILED(upload->Map(0, nullptr, &mapped))) return Fail("Map du buffer d'envoi");
  std::memcpy(mapped, memory.data(), memory.size());
  upload->Unmap(0, nullptr);

  D3D12_COMMAND_QUEUE_DESC queue_desc = {};
  queue_desc.Type = D3D12_COMMAND_LIST_TYPE_DIRECT;
  ComPtr<ID3D12CommandQueue> queue;
  ComPtr<ID3D12CommandAllocator> allocator;
  ComPtr<ID3D12GraphicsCommandList> list;
  ComPtr<ID3D12Fence> fence;
  if (FAILED(device->CreateCommandQueue(&queue_desc, IID_PPV_ARGS(&queue))) ||
      FAILED(device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&allocator))) ||
      FAILED(device->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, allocator.Get(), pipeline,
                                       IID_PPV_ARGS(&list))) ||
      FAILED(device->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&fence)))) {
    return Fail("création de la file de commandes");
  }
  list->CopyBufferRegion(gpu.Get(), 0, upload.Get(), 0, size);
  Transition(list.Get(), gpu.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
  if (draw == nullptr) {
    list->SetComputeRootSignature(root);
    list->SetComputeRootUnorderedAccessView(0, gpu->GetGPUVirtualAddress());
    list->Dispatch(groups, 1, 1);
  } else {
    list->SetGraphicsRootSignature(root);
    list->SetGraphicsRootUnorderedAccessView(0, gpu->GetGPUVirtualAddress());
    const D3D12_VIEWPORT viewport = {0.0f, 0.0f, static_cast<float>(draw->width), static_cast<float>(draw->height),
                                     0.0f, 1.0f};
    const D3D12_RECT scissor = {0, 0, static_cast<LONG>(draw->width), static_cast<LONG>(draw->height)};
    list->RSSetViewports(1, &viewport);
    list->RSSetScissorRects(1, &scissor);
    if (draw->target != nullptr) {
      const float black[4] = {0.0f, 0.0f, 0.0f, 0.0f};
      list->OMSetRenderTargets(1, &draw->view, FALSE, nullptr);
      list->ClearRenderTargetView(draw->view, black, 0, nullptr);
    }
    list->IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
    list->DrawInstanced(draw->vertices, 1, 0, 0);
  }
  Transition(list.Get(), gpu.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
  list->CopyBufferRegion(readback.Get(), 0, gpu.Get(), 0, size);
  if (FAILED(list->Close())) return Fail("fermeture de la liste de commandes");
  ID3D12CommandList* lists[] = {list.Get()};
  queue->ExecuteCommandLists(1, lists);
  HANDLE done = CreateEventW(nullptr, FALSE, FALSE, nullptr);
  queue->Signal(fence.Get(), 1);
  fence->SetEventOnCompletion(1, done);
  WaitForSingleObject(done, INFINITE);
  CloseHandle(done);
  if (FAILED(device->GetDeviceRemovedReason())) return Fail("périphérique perdu", device->GetDeviceRemovedReason());
  if (FAILED(readback->Map(0, nullptr, &mapped))) return Fail("Map du buffer de lecture");
  std::memcpy(memory.data(), mapped, memory.size());
  readback->Unmap(0, nullptr);
  return 0;
}

bool LoadMemory(const wchar_t* path, std::vector<char>& memory, size_t& original) {
  if (!ReadAll(path, memory)) return false;
  original = memory.size();
  memory.resize((memory.size() + kAlignment - 1) / kAlignment * kAlignment + kAlignment);
  return true;
}

int RunCompute(int argc, wchar_t** argv) {
  std::vector<char> shader;
  std::vector<char> memory;
  size_t original = 0;
  if (!ReadAll(argv[1], shader) || !LoadMemory(argv[2], memory, original)) return Fail("lecture des fichiers impossible");
  const UINT groups = argc > 3 ? static_cast<UINT>(_wtoi(argv[3])) : 1;
  const char* kind = "";
  ComPtr<ID3D12Device> device = CreateDevice(&kind);
  if (!device) return Fail("aucun périphérique D3D12");
  ComPtr<ID3D12RootSignature> root = RootSignature(device.Get());
  ComPtr<ID3D12PipelineState> pipeline = root ? ComputePipeline(device.Get(), root.Get(), shader) : nullptr;
  if (!pipeline) return Fail("shader DXIL refusé (signature ou format)");
  if (Execute(device.Get(), root.Get(), pipeline.Get(), memory, groups, nullptr) != 0) return 1;
  if (!WriteAll(argv[2], memory.data(), original)) return Fail("écriture du résultat impossible");
  std::printf("[gpu] %s : %u groupe(s) exécuté(s)\n", kind, groups);
  return 0;
}

int RunDraw(int argc, wchar_t** argv) {
  if (argc < 8) return Fail("--draw <vs.dxil> <ps.dxil|-> <mémoire.bin> <sommets> <largeur> <hauteur>");
  std::vector<char> vs;
  std::vector<char> ps;
  std::vector<char> memory;
  size_t original = 0;
  const bool has_pixel_shader = std::wcscmp(argv[3], L"-") != 0;
  if (!ReadAll(argv[2], vs) || (has_pixel_shader && !ReadAll(argv[3], ps)) || !LoadMemory(argv[4], memory, original)) {
    return Fail("lecture des fichiers impossible");
  }
  Draw draw;
  draw.vertices = static_cast<UINT>(_wtoi(argv[5]));
  draw.width = static_cast<UINT>(_wtoi(argv[6]));
  draw.height = static_cast<UINT>(_wtoi(argv[7]));
  if (draw.vertices == 0 || draw.width == 0 || draw.height == 0) return Fail("sommets, largeur et hauteur > 0");
  const char* kind = "";
  ComPtr<ID3D12Device> device = CreateDevice(&kind);
  if (!device) return Fail("aucun périphérique D3D12");
  ComPtr<ID3D12RootSignature> root = RootSignature(device.Get());
  ComPtr<ID3D12PipelineState> pipeline = root ? GraphicsPipeline(device.Get(), root.Get(), vs, ps) : nullptr;
  if (!pipeline) return Fail("VS/PS DXIL refusés (signature ou format)");
  ComPtr<ID3D12Resource> target;
  ComPtr<ID3D12DescriptorHeap> heap;
  if (has_pixel_shader) {
    D3D12_DESCRIPTOR_HEAP_DESC heap_desc = {D3D12_DESCRIPTOR_HEAP_TYPE_RTV, 1, D3D12_DESCRIPTOR_HEAP_FLAG_NONE, 0};
    target = RenderTarget(device.Get(), draw.width, draw.height);
    if (!target || FAILED(device->CreateDescriptorHeap(&heap_desc, IID_PPV_ARGS(&heap)))) {
      return Fail("création de la cible de rendu");
    }
    draw.target = target.Get();
    draw.view = heap->GetCPUDescriptorHandleForHeapStart();
    device->CreateRenderTargetView(target.Get(), nullptr, draw.view);
  }
  if (Execute(device.Get(), root.Get(), pipeline.Get(), memory, 0, &draw) != 0) return 1;
  if (!WriteAll(argv[4], memory.data(), original)) return Fail("écriture du résultat impossible");
  std::printf("[gpu] %s : %u sommet(s) dessiné(s) sur %ux%u\n", kind, draw.vertices, draw.width, draw.height);
  return 0;
}

}  // namespace

int wmain(int argc, wchar_t** argv) {
  if (argc >= 2 && std::wcscmp(argv[1], L"--adaptateur") == 0) return PrintAdapter();
  if (argc >= 2 && std::wcscmp(argv[1], L"--draw") == 0) return RunDraw(argc, argv);
  if (argc < 3) {
    std::fwprintf(stderr, L"Usage : anyxboxx_gpu_run.exe <shader.dxil> <mémoire.bin> [groupes]\n"
                          L"        anyxboxx_gpu_run.exe --draw <vs.dxil> <ps.dxil|-> <mémoire.bin> <sommets> "
                          L"<largeur> <hauteur>\n"
                          L"        anyxboxx_gpu_run.exe --adaptateur\n");
    return 2;
  }
  return RunCompute(argc, argv);
}
