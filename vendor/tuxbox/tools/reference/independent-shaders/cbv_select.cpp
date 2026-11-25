// GPU observation of the translated finite CBV selector.
#include "context.h"
using namespace shader_control;
static void run(Context& ctx) {
    auto device=ctx.device.Get();
    constexpr UINT words=384, groups=128, start=96;
    D3D12_DESCRIPTOR_RANGE ranges[]={
        {D3D12_DESCRIPTOR_RANGE_TYPE_CBV,2,0,0,0},
        {D3D12_DESCRIPTOR_RANGE_TYPE_UAV,1,0,0,2}};
    D3D12_ROOT_PARAMETER table{};
    table.ParameterType=D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE;
    table.DescriptorTable={2,ranges};
    D3D12_ROOT_SIGNATURE_DESC desc{1,&table,0,nullptr,D3D12_ROOT_SIGNATURE_FLAG_NONE};
    ComPtr<ID3DBlob> blob;
    check(D3D12SerializeRootSignature(&desc,D3D_ROOT_SIGNATURE_VERSION_1,&blob,nullptr),"serialize");
    ComPtr<ID3D12RootSignature> root;
    check(device->CreateRootSignature(0,blob->GetBufferPointer(),blob->GetBufferSize(),IID_PPV_ARGS(&root)),"root");
    auto code=read_shader("selector");
    D3D12_COMPUTE_PIPELINE_STATE_DESC pd{};
    pd.pRootSignature=root.Get();pd.CS={code.data(),code.size()};
    ComPtr<ID3D12PipelineState> pipeline;
    check(device->CreateComputePipelineState(&pd,IID_PPV_ARGS(&pipeline)),"pipeline");
    auto cb0=buffer(device,256,D3D12_HEAP_TYPE_UPLOAD,D3D12_RESOURCE_STATE_GENERIC_READ);
    auto cb1=buffer(device,256,D3D12_HEAP_TYPE_UPLOAD,D3D12_RESOURCE_STATE_GENERIC_READ);
    auto seed=buffer(device,words*4,D3D12_HEAP_TYPE_UPLOAD,D3D12_RESOURCE_STATE_GENERIC_READ);
    auto out=buffer(device,words*4,D3D12_HEAP_TYPE_DEFAULT,D3D12_RESOURCE_STATE_COPY_DEST,D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
    auto readback=buffer(device,words*4,D3D12_HEAP_TYPE_READBACK,D3D12_RESOURCE_STATE_COPY_DEST);
    std::array<UINT,64> a{},b{};a[0]=0x12345678;b[0]=0xfedcba98;
    upload(cb0.Get(),a.data(),sizeof(a));upload(cb1.Get(),b.data(),sizeof(b));
    std::array<UINT,words> initial{};
    for (UINT i=0;i<words;++i) initial[i]=0x76540000+i;
    upload(seed.Get(),initial.data(),sizeof(initial));
    D3D12_DESCRIPTOR_HEAP_DESC hd{D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,3,D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,0};
    ComPtr<ID3D12DescriptorHeap> heap;
    check(device->CreateDescriptorHeap(&hd,IID_PPV_ARGS(&heap)),"heap");
    const auto step=device->GetDescriptorHandleIncrementSize(hd.Type);
    auto cpu=heap->GetCPUDescriptorHandleForHeapStart();
    D3D12_CONSTANT_BUFFER_VIEW_DESC cbv{cb0->GetGPUVirtualAddress(),256};
    device->CreateConstantBufferView(&cbv,cpu);cpu.ptr+=step;
    cbv.BufferLocation=cb1->GetGPUVirtualAddress();
    device->CreateConstantBufferView(&cbv,cpu);cpu.ptr+=step;
    D3D12_UNORDERED_ACCESS_VIEW_DESC uav{};
    uav.Format=DXGI_FORMAT_R32_UINT;uav.ViewDimension=D3D12_UAV_DIMENSION_BUFFER;
    uav.Buffer.FirstElement=start;uav.Buffer.NumElements=groups;
    device->CreateUnorderedAccessView(out.Get(),nullptr,&uav,cpu);
    ComPtr<ID3D12CommandAllocator> allocator;
    ComPtr<ID3D12GraphicsCommandList> list;
    check(device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT,IID_PPV_ARGS(&allocator)),"allocator");
    check(device->CreateCommandList(0,D3D12_COMMAND_LIST_TYPE_DIRECT,allocator.Get(),pipeline.Get(),IID_PPV_ARGS(&list)),"list");
    list->CopyBufferRegion(out.Get(),0,seed.Get(),0,sizeof(initial));
    transition(list.Get(),out.Get(),D3D12_RESOURCE_STATE_COPY_DEST,D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
    ID3D12DescriptorHeap* heaps[]={heap.Get()};
    list->SetDescriptorHeaps(1,heaps);
    list->SetComputeRootSignature(root.Get());
    list->SetComputeRootDescriptorTable(0,heap->GetGPUDescriptorHandleForHeapStart());
    list->Dispatch(groups,1,1);
    transition(list.Get(),out.Get(),D3D12_RESOURCE_STATE_UNORDERED_ACCESS,D3D12_RESOURCE_STATE_COPY_SOURCE);
    list->CopyBufferRegion(readback.Get(),0,out.Get(),0,sizeof(initial));
    submit_and_wait(device,ctx.queue.Get(),list.Get());
    write_buffer(readback.Get(),"selector",sizeof(initial));
}
int main(){
    FILE* receipt{};
    if(fopen_s(&receipt,"result.txt","wb")) return 2;
    int status=0;
    try { Context ctx(receipt);run(ctx);std::fprintf(receipt,"words=384\n"); }
    catch(const std::exception& e){std::fprintf(receipt,"error=%s\n",e.what());status=1;}
    return std::fclose(receipt)?2:status;
}
