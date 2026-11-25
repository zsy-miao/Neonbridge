// Real Scarlett slot 86 link request and guarded physical draw.
#include "frame_stream.h"
#include "compute_fixture.h"
#include <fstream>
#include <iterator>
#include <vector>
using namespace frame_abi;
using namespace compute_fixture;
namespace {
using Make=HRESULT(WINAPI*)(ID3D12Device*,const D3D12_PIPELINE_STATE_STREAM_DESC*,REFIID,void**);
using SetRoot=void(WINAPI*)(ID3D12GraphicsCommandList*,ID3D12RootSignature*);
using SetTargets=void(WINAPI*)(ID3D12GraphicsCommandList*,UINT,const D3D12_CPU_DESCRIPTOR_HANDLE*,BOOL,const D3D12_CPU_DESCRIPTOR_HANDLE*);
using Signal=HRESULT(WINAPI*)(ID3D12CommandQueue*,ID3D12Fence*,UINT64);
std::vector<char> read(const char* name){
    std::ifstream f(name,std::ios::binary);
    std::vector<char> bytes{std::istreambuf_iterator<char>(f),{}};
    if(bytes.empty())throw std::runtime_error(name);
    return bytes;
}
constexpr UINT side=16,pitch=256;
void draw(ID3D12Device* device,ID3D12CommandQueue* queue,ID3D12RootSignature* root,
          const std::vector<char>& vs,const std::vector<char>& ps,UINT variant){
    RasterPipelineStream parts;
    parts.root.payload=root;parts.vs.payload={vs.data(),vs.size()};parts.ps.payload={ps.data(),ps.size()};
    parts.raster.payload.FillMode=D3D12_FILL_MODE_SOLID;parts.raster.payload.CullMode=D3D12_CULL_MODE_NONE;
    parts.raster.payload.DepthClipEnable=TRUE;parts.dsv.payload=DXGI_FORMAT_UNKNOWN;
    parts.formats.payload.NumRenderTargets=1;parts.formats.payload.RTFormats[0]=DXGI_FORMAT_R32G32B32A32_FLOAT;
    parts.blend.payload.RenderTarget[0].RenderTargetWriteMask=D3D12_COLOR_WRITE_ENABLE_ALL;
    D3D12_PIPELINE_STATE_STREAM_DESC raw{sizeof(parts),&parts};
    ComPtr<ID3D12PipelineState> pipeline;
    check(method<Make>(device,86)(device,&raw,IID_PPV_ARGS(&pipeline)),"slot 86 linked pipeline");
    D3D12_RESOURCE_DESC d{};d.Dimension=D3D12_RESOURCE_DIMENSION_TEXTURE2D;d.Width=d.Height=side;
    d.DepthOrArraySize=d.MipLevels=d.SampleDesc.Count=1;d.Format=parts.formats.payload.RTFormats[0];
    d.Flags=D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET;
    D3D12_HEAP_PROPERTIES hp{};hp.Type=D3D12_HEAP_TYPE_DEFAULT;hp.CreationNodeMask=hp.VisibleNodeMask=1;
    ComPtr<ID3D12Resource> target;
    check(device->CreateCommittedResource(&hp,D3D12_HEAP_FLAG_NONE,&d,D3D12_RESOURCE_STATE_RENDER_TARGET,nullptr,IID_PPV_ARGS(&target)),"target");
    auto out=buffer(device,D3D12_HEAP_TYPE_READBACK,side*pitch,D3D12_RESOURCE_STATE_COPY_DEST);
    D3D12_DESCRIPTOR_HEAP_DESC hd{D3D12_DESCRIPTOR_HEAP_TYPE_RTV,1,D3D12_DESCRIPTOR_HEAP_FLAG_NONE,0};
    ComPtr<ID3D12DescriptorHeap> heap;
    check(device->CreateDescriptorHeap(&hd,IID_PPV_ARGS(&heap)),"RTV heap");
    D3D12_CPU_DESCRIPTOR_HANDLE rtv{method<CpuHandle>(heap.Get(),9)(heap.Get())};
    device->CreateRenderTargetView(target.Get(),nullptr,rtv);
    ComPtr<ID3D12CommandAllocator> alloc;
    check(device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT,IID_PPV_ARGS(&alloc)),"allocator");
    const UINT params[]{D3D12_COMMAND_LIST_TYPE_DIRECT,0,1,0};
    ComPtr<ID3D12GraphicsCommandList> list;
    check(method<CreateList>(device,73)(device,params,alloc.Get(),nullptr,IID_PPV_ARGS(&list)),"list");
    constexpr float guard[]{-1,-2,-3,-4};
    method<Clear>(list.Get(),52)(list.Get(),rtv,guard,0,nullptr);
    std::memcpy(reinterpret_cast<BYTE*>(list.Get())+0x540,reinterpret_cast<const BYTE*>(pipeline.Get())+0x10,32);
    list->SetPipelineState(pipeline.Get());
    method<SetRoot>(list.Get(),34)(list.Get(),root);
    method<SetTargets>(list.Get(),50)(list.Get(),1,&rtv,FALSE,nullptr);
    D3D12_VIEWPORT vp{0,0,side,side,0,1};D3D12_RECT rect{3,2,12,11};
    list->RSSetViewports(1,&vp);list->RSSetScissorRects(1,&rect);
    list->IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);list->DrawInstanced(3,1,0,0);
    transition(list.Get(),target.Get(),D3D12_RESOURCE_STATE_RENDER_TARGET,D3D12_RESOURCE_STATE_COPY_SOURCE);
    D3D12_TEXTURE_COPY_LOCATION from{},to{};from.pResource=target.Get();from.Type=D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
    to.pResource=out.Get();to.Type=D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;to.PlacedFootprint={0,{d.Format,side,side,1,pitch}};
    method<Copy>(list.Get(),83)(list.Get(),&to,0,0,0,&from,nullptr,0);
    check(list->Close(),"close");ID3D12CommandList* batch[]{list.Get()};queue->ExecuteCommandLists(1,batch);
    ComPtr<ID3D12Fence> fence;check(device->CreateFence(0,D3D12_FENCE_FLAG_NONE,IID_PPV_ARGS(&fence)),"fence");
    check(method<Signal>(queue,16)(queue,fence.Get(),1),"signal");wait_for_fence(fence.Get(),1);
    D3D12_RANGE range{0,side*pitch},empty{};void* ptr{};check(out->Map(0,&range,&ptr),"readback");
    const float color[]{1,variant?0.5f:0.25f,variant?0.25f:0.5f,1};
    bool match=true;
    for(UINT y=0;y<side;++y)for(UINT x=0;x<side;++x){
        const float* want=x>=3&&x<12&&y>=2&&y<11?color:guard;
        match &= std::memcmp(static_cast<const BYTE*>(ptr)+y*pitch+x*16,want,16)==0;
    }
    out->Unmap(0,&empty);if(!match)throw std::runtime_error("slot 86 pixel mismatch");
}
}
int main(){
    FILE* report{};if(fopen_s(&report,"result.txt","wb"))return 2;
    int status=0;
    try{
        DXGI_ADAPTER_DESC1 selected{};
        constexpr std::array<UINT,18> profile{0x20004,0,UINT_MAX,0,0,0,UINT_MAX,UINT_MAX,0,1};
        auto device=create_device(nullptr,selected,profile.data(),__uuidof(ID3D12Device11));
        std::fprintf(report,"adapter=%04x:%04x\n",selected.VendorId,selected.DeviceId);
        D3D12_ROOT_SIGNATURE_DESC desc{};ComPtr<ID3DBlob> serialized;
        check(D3D12SerializeRootSignature(&desc,D3D_ROOT_SIGNATURE_VERSION_1,&serialized,nullptr),"serialize");
        ComPtr<ID3D12RootSignature> root;
        check(device->CreateRootSignature(0,serialized->GetBufferPointer(),serialized->GetBufferSize(),IID_PPV_ARGS(&root)),"root");
        D3D12_COMMAND_QUEUE_DESC q{};ComPtr<ID3D12CommandQueue> queue;
        check(device->CreateCommandQueue(&q,IID_PPV_ARGS(&queue)),"queue");
        auto vs=read("vertex.dxbc"),p0=read("p0.cso"),p1=read("p1.cso");
        for(UINT i=0;i<4;++i)draw(device.Get(),queue.Get(),root.Get(),vs,i%2?p1:p0,i%2);
        std::fprintf(report,"draws=4\npixels=1024\n");
    }catch(const std::exception& e){std::fprintf(report,"error=%s\n",e.what());status=1;}
    return std::fclose(report)?2:status;
}
