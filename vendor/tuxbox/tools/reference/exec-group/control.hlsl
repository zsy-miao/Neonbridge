RWBuffer<uint> output : register(u0);
[numthreads(8, 8, 1)]
void main(uint3 group : SV_GroupID, uint3 thread : SV_GroupThreadID) {
    uint lane = group.x * 64 + thread.y * 8 + thread.x;
    uint v0 = 11u + lane, v1 = 81u + lane, v2 = 114u + lane;
    uint exec_lo = (lane & 1u) == 0u;
    if (exec_lo & 1u) v0 = v0 + 17u;
    if (exec_lo & 1u) v1 = v0 ^ 0x9abcu;
    output[lane * 3 + 1] = v1;
    if (exec_lo & 1u) v1 = v1 + 53u;
    if (exec_lo & 1u) v2 = output[lane * 3 + 1];
    exec_lo = (lane % 5u) == 2u;
    if (exec_lo & 1u) v0 = v2 + 73u;
    if (exec_lo & 1u) v1 = v0 ^ v1;
    output[lane * 3] = v0;
    output[lane * 3 + 1] = v1;
    output[lane * 3 + 2] = v2;
}
