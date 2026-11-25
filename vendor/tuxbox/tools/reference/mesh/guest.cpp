#include "amplify.h"
#include "frame_stream.h"
#include "mesh.h"
#include "mesh_as.h"
#include "pixel.h"
#include <cstring>
using frame_abi::check;
using frame_abi::ComPtr;

using Stream = frame_abi::MeshPipelineStream;

static ComPtr<ID3D12Resource> buffer(ID3D12Device* d, bool readback) {
    D3D12_HEAP_PROPERTIES heap{};
    heap.Type = readback ? D3D12_HEAP_TYPE_READBACK : D3D12_HEAP_TYPE_DEFAULT;
    heap.CreationNodeMask = heap.VisibleNodeMask = 1;
    D3D12_RESOURCE_DESC desc{};
    desc.Dimension = D3D12_RESOURCE_DIMENSION_BUFFER;
    desc.Width = 8 * 8 * 4;
    desc.Height = desc.DepthOrArraySize = desc.MipLevels = desc.SampleDesc.Count = 1;
    desc.Layout = D3D12_TEXTURE_LAYOUT_ROW_MAJOR;
    if (!readback)
        desc.Flags = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
    ComPtr<ID3D12Resource> result;
    check(d->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc,
                                     readback ? D3D12_RESOURCE_STATE_COPY_DEST
                                              : D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                                     nullptr, IID_PPV_ARGS(&result)),
          "create buffer");
    return result;
}

