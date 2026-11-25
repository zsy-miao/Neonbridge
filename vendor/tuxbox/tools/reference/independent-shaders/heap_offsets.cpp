// Physical pixel readback for synthetic directly indexed texture/sampler heaps.
#include "context.h"
#include <array>
using namespace shader_control;
static constexpr UINT side=16,pitch=256,bytes=side*pitch;

static ComPtr<ID3D12Resource> image(ID3D12Device* device,UINT size,D3D12_RESOURCE_FLAGS flags,D3D12_RESOURCE_STATES state){
    D3D12_RESOURCE_DESC d{};
    d.Dimension=D3D12_RESOURCE_DIMENSION_TEXTURE2D;d.Width=d.Height=size;
    d.DepthOrArraySize=d.MipLevels=d.SampleDesc.Count=1;
    d.Format=DXGI_FORMAT_R32G32B32A32_FLOAT;d.Flags=flags;
    D3D12_HEAP_PROPERTIES hp{};hp.Type=D3D12_HEAP_TYPE_DEFAULT;hp.CreationNodeMask=hp.VisibleNodeMask=1;
    ComPtr<ID3D12Resource> out;
    check(device->CreateCommittedResource(&hp,D3D12_HEAP_FLAG_NONE,&d,state,nullptr,IID_PPV_ARGS(&out)),"texture");
    return out;
}

