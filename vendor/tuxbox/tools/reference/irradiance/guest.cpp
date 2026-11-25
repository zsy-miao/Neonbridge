#include "../independent-shaders/context.h"
using namespace shader_control;
static void run(Context &ctx, UINT count, float depth, UINT width, bool gradient_sample, const char *name) {
 auto dev=ctx.device.Get();
 auto code=read_shader("shader");
 auto raw=read_shader("native");
 ComPtr<ID3D12RootSignature> root;
 check(dev->CreateRootSignature(0,raw.data(),raw.size(),IID_PPV_ARGS(&root)),"root");
 D3D12_COMPUTE_PIPELINE_STATE_DESC pd{};pd.pRootSignature=root.Get();pd.CS={code.data(),code.size()};
 ComPtr<ID3D12PipelineState> p;check(dev->CreateComputePipelineState(&pd,IID_PPV_ARGS(&p)),"PSO");
 ComPtr<ID3D12CommandAllocator> a;ComPtr<ID3D12GraphicsCommandList> l;
 check(dev->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT,IID_PPV_ARGS(&a)),"allocator");
 check(dev->CreateCommandList(0,D3D12_COMMAND_LIST_TYPE_DIRECT,a.Get(),p.Get(),IID_PPV_ARGS(&l)),"list");
 D3D12_DESCRIPTOR_HEAP_DESC hd{D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,96,D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,0};
 ComPtr<ID3D12DescriptorHeap> heap;check(dev->CreateDescriptorHeap(&hd,IID_PPV_ARGS(&heap)),"heap");
 D3D12_DESCRIPTOR_HEAP_DESC sd{D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER,1,D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,0};
 ComPtr<ID3D12DescriptorHeap> sampler;check(dev->CreateDescriptorHeap(&sd,IID_PPV_ARGS(&sampler)),"sampler heap");
 D3D12_SAMPLER_DESC samp{};samp.Filter=D3D12_FILTER_MIN_MAG_MIP_POINT;samp.AddressU=samp.AddressV=samp.AddressW=D3D12_TEXTURE_ADDRESS_MODE_CLAMP;samp.MaxLOD=D3D12_FLOAT32_MAX;dev->CreateSampler(&samp,sampler->GetCPUDescriptorHandleForHeapStart());
 UINT step=dev->GetDescriptorHandleIncrementSize(hd.Type);
 auto cpu=[&](UINT i){return D3D12_CPU_DESCRIPTOR_HANDLE{heap->GetCPUDescriptorHandleForHeapStart().ptr+step*i};};
 auto gpu=[&](UINT i){return D3D12_GPU_DESCRIPTOR_HANDLE{heap->GetGPUDescriptorHandleForHeapStart().ptr+step*i};};
 std::array<UINT,128> cb{};cb[13]=width;cb[18]=1;cb[64]=0x3f800000u;cb[33]=1;cb[34]=1;
 auto cbv=buffer(dev,512,D3D12_HEAP_TYPE_UPLOAD,D3D12_RESOURCE_STATE_GENERIC_READ);upload(cbv.Get(),cb.data(),512);
 D3D12_CONSTANT_BUFFER_VIEW_DESC cv{cbv->GetGPUVirtualAddress(),512};dev->CreateConstantBufferView(&cv,cpu(64));
 auto s=buffer(dev,16,D3D12_HEAP_TYPE_UPLOAD,D3D12_RESOURCE_STATE_GENERIC_READ);std::array<UINT,4> sb{count,0,0,0};upload(s.Get(),sb.data(),16);
 D3D12_SHADER_RESOURCE_VIEW_DESC sv{};sv.ViewDimension=D3D12_SRV_DIMENSION_BUFFER;sv.Shader4ComponentMapping=D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;sv.Buffer.NumElements=4;sv.Buffer.StructureByteStride=4;dev->CreateShaderResourceView(s.Get(),&sv,cpu(1));
 D3D12_HEAP_PROPERTIES hp{};hp.Type=D3D12_HEAP_TYPE_DEFAULT;hp.CreationNodeMask=hp.VisibleNodeMask=1;
 D3D12_RESOURCE_DESC td{};td.Dimension=D3D12_RESOURCE_DIMENSION_TEXTURE2D;td.Width=8;td.Height=8;td.DepthOrArraySize=1;td.MipLevels=1;td.Format=DXGI_FORMAT_R32G32B32A32_FLOAT;td.SampleDesc.Count=1;td.Layout=D3D12_TEXTURE_LAYOUT_UNKNOWN;
 std::array<ComPtr<ID3D12Resource>,5> tex,staging;constexpr UINT pitch=256;
 for(UINT k=0;k<5;k++){
  td.Flags=k>=3?D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS:D3D12_RESOURCE_FLAG_NONE;
  check(dev->CreateCommittedResource(&hp,D3D12_HEAP_FLAG_NONE,&td,D3D12_RESOURCE_STATE_COPY_DEST,nullptr,IID_PPV_ARGS(&tex[k])),"texture");
  std::array<float,512> data{};
  for(UINT y=0;y<8;y++)for(UINT x=0;x<8;x++) {
   float val=k==0?depth:k==1?(gradient_sample ? float(x+1)*0.0625f : 0.5f):k==2?(gradient_sample ? 1.0f : float(x+1)*0.0625f):-77.0f;
   for(UINT c=0;c<4;c++)data[y*64+x*4+c]=val*(k==1?float(c+1):1.0f);
  }
  staging[k]=buffer(dev,sizeof(data),D3D12_HEAP_TYPE_UPLOAD,D3D12_RESOURCE_STATE_GENERIC_READ);upload(staging[k].Get(),data.data(),sizeof(data));
  D3D12_TEXTURE_COPY_LOCATION src{},dst{};src.pResource=staging[k].Get();src.Type=D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;src.PlacedFootprint.Footprint={td.Format,8,8,1,pitch};dst.pResource=tex[k].Get();dst.Type=D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;l->CopyTextureRegion(&dst,0,0,0,&src,nullptr);
  transition(l.Get(),tex[k].Get(),D3D12_RESOURCE_STATE_COPY_DEST,k>=3?D3D12_RESOURCE_STATE_UNORDERED_ACCESS:D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
  if(k>=3){D3D12_UNORDERED_ACCESS_VIEW_DESC u{};u.Format=td.Format;u.ViewDimension=D3D12_UAV_DIMENSION_TEXTURE2D;dev->CreateUnorderedAccessView(tex[k].Get(),nullptr,&u,cpu(80+k-3));}
  else {D3D12_SHADER_RESOURCE_VIEW_DESC v{};v.Format=td.Format;v.Shader4ComponentMapping=D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;v.ViewDimension=D3D12_SRV_DIMENSION_TEXTURE2D;v.Texture2D.MipLevels=1;dev->CreateShaderResourceView(tex[k].Get(),&v,cpu(k==0?0:k+1));}
 }
 ID3D12DescriptorHeap* heaps[]={heap.Get(),sampler.Get()};l->SetDescriptorHeaps(2,heaps);l->SetComputeRootSignature(root.Get());
 l->SetComputeRootDescriptorTable(0,gpu(0));l->SetComputeRootDescriptorTable(1,gpu(64));l->SetComputeRootDescriptorTable(2,sampler->GetGPUDescriptorHandleForHeapStart());l->SetComputeRootDescriptorTable(3,gpu(80));
 l->Dispatch(1,1,1);
 transition(l.Get(),tex[3].Get(),D3D12_RESOURCE_STATE_UNORDERED_ACCESS,D3D12_RESOURCE_STATE_COPY_SOURCE);
 auto r=buffer(dev,pitch*8,D3D12_HEAP_TYPE_READBACK,D3D12_RESOURCE_STATE_COPY_DEST);
 D3D12_TEXTURE_COPY_LOCATION src{},dst{};src.pResource=tex[3].Get();src.Type=D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;dst.pResource=r.Get();dst.Type=D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;dst.PlacedFootprint.Footprint={td.Format,8,8,1,pitch};l->CopyTextureRegion(&dst,0,0,0,&src,nullptr);
 transition(l.Get(),tex[4].Get(),D3D12_RESOURCE_STATE_UNORDERED_ACCESS,D3D12_RESOURCE_STATE_COPY_SOURCE);
 auto r2=buffer(dev,pitch*8,D3D12_HEAP_TYPE_READBACK,D3D12_RESOURCE_STATE_COPY_DEST);
 src.pResource=tex[4].Get();dst.pResource=r2.Get();l->CopyTextureRegion(&dst,0,0,0,&src,nullptr);
 submit_and_wait(dev,ctx.queue.Get(),l.Get());write_buffer(r.Get(),name,pitch*8);write_buffer(r2.Get(),std::string(name)+"-u1",pitch*8);
}
int main(){FILE *f{};if(fopen_s(&f,"result.txt","wb"))return 2;try{Context ctx(f);run(ctx,1,1.0f,8,false,"active");run(ctx,1,1.0f,1,false,"width1");run(ctx,1,1.0f,2,false,"width2");run(ctx,1,1.0f,3,false,"width3");run(ctx,1,1.0f,5,false,"partial");run(ctx,1,1.0f,4,false,"quarter");run(ctx,0,1.0f,8,false,"empty");run(ctx,1,0.0f,8,false,"rejected");run(ctx,1,1.0f,3,true,"sampled");fclose(f);return 0;}catch(const std::exception&e){fprintf(f,"ERROR %s",e.what());fclose(f);return 1;}}
