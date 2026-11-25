// Exercise the full root-CBV window, including a page crossing on Scarlett.
#include "compute_fixture.h"
#include "frame_cli.h"
#include "root_cbv.h"
namespace {
using namespace frame_abi;
using namespace compute_fixture;
using Pipeline = HRESULT(WINAPI*)(ID3D12Device*,const D3D12_COMPUTE_PIPELINE_STATE_DESC*,UINT,const void*,REFIID,void**);
using Root = void(WINAPI*)(ID3D12GraphicsCommandList*,ID3D12RootSignature*);
using View = void(WINAPI*)(ID3D12GraphicsCommandList*,UINT,UINT64);
using Signal = HRESULT(WINAPI*)(ID3D12CommandQueue*,ID3D12Fence*,UINT64);
constexpr UINT page=65536;
std::string run(const wchar_t* requested,bool) {
    DXGI_ADAPTER_DESC1 selected{};
    constexpr std::array<UINT,18> release_profile{0x20004,0,UINT_MAX,0,0,0,UINT_MAX,UINT_MAX,0,1,0,0,0,0,0,0,0,0};
    auto device=create_device(requested,selected,release_profile.data(),__uuidof(ID3D12Device11));
    D3D12_ROOT_PARAMETER params[2]{};
    params[0].ParameterType=D3D12_ROOT_PARAMETER_TYPE_CBV;
    params[1].ParameterType=D3D12_ROOT_PARAMETER_TYPE_UAV;
    D3D12_ROOT_SIGNATURE_DESC rd{2,params,0,nullptr,D3D12_ROOT_SIGNATURE_FLAG_NONE};
    ComPtr<ID3DBlob> blob;
    check(D3D12SerializeRootSignature(&rd,D3D_ROOT_SIGNATURE_VERSION_1,&blob,nullptr),"serialize root");
    ComPtr<ID3D12RootSignature> root;
    check(device->CreateRootSignature(0,blob->GetBufferPointer(),blob->GetBufferSize(),IID_PPV_ARGS(&root)),"root");
    D3D12_COMPUTE_PIPELINE_STATE_DESC ps{};
    ps.pRootSignature=root.Get();ps.CS={root_cbv_dxil,sizeof(root_cbv_dxil)};
    ComPtr<ID3D12PipelineState> pipeline;
    check(method<Pipeline>(device.Get(),62)(device.Get(),&ps,0,nullptr,IID_PPV_ARGS(&pipeline)),"pipeline");
    struct Memory {
        UINT* data=static_cast<UINT*>(VirtualAlloc(nullptr,3*page,MEM_RESERVE|MEM_COMMIT,PAGE_READWRITE));
        ~Memory(){if(data)VirtualFree(data,0,MEM_RELEASE);}
    } memory;
    if(!memory.data)throw std::runtime_error("CBV pages");
    for(UINT i=0;i<3*page/4;++i)memory.data[i]=0x12340000u+i*13u;
    D3D12_COMMAND_QUEUE_DESC q{};q.Type=D3D12_COMMAND_LIST_TYPE_COMPUTE;
    ComPtr<ID3D12CommandQueue> queue;check(device->CreateCommandQueue(&q,IID_PPV_ARGS(&queue)),"queue");
    ComPtr<ID3D12Fence> fence;check(device->CreateFence(0,D3D12_FENCE_FLAG_NONE,IID_PPV_ARGS(&fence)),"fence");
    constexpr UINT n=24;
    std::array<UINT,n> initial;initial.fill(0xdeadbeef);
    auto seed=buffer(device.Get(),D3D12_HEAP_TYPE_UPLOAD,sizeof(initial),D3D12_RESOURCE_STATE_GENERIC_READ);
    upload(seed.Get(),initial.data(),sizeof(initial));
    auto result=buffer(device.Get(),D3D12_HEAP_TYPE_DEFAULT,sizeof(initial),D3D12_RESOURCE_STATE_COPY_DEST,D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
    auto readback=buffer(device.Get(),D3D12_HEAP_TYPE_READBACK,sizeof(initial),D3D12_RESOURCE_STATE_COPY_DEST);
    UINT pass=0;
    for(UINT offset:{0u,256u,page-256u,page,page+256u}){
        ComPtr<ID3D12CommandAllocator> allocator;
        check(device->CreateCommandAllocator(q.Type,IID_PPV_ARGS(&allocator)),"allocator");
        const UINT desc[]{D3D12_COMMAND_LIST_TYPE_COMPUTE,0,1,0};
        ComPtr<ID3D12GraphicsCommandList> list;
        check(method<CreateList>(device.Get(),73)(device.Get(),desc,allocator.Get(),pipeline.Get(),IID_PPV_ARGS(&list)),"list");
        if(pass)transition(list.Get(),result.Get(),D3D12_RESOURCE_STATE_COPY_SOURCE,D3D12_RESOURCE_STATE_COPY_DEST);
        list->CopyBufferRegion(result.Get(),0,seed.Get(),0,sizeof(initial));
        transition(list.Get(),result.Get(),D3D12_RESOURCE_STATE_COPY_DEST,D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
        method<Root>(list.Get(),33)(list.Get(),root.Get());
        method<View>(list.Get(),41)(list.Get(),0,reinterpret_cast<UINT64>(memory.data)+offset);
        method<View>(list.Get(),45)(list.Get(),1,result->GetGPUVirtualAddress());
        list->Dispatch(1,1,1);
        transition(list.Get(),result.Get(),D3D12_RESOURCE_STATE_UNORDERED_ACCESS,D3D12_RESOURCE_STATE_COPY_SOURCE);
        list->CopyBufferRegion(readback.Get(),0,result.Get(),0,sizeof(initial));
        check(list->Close(),"close");
        ID3D12CommandList* batch[]{list.Get()};queue->ExecuteCommandLists(1,batch);
        check(method<Signal>(queue.Get(),16)(queue.Get(),fence.Get(),++pass),"signal");
        wait_for_fence(fence.Get(),pass);
        auto expected=initial;
        const UINT indices[]{0,1,2048,4095};
        for(UINT j=0;j<4;++j)for(UINT k=0;k<4;++k)
            expected[4+4*j+k]=memory.data[offset/4+4*indices[j]+k];
        D3D12_RANGE range{0,sizeof(initial)},empty{};void* ptr{};
        check(readback->Map(0,&range,&ptr),"readback");
        const bool equal=std::memcmp(ptr,expected.data(),sizeof(expected))==0;
        readback->Unmap(0,&empty);
        if(!equal)throw std::runtime_error("root CBV mismatch at offset "+std::to_string(offset));
    }
    char receipt[160];
    std::snprintf(receipt,sizeof(receipt),"d3d12-root-cbv-window PASS pci=%04x:%04x words=120/120 window=65536 offsets=0,256,65280,65536,65792",selected.VendorId,selected.DeviceId);
    return receipt;
}
}
int wmain(int argc,wchar_t** argv){
    return tuxbox_test::frame_main(argc,argv,"d3d12-root-cbv-window",false,run);
}