static void draw(Context& ctx,const std::string& name,const std::string& shader,UINT tex,UINT samp) {
    auto device=ctx.device.Get();
    D3D12_ROOT_PARAMETER1 constant{};
    constant.ParameterType=D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS;
    constant.Constants={500,0,2};
    D3D12_VERSIONED_ROOT_SIGNATURE_DESC root_desc{};
    root_desc.Version=D3D_ROOT_SIGNATURE_VERSION_1_1;
    root_desc.Desc_1_1.NumParameters=1;root_desc.Desc_1_1.pParameters=&constant;
    root_desc.Desc_1_1.Flags=D3D12_ROOT_SIGNATURE_FLAG_CBV_SRV_UAV_HEAP_DIRECTLY_INDEXED |
                             D3D12_ROOT_SIGNATURE_FLAG_SAMPLER_HEAP_DIRECTLY_INDEXED;
    ComPtr<ID3DBlob> blob;
    check(D3D12SerializeVersionedRootSignature(&root_desc,&blob,nullptr),"serialize directly indexed root");
    ComPtr<ID3D12RootSignature> root;
    check(device->CreateRootSignature(0,blob->GetBufferPointer(),blob->GetBufferSize(),IID_PPV_ARGS(&root)),"root");
    auto vs=read_shader("vertex"),ps=read_shader(shader);
    D3D12_GRAPHICS_PIPELINE_STATE_DESC pd{};
    pd.pRootSignature=root.Get();pd.VS={vs.data(),vs.size()};pd.PS={ps.data(),ps.size()};
    pd.RasterizerState.FillMode=D3D12_FILL_MODE_SOLID;pd.RasterizerState.CullMode=D3D12_CULL_MODE_NONE;
    pd.RasterizerState.DepthClipEnable=TRUE;
    pd.PrimitiveTopologyType=D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE;
    pd.SampleDesc.Count=1;pd.SampleMask=UINT_MAX;pd.NumRenderTargets=1;
    pd.RTVFormats[0]=DXGI_FORMAT_R32G32B32A32_FLOAT;
    pd.BlendState.RenderTarget[0].RenderTargetWriteMask=D3D12_COLOR_WRITE_ENABLE_ALL;
    ComPtr<ID3D12PipelineState> pipeline;
    check(device->CreateGraphicsPipelineState(&pd,IID_PPV_ARGS(&pipeline)),"pipeline");
    auto target=image(device,side,D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,D3D12_RESOURCE_STATE_RENDER_TARGET);
    std::array<ComPtr<ID3D12Resource>,2> textures,uploads;
    ComPtr<ID3D12DescriptorHeap> rtv,srv,samplers;
    D3D12_DESCRIPTOR_HEAP_DESC hd{D3D12_DESCRIPTOR_HEAP_TYPE_RTV,1,D3D12_DESCRIPTOR_HEAP_FLAG_NONE,0};
    check(device->CreateDescriptorHeap(&hd,IID_PPV_ARGS(&rtv)),"RTV heap");
    auto rt=rtv->GetCPUDescriptorHandleForHeapStart();device->CreateRenderTargetView(target.Get(),nullptr,rt);
    hd={D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,2,D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,0};
    check(device->CreateDescriptorHeap(&hd,IID_PPV_ARGS(&srv)),"resource heap");
    hd.Type=D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER;
    check(device->CreateDescriptorHeap(&hd,IID_PPV_ARGS(&samplers)),"sampler heap");
    auto readback=buffer(device,bytes,D3D12_HEAP_TYPE_READBACK,D3D12_RESOURCE_STATE_COPY_DEST);
    ComPtr<ID3D12CommandAllocator> allocator;ComPtr<ID3D12GraphicsCommandList> list;
    check(device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT,IID_PPV_ARGS(&allocator)),"allocator");
    check(device->CreateCommandList(0,D3D12_COMMAND_LIST_TYPE_DIRECT,allocator.Get(),pipeline.Get(),IID_PPV_ARGS(&list)),"list");
    auto sr=srv->GetCPUDescriptorHandleForHeapStart(),sa=samplers->GetCPUDescriptorHandleForHeapStart();
    for(UINT i=0;i<2;++i){
        textures[i]=image(device,2,D3D12_RESOURCE_FLAG_NONE,D3D12_RESOURCE_STATE_COPY_DEST);
        uploads[i]=buffer(device,512,D3D12_HEAP_TYPE_UPLOAD,D3D12_RESOURCE_STATE_GENERIC_READ);
        std::array<float,128> data{};
        constexpr float colors[4][4]={{1,0,0,1},{0,1,0,1},{0,0,1,1},{1,1,1,1}};
        for(UINT y=0;y<2;++y)for(UINT x=0;x<2;++x)for(UINT c=0;c<4;++c)
            data[y*64+x*4+c]=colors[y*2+x][c]+(c<3?2.0f*i:0.0f);
        upload(uploads[i].Get(),data.data(),sizeof(data));
        D3D12_TEXTURE_COPY_LOCATION from{},to{};
        from.pResource=uploads[i].Get();from.Type=D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
        from.PlacedFootprint={0,{DXGI_FORMAT_R32G32B32A32_FLOAT,2,2,1,256}};
        to.pResource=textures[i].Get();to.Type=D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
        list->CopyTextureRegion(&to,0,0,0,&from,nullptr);
        transition(list.Get(),textures[i].Get(),D3D12_RESOURCE_STATE_COPY_DEST,D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE);
        D3D12_SHADER_RESOURCE_VIEW_DESC view{};
        view.Format=DXGI_FORMAT_R32G32B32A32_FLOAT;view.ViewDimension=D3D12_SRV_DIMENSION_TEXTURE2D;
        view.Shader4ComponentMapping=D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;view.Texture2D.MipLevels=1;
        device->CreateShaderResourceView(textures[i].Get(),&view,sr);
        sr.ptr+=device->GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV);
        D3D12_SAMPLER_DESC sd{};
        sd.Filter=i?D3D12_FILTER_MIN_MAG_MIP_LINEAR:D3D12_FILTER_MIN_MAG_MIP_POINT;
        sd.AddressU=sd.AddressV=sd.AddressW=D3D12_TEXTURE_ADDRESS_MODE_CLAMP;
        sd.MaxAnisotropy=1;sd.ComparisonFunc=D3D12_COMPARISON_FUNC_ALWAYS;sd.MaxLOD=D3D12_FLOAT32_MAX;
        device->CreateSampler(&sd,sa);
        sa.ptr+=device->GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER);
    }
    const float guard[]{-1,-2,-3,-4};list->ClearRenderTargetView(rt,guard,0,nullptr);
    list->OMSetRenderTargets(1,&rt,FALSE,nullptr);
    ID3D12DescriptorHeap* heaps[]{srv.Get(),samplers.Get()};list->SetDescriptorHeaps(2,heaps);
    list->SetGraphicsRootSignature(root.Get());
    UINT args[]{tex,samp};list->SetGraphicsRoot32BitConstants(0,2,args,0);
    D3D12_VIEWPORT vp{0,0,side,side,0,1};D3D12_RECT rect{3,2,12,11};
    list->RSSetViewports(1,&vp);list->RSSetScissorRects(1,&rect);
    list->IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
    list->DrawInstanced(3,1,0,0);
    transition(list.Get(),target.Get(),D3D12_RESOURCE_STATE_RENDER_TARGET,D3D12_RESOURCE_STATE_COPY_SOURCE);
    D3D12_TEXTURE_COPY_LOCATION from{},to{};
    from.pResource=target.Get();from.Type=D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
    to.pResource=readback.Get();to.Type=D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
    to.PlacedFootprint={0,{DXGI_FORMAT_R32G32B32A32_FLOAT,side,side,1,pitch}};
    list->CopyTextureRegion(&to,0,0,0,&from,nullptr);
    submit_and_wait(device,ctx.queue.Get(),list.Get());write_buffer(readback.Get(),name,bytes);
}

int main(){
    FILE* report{};if(fopen_s(&report,"result.txt","wb"))return 2;
    int status=0;
    try{
        Context ctx(report);std::ifstream cases("cases.txt");
        std::string name,shader;UINT tex,samp;
        while(cases>>name>>shader>>tex>>samp){
            if(tex>1||samp>1)throw std::runtime_error("bad heap index");
            draw(ctx,name,shader,tex,samp);std::fprintf(report,"%s_pixels=256\n",name.c_str());
        }
        if(!cases.eof())throw std::runtime_error("invalid cases");
    }catch(const std::exception& e){std::fprintf(report,"error=%s\n",e.what());status=1;}
    return std::fclose(report)?2:status;
}
