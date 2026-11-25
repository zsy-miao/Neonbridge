// Standalone bounded GPU guest-address lookup control.
#include "../independent-shaders/context.h"
#include <algorithm>
using namespace shader_control;

static std::vector<char> read(const char* name) {
    std::ifstream input(name,std::ios::binary);
    std::vector<char> data{std::istreambuf_iterator<char>{input},{}};
    if (data.empty() || data.size()%4) throw std::runtime_error(std::string("invalid ")+name);
    return data;
}
int main() {
    FILE* receipt{}; if (fopen_s(&receipt,"result.txt","wb")) return 2;
    int result=0;
    try {
        Context ctx(receipt);
        auto config=read("config.bin"), directory=read("directory.bin"), queries=read("queries.bin");
        auto d0=read("data0.bin"), d1=read("data1.bin"), d2=read("data2.bin");
        if (config.size()!=12) throw std::runtime_error("invalid config");
        UINT counts[3]{}; std::memcpy(counts,config.data(),12);
        if (counts[2]==0 || counts[2]>4096) throw std::runtime_error("invalid result capacity");
        const UINT outputWords=32+counts[2]*5;
        std::vector<UINT> initial(outputWords);
        for (UINT i=0;i<outputWords;++i) initial[i]=0x76540000u+i;
        auto seed=buffer(ctx.device.Get(),outputWords*4,D3D12_HEAP_TYPE_UPLOAD,D3D12_RESOURCE_STATE_GENERIC_READ);
        auto output=buffer(ctx.device.Get(),outputWords*4,D3D12_HEAP_TYPE_DEFAULT,D3D12_RESOURCE_STATE_COPY_DEST,D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
        auto readback=buffer(ctx.device.Get(),outputWords*4,D3D12_HEAP_TYPE_READBACK,D3D12_RESOURCE_STATE_COPY_DEST);
        upload(seed.Get(),initial.data(),outputWords*4);
        std::vector<std::vector<char>> inputData{directory,d0,d1,d2,queries};
        std::vector<ComPtr<ID3D12Resource>> uploads, inputs;
        for (const auto& data:inputData) {
            uploads.push_back(buffer(ctx.device.Get(),static_cast<UINT>(data.size()),D3D12_HEAP_TYPE_UPLOAD,D3D12_RESOURCE_STATE_GENERIC_READ));
            inputs.push_back(buffer(ctx.device.Get(),static_cast<UINT>(data.size()),D3D12_HEAP_TYPE_DEFAULT,D3D12_RESOURCE_STATE_COPY_DEST));
            upload(uploads.back().Get(),data.data(),data.size());
        }
        D3D12_DESCRIPTOR_HEAP_DESC hd{D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,6,D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,0};
        ComPtr<ID3D12DescriptorHeap> heap; check(ctx.device->CreateDescriptorHeap(&hd,IID_PPV_ARGS(&heap)),"heap");
        const UINT stride=ctx.device->GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV);
        auto cpu=heap->GetCPUDescriptorHandleForHeapStart();
        D3D12_SHADER_RESOURCE_VIEW_DESC srv{}; srv.Format=DXGI_FORMAT_R32_TYPELESS;
        srv.ViewDimension=D3D12_SRV_DIMENSION_BUFFER; srv.Shader4ComponentMapping=D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
        srv.Buffer.Flags=D3D12_BUFFER_SRV_FLAG_RAW;
        for (size_t i=0;i<inputs.size();++i) {
            srv.Buffer.NumElements=static_cast<UINT>(inputData[i].size()/4);
            ctx.device->CreateShaderResourceView(inputs[i].Get(),&srv,cpu); cpu.ptr+=stride;
        }
        D3D12_UNORDERED_ACCESS_VIEW_DESC uav{}; uav.Format=DXGI_FORMAT_R32_TYPELESS;
        uav.ViewDimension=D3D12_UAV_DIMENSION_BUFFER; uav.Buffer.FirstElement=16;
        uav.Buffer.NumElements=counts[2]*5; uav.Buffer.Flags=D3D12_BUFFER_UAV_FLAG_RAW;
        ctx.device->CreateUnorderedAccessView(output.Get(),nullptr,&uav,cpu);
        D3D12_DESCRIPTOR_RANGE ranges[2]={{D3D12_DESCRIPTOR_RANGE_TYPE_SRV,5,0,0,0},{D3D12_DESCRIPTOR_RANGE_TYPE_UAV,1,0,0,0}};
        D3D12_ROOT_PARAMETER params[3]{};
        for (UINT i=0;i<2;++i) { params[i].ParameterType=D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE; params[i].DescriptorTable={1,&ranges[i]}; }
        params[2].ParameterType=D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS; params[2].Constants.Num32BitValues=2;
        D3D12_ROOT_SIGNATURE_DESC desc{3,params,0,nullptr,D3D12_ROOT_SIGNATURE_FLAG_NONE};
        ComPtr<ID3DBlob> blob; check(D3D12SerializeRootSignature(&desc,D3D_ROOT_SIGNATURE_VERSION_1,&blob,nullptr),"root serialization");
        ComPtr<ID3D12RootSignature> root; check(ctx.device->CreateRootSignature(0,blob->GetBufferPointer(),blob->GetBufferSize(),IID_PPV_ARGS(&root)),"root");
        auto code=read_shader("lookup");
        D3D12_COMPUTE_PIPELINE_STATE_DESC pd{}; pd.pRootSignature=root.Get(); pd.CS={code.data(),code.size()};
        ComPtr<ID3D12PipelineState> pso; check(ctx.device->CreateComputePipelineState(&pd,IID_PPV_ARGS(&pso)),"pipeline");
        ComPtr<ID3D12CommandAllocator> allocator; ComPtr<ID3D12GraphicsCommandList> list;
        check(ctx.device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT,IID_PPV_ARGS(&allocator)),"allocator");
        check(ctx.device->CreateCommandList(0,D3D12_COMMAND_LIST_TYPE_DIRECT,allocator.Get(),pso.Get(),IID_PPV_ARGS(&list)),"list");
        for (size_t i=0;i<inputs.size();++i) {
            list->CopyBufferRegion(inputs[i].Get(),0,uploads[i].Get(),0,inputData[i].size());
            transition(list.Get(),inputs[i].Get(),D3D12_RESOURCE_STATE_COPY_DEST,D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
        }
        list->CopyBufferRegion(output.Get(),0,seed.Get(),0,outputWords*4);
        transition(list.Get(),output.Get(),D3D12_RESOURCE_STATE_COPY_DEST,D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
        ID3D12DescriptorHeap* heaps[]={heap.Get()}; list->SetDescriptorHeaps(1,heaps);
        list->SetComputeRootSignature(root.Get()); auto gpu=heap->GetGPUDescriptorHandleForHeapStart();
        list->SetComputeRootDescriptorTable(0,gpu); gpu.ptr+=UINT64(stride)*5;
        list->SetComputeRootDescriptorTable(1,gpu); list->SetComputeRoot32BitConstants(2,2,counts,0);
        const UINT dispatch=std::min(counts[1],counts[2]);
        if (dispatch) list->Dispatch((dispatch+63)/64,1,1);
        transition(list.Get(),output.Get(),D3D12_RESOURCE_STATE_UNORDERED_ACCESS,D3D12_RESOURCE_STATE_COPY_SOURCE);
        list->CopyBufferRegion(readback.Get(),0,output.Get(),0,outputWords*4);
        submit_and_wait(ctx.device.Get(),ctx.queue.Get(),list.Get());
        write_buffer(readback.Get(),"lookup",outputWords*4);
        std::fprintf(receipt,"entries=%u\nqueries=%u\ncapacity=%u\n",counts[0],counts[1],counts[2]);
    } catch (const std::exception& e) { std::fprintf(receipt,"error=%s\n",e.what()); result=1; }
    return std::fclose(receipt) ? 2 : result;
}
