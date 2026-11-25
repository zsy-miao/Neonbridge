// Guarded GPU traversal of a synthetic Xbox-layout TLAS and validated BLAS.
#include "../independent-shaders/context.h"
#include "traverse_dxil.h"
#include "verified_initialize_dxil.h"
#include "verified_compare_dxil.h"
#include "verified_commit_dxil.h"
#include "verified_finalize_dxil.h"
#include <algorithm>
using namespace shader_control;
extern "C" int tb_shadow_create(ID3D12Device*, const unsigned char*, size_t, void**);
extern "C" int tb_shadow_info(const void*, UINT64*, UINT64*);
extern "C" int tb_shadow_gpu_resources(const void*, ID3D12Resource**, ID3D12Resource**, UINT64*);
extern "C" int tb_shadow_retain_recording(void*);
extern "C" int tb_shadow_hold(void*, ID3D12Fence*, UINT64);
extern "C" int tb_shadow_release_recording(void*);
extern "C" int tb_shadow_try_release(void*);

static std::vector<char> read(const char* name) {
    std::ifstream input(name, std::ios::binary);
    std::vector<char> data{std::istreambuf_iterator<char>{input}, {}};
    if (data.empty()) throw std::runtime_error(std::string("missing ") + name);
    return data;
}
static void wait(ID3D12Fence* fence, UINT64 value) {
    HANDLE event = CreateEventW(nullptr, FALSE, FALSE, nullptr);
    if (!event) throw std::runtime_error("create event");
    HRESULT hr = fence->SetEventOnCompletion(value,event);
    DWORD status = SUCCEEDED(hr) ? WaitForSingleObject(event,15000) : WAIT_FAILED;
    CloseHandle(event); check(hr,"set event");
    if (status != WAIT_OBJECT_0 || fence->GetCompletedValue() < value) throw std::runtime_error("fence wait");
}
int main() {
    FILE* receipt{}; if (fopen_s(&receipt,"result.txt","wb")) return 2;
    int result=0;
    try {
        Context ctx(receipt);
        auto source=read("sphere.serialized"), scene=read("scene.bin"), rays=read("rays.bin");
        if (scene.size()!=576 || rays.size()%36 || rays.size()/36>65535*64) throw std::runtime_error("invalid fixture");
        const UINT count=static_cast<UINT>(rays.size()/36), bytes=count*16;
        void* shadow{};
        if (tb_shadow_create(ctx.device.Get(),reinterpret_cast<const unsigned char*>(source.data()),source.size(),&shadow) || !shadow)
            throw std::runtime_error("validated shadow creation");
        UINT64 address{}, size{};
        if (tb_shadow_info(shadow,&address,&size) || size+64!=source.size()) throw std::runtime_error("shadow info");
        ID3D12Resource* shadowResource{}; ID3D12Resource* validationResource{}; UINT64 validationAddress{};
        if (tb_shadow_gpu_resources(shadow,&shadowResource,&validationResource,&validationAddress) ||
            !shadowResource || !validationResource || !validationAddress || (size&3) || size>64*1024*1024)
            throw std::runtime_error("verification resources");
        auto guestUpload=buffer(ctx.device.Get(),static_cast<UINT>(size+256),D3D12_HEAP_TYPE_UPLOAD,D3D12_RESOURCE_STATE_GENERIC_READ);
        auto guestGpu=buffer(ctx.device.Get(),static_cast<UINT>(size+128),D3D12_HEAP_TYPE_DEFAULT,D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
        auto status=buffer(ctx.device.Get(),4,D3D12_HEAP_TYPE_DEFAULT,D3D12_RESOURCE_STATE_UNORDERED_ACCESS,D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
        auto sceneBuffer=buffer(ctx.device.Get(),static_cast<UINT>(scene.size()),D3D12_HEAP_TYPE_UPLOAD,D3D12_RESOURCE_STATE_GENERIC_READ);
        auto rayBuffer=buffer(ctx.device.Get(),static_cast<UINT>(rays.size()),D3D12_HEAP_TYPE_UPLOAD,D3D12_RESOURCE_STATE_GENERIC_READ);
        const UINT guards[]={1,0};
        auto guardBuffer=buffer(ctx.device.Get(),sizeof(guards),D3D12_HEAP_TYPE_UPLOAD,D3D12_RESOURCE_STATE_GENERIC_READ);
        auto output=buffer(ctx.device.Get(),bytes*2+84,D3D12_HEAP_TYPE_DEFAULT,D3D12_RESOURCE_STATE_UNORDERED_ACCESS,D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
        auto readback=buffer(ctx.device.Get(),bytes*2+84,D3D12_HEAP_TYPE_READBACK,D3D12_RESOURCE_STATE_COPY_DEST);
        upload(sceneBuffer.Get(),scene.data(),scene.size()); upload(rayBuffer.Get(),rays.data(),rays.size()); upload(guardBuffer.Get(),guards,sizeof(guards));
        std::array<unsigned char, 5 * 36> negative{};
        for (UINT i=0;i<5;++i) std::memcpy(negative.data()+i*36,rays.data(),36);
        const UINT bad_flags=0x200, one=0x3f800000, nan=0x7fc00000;
        std::memcpy(negative.data()+32,&bad_flags,4);
        std::memcpy(negative.data()+36+12,&one,4);
        std::memcpy(negative.data()+72+28,&nan,4);
        std::memcpy(negative.data()+108+16,&nan,4);
        std::memset(negative.data()+144+16,0,12);
        auto negativeBuffer=buffer(ctx.device.Get(),negative.size(),D3D12_HEAP_TYPE_UPLOAD,D3D12_RESOURCE_STATE_GENERIC_READ);
        upload(negativeBuffer.Get(),negative.data(),negative.size());
        D3D12_ROOT_PARAMETER params[6]{};
        for (UINT n=0;n<4;++n) { params[n].ParameterType=D3D12_ROOT_PARAMETER_TYPE_SRV; params[n].Descriptor.ShaderRegister=n; }
        params[4].ParameterType=D3D12_ROOT_PARAMETER_TYPE_UAV;
        params[5].ParameterType=D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS; params[5].Constants.Num32BitValues=1;
        D3D12_ROOT_SIGNATURE_DESC desc{6,params,0,nullptr,D3D12_ROOT_SIGNATURE_FLAG_NONE};
        ComPtr<ID3DBlob> blob; check(D3D12SerializeRootSignature(&desc,D3D_ROOT_SIGNATURE_VERSION_1,&blob,nullptr),"serialize root");
        ComPtr<ID3D12RootSignature> root; check(ctx.device->CreateRootSignature(0,blob->GetBufferPointer(),blob->GetBufferSize(),IID_PPV_ARGS(&root)),"root");
        D3D12_COMPUTE_PIPELINE_STATE_DESC pd{}; pd.pRootSignature=root.Get(); pd.CS={traverse_dxil,sizeof(traverse_dxil)};
        ComPtr<ID3D12PipelineState> pso; check(ctx.device->CreateComputePipelineState(&pd,IID_PPV_ARGS(&pso)),"pipeline");
        // Bounded SRV/UAV descriptors for GPU validation and publication.
        D3D12_DESCRIPTOR_HEAP_DESC heapDesc{D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,4,D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,0};
        ComPtr<ID3D12DescriptorHeap> heap; check(ctx.device->CreateDescriptorHeap(&heapDesc,IID_PPV_ARGS(&heap)),"verification heap");
        const UINT stride=ctx.device->GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV);
        auto cpu=heap->GetCPUDescriptorHandleForHeapStart();
        D3D12_SHADER_RESOURCE_VIEW_DESC srv{}; srv.Format=DXGI_FORMAT_R32_TYPELESS; srv.ViewDimension=D3D12_SRV_DIMENSION_BUFFER;
        srv.Shader4ComponentMapping=D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING; srv.Buffer.Flags=D3D12_BUFFER_SRV_FLAG_RAW;
        srv.Buffer.FirstElement=128/4; srv.Buffer.NumElements=static_cast<UINT>(size/4);
        ctx.device->CreateShaderResourceView(guestGpu.Get(),&srv,cpu); cpu.ptr+=stride;
        srv.Buffer.FirstElement=0; ctx.device->CreateShaderResourceView(validationResource,&srv,cpu); cpu.ptr+=stride;
        D3D12_UNORDERED_ACCESS_VIEW_DESC uv{}; uv.Format=DXGI_FORMAT_R32_TYPELESS; uv.ViewDimension=D3D12_UAV_DIMENSION_BUFFER;
        uv.Buffer.Flags=D3D12_BUFFER_UAV_FLAG_RAW; uv.Buffer.NumElements=static_cast<UINT>(size/4);
        ctx.device->CreateUnorderedAccessView(shadowResource,nullptr,&uv,cpu); cpu.ptr+=stride;
        uv.Buffer.NumElements=1; ctx.device->CreateUnorderedAccessView(status.Get(),nullptr,&uv,cpu);
        D3D12_DESCRIPTOR_RANGE ranges[2]={{D3D12_DESCRIPTOR_RANGE_TYPE_SRV,2,0,0,0},{D3D12_DESCRIPTOR_RANGE_TYPE_UAV,2,0,0,0}};
        D3D12_ROOT_PARAMETER vp[3]{};
        for (UINT i=0;i<2;++i) { vp[i].ParameterType=D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE; vp[i].DescriptorTable={1,&ranges[i]}; }
        vp[2].ParameterType=D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS; vp[2].Constants.Num32BitValues=3;
        D3D12_ROOT_SIGNATURE_DESC vd{3,vp,0,nullptr,D3D12_ROOT_SIGNATURE_FLAG_NONE};
        ComPtr<ID3DBlob> vb; check(D3D12SerializeRootSignature(&vd,D3D_ROOT_SIGNATURE_VERSION_1,&vb,nullptr),"verification signature");
        ComPtr<ID3D12RootSignature> vr; check(ctx.device->CreateRootSignature(0,vb->GetBufferPointer(),vb->GetBufferSize(),IID_PPV_ARGS(&vr)),"verification root");
        const D3D12_SHADER_BYTECODE codes[]={{verified_initialize_dxil,sizeof(verified_initialize_dxil)},
            {verified_compare_dxil,sizeof(verified_compare_dxil)},{verified_commit_dxil,sizeof(verified_commit_dxil)},
            {verified_finalize_dxil,sizeof(verified_finalize_dxil)}};
        ComPtr<ID3D12PipelineState> verifyPso[4];
        for (UINT i=0;i<4;++i) { D3D12_COMPUTE_PIPELINE_STATE_DESC vpd{}; vpd.pRootSignature=vr.Get(); vpd.CS=codes[i];
            check(ctx.device->CreateComputePipelineState(&vpd,IID_PPV_ARGS(&verifyPso[i])),"verification pipeline"); }
        ComPtr<ID3D12CommandAllocator> allocator; ComPtr<ID3D12GraphicsCommandList> list;
        check(ctx.device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT,IID_PPV_ARGS(&allocator)),"allocator");
        check(ctx.device->CreateCommandList(0,D3D12_COMMAND_LIST_TYPE_DIRECT,allocator.Get(),pso.Get(),IID_PPV_ARGS(&list)),"list");
        if (tb_shadow_retain_recording(shadow)) throw std::runtime_error("shadow recording hold");
        transition(list.Get(),guestGpu.Get(),D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,D3D12_RESOURCE_STATE_COPY_DEST);
        list->CopyBufferRegion(guestGpu.Get(),128,guestUpload.Get(),256,size);
        transition(list.Get(),guestGpu.Get(),D3D12_RESOURCE_STATE_COPY_DEST,D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
        transition(list.Get(),shadowResource,D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
        list->SetComputeRootSignature(vr.Get());
        ID3D12DescriptorHeap* heaps[]={heap.Get()}; list->SetDescriptorHeaps(1,heaps);
        auto gpu=heap->GetGPUDescriptorHandleForHeapStart(); list->SetComputeRootDescriptorTable(0,gpu);
        gpu.ptr+=UINT64(stride)*2; list->SetComputeRootDescriptorTable(1,gpu);
        const UINT groups=static_cast<UINT>((size+255)/256), groupsX=std::min(groups,65535u), groupsY=(groups+groupsX-1)/groupsX;
        const UINT constants[]={static_cast<UINT>(size),groupsX,groupsY}; list->SetComputeRoot32BitConstants(2,3,constants,0);
        for (UINT stage=0;stage<4;++stage) {
            list->SetPipelineState(verifyPso[stage].Get());
            list->Dispatch(stage==1 || stage==2 ? groupsX : 1,stage==1 || stage==2 ? groupsY : 1,1);
            D3D12_RESOURCE_BARRIER barrier{}; barrier.Type=D3D12_RESOURCE_BARRIER_TYPE_UAV; list->ResourceBarrier(1,&barrier);
        }
        transition(list.Get(),shadowResource,D3D12_RESOURCE_STATE_UNORDERED_ACCESS,D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
        transition(list.Get(),status.Get(),D3D12_RESOURCE_STATE_UNORDERED_ACCESS,D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
        list->SetComputeRootSignature(root.Get());
        list->SetPipelineState(pso.Get());
        list->SetComputeRootShaderResourceView(0,sceneBuffer->GetGPUVirtualAddress());
        list->SetComputeRootShaderResourceView(1,address);
        list->SetComputeRootShaderResourceView(2,rayBuffer->GetGPUVirtualAddress());
        list->SetComputeRootShaderResourceView(3,status->GetGPUVirtualAddress());
        list->SetComputeRootUnorderedAccessView(4,output->GetGPUVirtualAddress());
        list->SetComputeRoot32BitConstant(5,count,0);
        list->Dispatch((count+63)/64,1,1);
        D3D12_RESOURCE_BARRIER uav{}; uav.Type=D3D12_RESOURCE_BARRIER_TYPE_UAV; uav.UAV.pResource=output.Get(); list->ResourceBarrier(1,&uav);
        list->SetComputeRootShaderResourceView(3,guardBuffer->GetGPUVirtualAddress()+4);
        list->SetComputeRootUnorderedAccessView(4,output->GetGPUVirtualAddress()+bytes);
        list->Dispatch((count+63)/64,1,1);
        list->ResourceBarrier(1,&uav);
        list->SetComputeRootShaderResourceView(2,negativeBuffer->GetGPUVirtualAddress());
        list->SetComputeRootShaderResourceView(3,status->GetGPUVirtualAddress());
        list->SetComputeRootUnorderedAccessView(4,output->GetGPUVirtualAddress()+bytes*2);
        list->SetComputeRoot32BitConstant(5,5,0);
        list->Dispatch(1,1,1);
        transition(list.Get(),output.Get(),D3D12_RESOURCE_STATE_UNORDERED_ACCESS,D3D12_RESOURCE_STATE_COPY_SOURCE);
        list->CopyBufferRegion(readback.Get(),0,output.Get(),0,bytes*2+80);
        transition(list.Get(),status.Get(),D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,D3D12_RESOURCE_STATE_COPY_SOURCE);
        list->CopyBufferRegion(readback.Get(),bytes*2+80,status.Get(),0,4);
        transition(list.Get(),status.Get(),D3D12_RESOURCE_STATE_COPY_SOURCE,D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
        check(list->Close(),"close");
        std::vector<char> guestData(static_cast<size_t>(size)+256);
        std::memcpy(guestData.data()+256,source.data()+64,size);
        upload(guestUpload.Get(),guestData.data(),guestData.size());
        ComPtr<ID3D12Fence> fence; check(ctx.device->CreateFence(0,D3D12_FENCE_FLAG_NONE,IID_PPV_ARGS(&fence)),"fence");
        if (tb_shadow_hold(shadow,fence.Get(),1)) throw std::runtime_error("hold");
        ID3D12CommandList* lists[]={list.Get()}; ctx.queue->ExecuteCommandLists(1,lists);
        check(ctx.queue->Signal(fence.Get(),1),"signal"); wait(fence.Get(),1);
        write_buffer(readback.Get(),"hits",bytes*2+84);
        // Restore transient states after completion, then replay with the final
        // GPU source byte changed. The verifier must reject every traversal.
        ComPtr<ID3D12CommandAllocator> resetAllocator; ComPtr<ID3D12GraphicsCommandList> reset;
        check(ctx.device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT,IID_PPV_ARGS(&resetAllocator)),"reset allocator");
        check(ctx.device->CreateCommandList(0,D3D12_COMMAND_LIST_TYPE_DIRECT,resetAllocator.Get(),nullptr,IID_PPV_ARGS(&reset)),"reset list");
        transition(reset.Get(),status.Get(),D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
        transition(reset.Get(),output.Get(),D3D12_RESOURCE_STATE_COPY_SOURCE,D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
        submit_and_wait(ctx.device.Get(),ctx.queue.Get(),reset.Get());
        guestData.back() ^= 0x5a; upload(guestUpload.Get(),guestData.data(),guestData.size());
        if (tb_shadow_hold(shadow,fence.Get(),2)) throw std::runtime_error("replay hold");
        ctx.queue->ExecuteCommandLists(1,lists); check(ctx.queue->Signal(fence.Get(),2),"replay signal"); wait(fence.Get(),2);
        write_buffer(readback.Get(),"mismatch",bytes*2+84);
        list.Reset();
        if (tb_shadow_release_recording(shadow) || tb_shadow_try_release(shadow)!=1) throw std::runtime_error("retirement");
        std::fprintf(receipt,"rays=%u\nguard_failure_dispatch=1\nunsupported_ray_rejections=5\ngpu_verified_source_replay=1\n",count);
    } catch (const std::exception& e) { std::fprintf(receipt,"error=%s\n",e.what()); result=1; }
    return std::fclose(receipt) ? 2 : result;
}
