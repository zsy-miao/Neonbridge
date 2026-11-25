RWStructuredBuffer<uint> counts : register(u0);

[numthreads(1, 1, 1)]
void main(uint3 group : SV_GroupID)
{
    if (all(group < uint3(5, 4, 4)))
        InterlockedAdd(counts[group.x + 5 * (group.y + 4 * group.z)], 1);
    else
        InterlockedAdd(counts[80], 1);
}
