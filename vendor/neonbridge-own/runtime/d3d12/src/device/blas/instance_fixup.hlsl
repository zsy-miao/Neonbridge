// The table and records contain GPU virtual addresses encoded least-significant
// DWORD first. All other fields keep their exact guest representation.
ByteAddressBuffer instances : register(t0);
ByteAddressBuffer addresses : register(t1);
RWByteAddressBuffer outputRecords : register(u0);
cbuffer Bounds : register(b0) { uint instanceCount; uint groupsX; }

[numthreads(64, 1, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    uint i = id.x + id.y * groupsX * 64;
    if (i >= instanceCount) return;
    uint base = i * 64;
    outputRecords.Store4(base, instances.Load4(base));
    outputRecords.Store4(base + 16, instances.Load4(base + 16));
    outputRecords.Store4(base + 32, instances.Load4(base + 32));
    uint4 last = instances.Load4(base + 48);
    uint lo = 0, hi = addresses.Load(0);
    while (lo < hi) {
        uint mid = lo + (hi - lo) / 2;
        uint4 entry = addresses.Load4(16 + mid * 16);
        if (all(entry.xy == last.zw)) { last.zw = entry.zw; break; }
        if (entry.y < last.w || (entry.y == last.w && entry.x < last.z)) lo = mid + 1;
        else hi = mid;
    }
    outputRecords.Store4(base + 48, last);
}
