// Guest-side declarations shared by the portable and retail mesh draws.
#pragma once
#include "compute_fixture.h"
#include "frame_stream.h"

namespace mesh_fixture {
using namespace frame_abi;
using namespace compute_fixture;
using DispatchMesh = void(WINAPI*)(ID3D12GraphicsCommandList*, UINT, UINT, UINT);
using BindHeaps = void(WINAPI*)(ID3D12GraphicsCommandList*, UINT, ID3D12DescriptorHeap* const*);
using BindRoot = void(WINAPI*)(ID3D12GraphicsCommandList*, ID3D12RootSignature*);
using SetTable = void(WINAPI*)(ID3D12GraphicsCommandList*, UINT, D3D12_GPU_DESCRIPTOR_HANDLE);
using GpuHandle = UINT64(WINAPI*)(ID3D12DescriptorHeap*);
using CreateStream = HRESULT(WINAPI*)(ID3D12Device2*, const D3D12_PIPELINE_STATE_STREAM_DESC*, REFIID, void**);

struct Context {
    DXGI_ADAPTER_DESC1 adapter{};
    ComPtr<ID3D12Device> device;
    ComPtr<ID3D12Device2> device2;
    ComPtr<ID3D12CommandQueue> queue;
    ComPtr<ID3D12CommandAllocator> allocator;
    ComPtr<ID3D12GraphicsCommandList> list;
    ComPtr<ID3D12DescriptorHeap> views;
    UINT stride;

    Context(const wchar_t* selected, UINT viewCount) {
        constexpr std::array<UINT, 18> mesh_profile{0x20004, 0, ~0u, 0, 0, 0, ~0u, ~0u, 0, 1};
        device = create_device(selected, adapter, mesh_profile.data());
        check(device.As(&device2), "Xbox device2");
        D3D12_COMMAND_QUEUE_DESC q{};
        check(device->CreateCommandQueue(&q, IID_PPV_ARGS(&queue)), "mesh queue");
        check(device->CreateCommandAllocator(q.Type, IID_PPV_ARGS(&allocator)), "mesh allocator");
        const UINT params[]{D3D12_COMMAND_LIST_TYPE_DIRECT, 0, 1, 0};
        check(method<CreateList>(device.Get(), 73)(device.Get(), params, allocator.Get(), nullptr,
                                                   IID_PPV_ARGS(&list)),
              "mesh list");
        D3D12_DESCRIPTOR_HEAP_DESC heap{D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, viewCount,
                                        D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, 1};
        check(device->CreateDescriptorHeap(&heap, IID_PPV_ARGS(&views)), "mesh descriptor heap");
        stride = device->GetDescriptorHandleIncrementSize(heap.Type);
        if (!stride) throw std::runtime_error("zero descriptor increment");
    }
    D3D12_CPU_DESCRIPTOR_HANDLE cpu(UINT index) const {
        return {method<CpuHandle>(views.Get(), 9)(views.Get()) + SIZE_T(index) * stride};
    }
    void table(UINT parameter, UINT index) const {
        D3D12_GPU_DESCRIPTOR_HANDLE handle{method<GpuHandle>(views.Get(), 10)(views.Get()) +
                                           UINT64(index) * stride};
        method<SetTable>(list.Get(), 36)(list.Get(), parameter, handle);
    }
    ComPtr<ID3D12RootSignature> root(const D3D12_ROOT_SIGNATURE_DESC& desc) const {
        ComPtr<ID3DBlob> blob;
        check(D3D12SerializeRootSignature(&desc, D3D_ROOT_SIGNATURE_VERSION_1, &blob, nullptr),
              "serialize mesh root");
        ComPtr<ID3D12RootSignature> result;
        check(
            device->CreateRootSignature(0, blob->GetBufferPointer(), blob->GetBufferSize(), IID_PPV_ARGS(&result)),
            "create mesh root");
        return result;
    }
    HRESULT create_pipeline(MeshPipelineStream& stream, ComPtr<ID3D12PipelineState>& pso) const {
        D3D12_PIPELINE_STATE_STREAM_DESC desc{sizeof(stream), &stream};
        return method<CreateStream>(device2.Get(), 86)(device2.Get(), &desc,
                                                       IID_PPV_ARGS(pso.ReleaseAndGetAddressOf()));
    }
    void bind(ID3D12RootSignature* root, UINT width, UINT height) const {
        method<BindRoot>(list.Get(), 34)(list.Get(), root);
        ID3D12DescriptorHeap* bound[]{views.Get()};
        method<BindHeaps>(list.Get(), 32)(list.Get(), 1, bound);
        const D3D12_VIEWPORT viewport{0, 0, float(width), float(height), 0, 1};
        const D3D12_RECT scissor{0, 0, LONG(width), LONG(height)};
        list->RSSetViewports(1, &viewport);
        list->RSSetScissorRects(1, &scissor);
    }
    void dispatch(UINT x, UINT y, UINT z) const { method<DispatchMesh>(list.Get(), 127)(list.Get(), x, y, z); }
    void submit() const {
        check(list->Close(), "close mesh draw");
        ComPtr<ID3D12Fence> fence;
        check(device->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&fence)), "mesh fence");
        ID3D12CommandList* lists[]{list.Get()};
        queue->ExecuteCommandLists(1, lists);
        check(queue->Signal(fence.Get(), 1), "signal mesh fence");
        wait_for_fence(fence.Get(), 1);
    }
};

inline MeshPipelineStream stream(ID3D12RootSignature* root, D3D12_SHADER_BYTECODE mesh,
                                 D3D12_SHADER_BYTECODE pixel) {
    MeshPipelineStream s;
    s.root.payload = root;
    s.ms.payload = mesh;
    s.ps.payload = pixel;
    s.raster.payload.FillMode = D3D12_FILL_MODE_SOLID;
    s.raster.payload.CullMode = D3D12_CULL_MODE_NONE;
    s.raster.payload.DepthClipEnable = TRUE;
    return s;
}
}
