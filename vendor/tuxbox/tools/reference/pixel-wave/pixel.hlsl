RWStructuredBuffer<uint4> pixels : register(u0);
RWByteAddressBuffer counter : register(u1);
RWStructuredBuffer<uint> records : register(u2);
float4 main(float4 p : SV_Position) : SV_Target {
    uint2 xy = uint2(p.xy);
    uint id = xy.y * 32u + xy.x;
    bool take = ((xy.x + 3u * xy.y) % 5u) != 0u;
    uint count = WaveActiveCountBits(take);
    uint rank = WavePrefixCountBits(take);
    uint base = 0;
    if (WaveIsFirstLane()) counter.InterlockedAdd(0, count, base);
    base = WaveReadLaneFirst(base);
    if (take && base + rank < 160u) records[base + rank] = id;
    uint self = IsHelperLane() ? 1u : 0u;
    uint helpers = self | (QuadReadAcrossX(self) << 1u)
                 | (QuadReadAcrossY(self) << 2u) | (QuadReadAcrossDiagonal(self) << 3u);
    pixels[id] = uint4(WaveGetLaneCount(), WaveGetLaneIndex(), helpers, take ? base + rank : 0xffffffffu);
    return float4(1, 0, 0, 1);
}
