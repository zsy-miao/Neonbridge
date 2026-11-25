cbuffer Parameters : register(b5, space2) {
    uint increment;
    uint multiplier;
    uint first;
    uint count;
};
RWStructuredBuffer<uint> result : register(u3, space4);
[numthreads(8, 1, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    if (id.x < count) {
        result[first + id.x] = id.x * multiplier + increment;
    }
}

[numthreads(8, 1, 1)]
void accumulate(uint3 id : SV_DispatchThreadID) {
    if (id.x < count) {
        result[first + id.x] = result[first + id.x] * multiplier + increment;
    }
}