static void run(bool xbox, bool amplification, FILE* file) {
    ComPtr<ID3D12Device> provider;
    if (xbox) {
        const std::array<UINT, 18> profile{0x20004, 0, ~0u, 0, 0, 0, ~0u, ~0u, 0, 1};
        auto dll = LoadLibraryExW(L"d3d12_xs.dll", nullptr,
                                  LOAD_LIBRARY_SEARCH_APPLICATION_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32);
        using Create = HRESULT(WINAPI*)(IUnknown*, const void*, REFIID, void**);
        auto create = dll ? reinterpret_cast<Create>(GetProcAddress(dll, "D3D12XboxCreateDevice")) : nullptr;
        if (!create)
            throw std::runtime_error("missing Xbox device factory");
        check(create(nullptr, profile.data(), IID_PPV_ARGS(&provider)), "create Xbox device");
    } else {
        check(D3D12CreateDevice(nullptr, D3D_FEATURE_LEVEL_12_0, IID_PPV_ARGS(&provider)), "desktop device");
    }
    D3D12_ROOT_PARAMETER parameter{};
    parameter.ParameterType = D3D12_ROOT_PARAMETER_TYPE_UAV;
    D3D12_ROOT_SIGNATURE_DESC desc{1, &parameter, 0, nullptr, D3D12_ROOT_SIGNATURE_FLAG_NONE};
    ComPtr<ID3DBlob> blob;
    check(D3D12SerializeRootSignature(&desc, D3D_ROOT_SIGNATURE_VERSION_1, &blob, nullptr), "serialize root");
    ComPtr<ID3D12RootSignature> root;
    check(provider->CreateRootSignature(0, blob->GetBufferPointer(), blob->GetBufferSize(), IID_PPV_ARGS(&root)),
          "create root");
    ComPtr<ID3D12Device2> owner;
    check(root->GetDevice(IID_PPV_ARGS(&owner)), "get owning device");
    ComPtr<IDXGIFactory4> factory;
    ComPtr<IDXGIAdapter1> adapter;
    DXGI_ADAPTER_DESC1 gpu{};
    check(CreateDXGIFactory1(IID_PPV_ARGS(&factory)), "DXGI factory");
    check(factory->EnumAdapterByLuid(owner->GetAdapterLuid(), IID_PPV_ARGS(&adapter)), "DXGI adapter");
    check(adapter->GetDesc1(&gpu), "GPU description");
    if (!amplification)
        std::fprintf(file, "adapter=%04x:%04x\n", gpu.VendorId, gpu.DeviceId);
    Stream stream;
    stream.root.payload = root.Get();
    stream.ms.payload = amplification ? D3D12_SHADER_BYTECODE{mesh_as_dxil, sizeof(mesh_as_dxil)}
                                      : D3D12_SHADER_BYTECODE{mesh_dxil, sizeof(mesh_dxil)};
    if (amplification)
        stream.as.payload = {amplify_dxil, sizeof(amplify_dxil)};
    stream.ps.payload = {pixel_dxil, sizeof(pixel_dxil)};
    stream.raster.payload.FillMode = D3D12_FILL_MODE_SOLID;
    stream.raster.payload.CullMode = D3D12_CULL_MODE_NONE;
    stream.raster.payload.DepthClipEnable = TRUE;
    D3D12_PIPELINE_STATE_STREAM_DESC pipelineDesc{xbox ? sizeof(stream) : offsetof(Stream, extension), &stream};
    ComPtr<ID3D12PipelineState> pso;
    if (xbox) {
        using CreatePSO = HRESULT(WINAPI*)(ID3D12Device*, const D3D12_PIPELINE_STATE_STREAM_DESC*, REFIID, void**);
        auto create = frame_abi::method<CreatePSO>(provider.Get(), 86);
        stream.stencil[16] = 1; // Does not match the desktop read mask.
        HRESULT invalid = create(provider.Get(), &pipelineDesc, IID_PPV_ARGS(&pso));
        if (invalid != E_NOTIMPL || pso)
            throw std::runtime_error("mismatched stencil accepted");
        stream.stencil[16] = 0;
        std::swap(stream.ms.payload, stream.ps.payload);
        invalid = create(provider.Get(), &pipelineDesc, IID_PPV_ARGS(&pso));
        if (invalid != E_NOTIMPL || pso)
            throw std::runtime_error("incorrect DXIL stage accepted");
        std::swap(stream.ms.payload, stream.ps.payload);
        check(create(provider.Get(), &pipelineDesc, IID_PPV_ARGS(&pso)), "Xbox slot 86");
    } else {
        stream.ms.tag = D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_MS;
        stream.as.tag = D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_AS;
        check(owner->CreatePipelineState(&pipelineDesc, IID_PPV_ARGS(&pso)), "desktop mesh pipeline");
    }
    ComPtr<ID3D12CommandQueue> queue;
    ComPtr<ID3D12CommandAllocator> allocator;
    ComPtr<ID3D12GraphicsCommandList6> list;
    D3D12_COMMAND_QUEUE_DESC q{};
    check(owner->CreateCommandQueue(&q, IID_PPV_ARGS(&queue)), "queue");
    check(owner->CreateCommandAllocator(q.Type, IID_PPV_ARGS(&allocator)), "allocator");
    check(owner->CreateCommandList(0, q.Type, allocator.Get(), pso.Get(), IID_PPV_ARGS(&list)), "list6");
    auto output = buffer(owner.Get(), false), readback = buffer(owner.Get(), true);
    list->SetGraphicsRootSignature(root.Get());
    list->SetGraphicsRootUnorderedAccessView(0, output->GetGPUVirtualAddress());
    D3D12_VIEWPORT viewport{0, 0, 8, 8, 0, 1};
    D3D12_RECT scissor{0, 0, 8, 8};
    list->RSSetViewports(1, &viewport);
    list->RSSetScissorRects(1, &scissor);
    list->DispatchMesh(1, 1, 1);
    D3D12_RESOURCE_BARRIER barrier{};
    barrier.Transition = {output.Get(), 0, D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                          D3D12_RESOURCE_STATE_COPY_SOURCE};
    list->ResourceBarrier(1, &barrier);
    list->CopyResource(readback.Get(), output.Get());
    check(list->Close(), "close");
    ID3D12CommandList* lists[] = {list.Get()};
    queue->ExecuteCommandLists(1, lists);
    ComPtr<ID3D12Fence> fence;
    check(owner->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&fence)), "fence");
    check(queue->Signal(fence.Get(), 1), "signal");
    HANDLE event = CreateEventW(nullptr, FALSE, FALSE, nullptr);
    if (!event)
        throw std::runtime_error("event");
    HRESULT hr = fence->SetEventOnCompletion(1, event);
    DWORD wait = SUCCEEDED(hr) ? WaitForSingleObject(event, 15000) : WAIT_FAILED;
    CloseHandle(event);
    check(hr, "signal event");
    if (wait != WAIT_OBJECT_0 || fence->GetCompletedValue() != 1)
        throw std::runtime_error("GPU fence");
    UINT expected = amplification ? 0x1c3d5e7f : 0x2468ace0;
    void* data{};
    D3D12_RANGE range{0, 256}, none{};
    check(readback->Map(0, &range, &data), "readback");
    UINT matched = 0;
    for (UINT i = 0; i < 64; ++i)
        matched += static_cast<const UINT*>(data)[i] == expected;
    readback->Unmap(0, &none);
    std::fprintf(file, "%s_matching_pixels=%u\n", amplification ? "as_ms_ps" : "ms_ps", matched);
    if (matched != 64)
        throw std::runtime_error("mesh/pixel readback mismatch");
}

int main(int argc, char** argv) {
    if (argc != 3)
        return 2;
    FILE* file{};
    if (fopen_s(&file, argv[2], "wb"))
        return 2;
    int exit = 0;
    try {
        bool xbox = std::strcmp(argv[1], "xbox") == 0;
        run(xbox, false, file);
        run(xbox, true, file);
    } catch (const std::exception& e) {
        std::fprintf(file, "error=%s\n", e.what());
        exit = 1;
    }
    return fclose(file) ? 2 : exit;
}
