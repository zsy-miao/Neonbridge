cbuffer Route : register(b1, space2) {
    uint seed;
    uint rowCount;
}
RWStructuredBuffer<uint> course : register(u2, space1);

[numthreads(8, 1, 1)]
void main(uint3 thread : SV_DispatchThreadID) {
    uint row = thread.x;
    if (row >= rowCount) return;
    uint beat = (row + seed) & 3u;
    uint coin = beat == 3 ? 1 : beat;
    uint mixed = (row + seed) * 747796405u + 2891336453u;
    mixed ^= mixed >> 16;
    uint barrier = (coin + 1 + (mixed & 1)) % 3;
    course[row] = coin | (barrier << 8);
}
