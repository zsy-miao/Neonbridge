cbuffer Film : register(b0) { uint4 cb[48]; };
RWBuffer<uint> result : register(u0);
[numthreads(48, 1, 1)]
void main(uint i : SV_GroupIndex) {
    uint4 v = cb[i];
    result[4*i+0] = v.x;
    result[4*i+1] = v.y;
    result[4*i+2] = v.z;
    result[4*i+3] = v.w;
}
