#include "../independent-shaders/context.h"
#include <array>
#include <algorithm>
using namespace shader_control;
constexpr UINT side=32, count=side*side, pixel_words=count*4, slots=160, guard=16;
static ComPtr<ID3D12Resource> surface(ID3D12Device* d) {
    D3D12_HEAP_PROPERTIES hp{}; hp.Type=D3D12_HEAP_TYPE_DEFAULT; hp.CreationNodeMask=hp.VisibleNodeMask=1;
    D3D12_RESOURCE_DESC desc{}; desc.Dimension=D3D12_RESOURCE_DIMENSION_TEXTURE2D; desc.Width=desc.Height=side;
    desc.DepthOrArraySize=desc.MipLevels=desc.SampleDesc.Count=1;desc.Format=DXGI_FORMAT_R8G8B8A8_UNORM;
    desc.Flags=D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET;
    ComPtr<ID3D12Resource> out;check(d->CreateCommittedResource(&hp,D3D12_HEAP_FLAG_NONE,&desc,D3D12_RESOURCE_STATE_RENDER_TARGET,nullptr,IID_PPV_ARGS(&out)),"surface");return out;
}
int main(){
 FILE* f=nullptr; if(fopen_s(&f,"result.txt","wb"))return 2;
 try {
  Context ctx(f);auto d=ctx.device.Get(); auto vs=read_shader("vertex"),ps=read_shader("pixel");
  D3D12_ROOT_PARAMETER params[3]{}; for(UINT i=0;i<3;i++){params[i].ParameterType=D3D12_ROOT_PARAMETER_TYPE_UAV;params[i].Descriptor.ShaderRegister=i;params[i].ShaderVisibility=D3D12_SHADER_VISIBILITY_PIXEL;}
  D3D12_ROOT_SIGNATURE_DESC sd{};sd.NumParameters=3;sd.pParameters=params;
  ComPtr<ID3DBlob> blob;check(D3D12SerializeRootSignature(&sd,D3D_ROOT_SIGNATURE_VERSION_1,&blob,nullptr),"serialize");ComPtr<ID3D12RootSignature> root;
  check(d->CreateRootSignature(0,blob->GetBufferPointer(),blob->GetBufferSize(),IID_PPV_ARGS(&root)),"root");
  D3D12_GRAPHICS_PIPELINE_STATE_DESC pd{};pd.pRootSignature=root.Get();pd.VS={vs.data(),vs.size()};pd.PS={ps.data(),ps.size()};pd.PrimitiveTopologyType=D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE;
  pd.RasterizerState.FillMode=D3D12_FILL_MODE_SOLID;pd.RasterizerState.CullMode=D3D12_CULL_MODE_NONE;pd.RasterizerState.DepthClipEnable=TRUE;
  pd.BlendState.RenderTarget[0].RenderTargetWriteMask=D3D12_COLOR_WRITE_ENABLE_ALL;pd.SampleDesc.Count=1;pd.SampleMask=UINT_MAX;pd.NumRenderTargets=1;pd.RTVFormats[0]=DXGI_FORMAT_R8G8B8A8_UNORM;
  ComPtr<ID3D12PipelineState> pso;check(d->CreateGraphicsPipelineState(&pd,IID_PPV_ARGS(&pso)),"pipeline");
  auto target=surface(d);D3D12_DESCRIPTOR_HEAP_DESC hd{D3D12_DESCRIPTOR_HEAP_TYPE_RTV,1,D3D12_DESCRIPTOR_HEAP_FLAG_NONE,0};ComPtr<ID3D12DescriptorHeap> rtv;check(d->CreateDescriptorHeap(&hd,IID_PPV_ARGS(&rtv)),"rtv");auto handle=rtv->GetCPUDescriptorHandleForHeapStart();d->CreateRenderTargetView(target.Get(),nullptr,handle);
  constexpr UINT sizes[3]={pixel_words*4,4,(slots+guard)*4};ComPtr<ID3D12Resource> out[3],up[3],read[3];
  for(UINT i=0;i<3;i++){out[i]=buffer(d,sizes[i],D3D12_HEAP_TYPE_DEFAULT,D3D12_RESOURCE_STATE_COPY_DEST,D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);up[i]=buffer(d,sizes[i],D3D12_HEAP_TYPE_UPLOAD,D3D12_RESOURCE_STATE_GENERIC_READ);read[i]=buffer(d,sizes[i],D3D12_HEAP_TYPE_READBACK,D3D12_RESOURCE_STATE_COPY_DEST);std::vector<UINT> init(sizes[i]/4,i==1?0u:0xdeadbeefu);upload(up[i].Get(),init.data(),sizes[i]);}
  ComPtr<ID3D12CommandAllocator> alloc;ComPtr<ID3D12GraphicsCommandList> list;check(d->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT,IID_PPV_ARGS(&alloc)),"alloc");check(d->CreateCommandList(0,D3D12_COMMAND_LIST_TYPE_DIRECT,alloc.Get(),pso.Get(),IID_PPV_ARGS(&list)),"list");
  for(UINT i=0;i<3;i++){list->CopyBufferRegion(out[i].Get(),0,up[i].Get(),0,sizes[i]);transition(list.Get(),out[i].Get(),D3D12_RESOURCE_STATE_COPY_DEST,D3D12_RESOURCE_STATE_UNORDERED_ACCESS);}
  list->SetGraphicsRootSignature(root.Get());for(UINT i=0;i<3;i++)list->SetGraphicsRootUnorderedAccessView(i,out[i]->GetGPUVirtualAddress());list->OMSetRenderTargets(1,&handle,FALSE,nullptr);
  D3D12_VIEWPORT vp{0,0,float(side),float(side),0,1};D3D12_RECT rect{0,0,side,side};list->RSSetViewports(1,&vp);list->RSSetScissorRects(1,&rect);list->IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);list->DrawInstanced(3,1,0,0);
  for(UINT i=0;i<3;i++){transition(list.Get(),out[i].Get(),D3D12_RESOURCE_STATE_UNORDERED_ACCESS,D3D12_RESOURCE_STATE_COPY_SOURCE);list->CopyBufferRegion(read[i].Get(),0,out[i].Get(),0,sizes[i]);}
  submit_and_wait(d,ctx.queue.Get(),list.Get());for(UINT i=0;i<3;i++)write_buffer(read[i].Get(),"out"+std::to_string(i),sizes[i]);
 }catch(const std::exception& e){std::fprintf(f,"error=%s\n",e.what());std::fclose(f);return 1;}return std::fclose(f)?2:0;
}
