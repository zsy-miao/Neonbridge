cbuffer Params : register(b0) {
    uint key;
    uint salt;
};
StructuredBuffer<uint> source : register(t0);
RWStructuredBuffer<uint> output : register(u0);

[numthreads(1, 1, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    output[id.x] = source[id.x] * 131u + key * 17u + salt;
}
