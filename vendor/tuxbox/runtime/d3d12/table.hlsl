cbuffer Parameters : register(b0) {
    uint multiplier;
    uint increment;
};
StructuredBuffer<uint> source : register(t0);
Texture2D<float4> nullTexture : register(t1);
RWStructuredBuffer<uint> result : register(u0);
SamplerState pointSampler : register(s0);

[numthreads(8, 1, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    result[id.x] = source[id.x] * multiplier + increment
                 + asuint(nullTexture.SampleLevel(pointSampler, float2(0.5, 0.5), 0).x);
}
