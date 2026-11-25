#include "../rtz_triangle.hlsl"
ByteAddressBuffer input : register(t0);
RWByteAddressBuffer output : register(u0);
[numthreads(64,1,1)]
void main(uint3 id : SV_DispatchThreadID) {
    uint node[32];
    [unroll] for (uint i=0; i<32; i++) node[i]=input.Load(i*4);
    float3 origin=asfloat(input.Load3(128));
    origin.x += (float)(id.x & 1023u)*0.0001f;
    float3 direction=asfloat(input.Load3(140));
    float4 result=tb_bvh_triangle_rtz(node,0,origin,direction);
    output.Store4(id.x*16,asuint(result));
}
