// Experimental bounded TRI0/TRI1 -> procedural AABB conversion. A failed status
// invalidates the entire output; this shader does not itself block an AS build.
ByteAddressBuffer leaves : register(t0);
ByteAddressBuffer directory : register(t1);
RWByteAddressBuffer bounds : register(u0);
RWByteAddressBuffer status : register(u1);
cbuffer Sizes : register(b0) { uint leaf_bytes; uint bounds_bytes; uint count; uint directory_bytes; }

float outward(float value, bool upper) {
    uint bits = asuint(value);
    // Expand tiny values across zero by a normal amount. Subnormal bounds
    // could otherwise be flushed inward by a driver or traversal unit.
    if ((bits & 0x7fffffffu) < 0x35800000u)
        return asfloat(upper ? 0x35800000u : 0xb5800000u);
    bool positive = (bits & 0x80000000u) == 0;
    return asfloat(bits + ((positive == upper) ? 1u : 0xffffffffu));
}

[numthreads(64, 1, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    if (id.x >= count) return;
    if (count > bounds_bytes / 24 || directory_bytes < 16) {
        uint ignored; status.InterlockedOr(0, 1, ignored); return;
    }
    uint4 header = directory.Load4(0);
    if (header.x != 1 || header.y == 0 || header.w > leaf_bytes ||
        header.y > (directory_bytes - 16) / 16) {
        uint ignored; status.InterlockedOr(0, 1, ignored); return;
    }
    uint start = 16 + header.y * 16;
    if (header.z > (directory_bytes - start) / 16) {
        uint ignored; status.InterlockedOr(0, 1, ignored); return;
    }
    uint4 instance = directory.Load4(16);
    if (instance.z != 0 || instance.w != 0 || instance.x > header.z ||
        instance.y != count || instance.y > header.z - instance.x) {
        uint ignored; status.InterlockedOr(0, 1, ignored); return;
    }
    uint4 candidate = directory.Load4(start + (instance.x + id.x) * 16);
    uint offset = candidate.x, side = candidate.y;
    if (candidate.z != 0 || candidate.w != 0 || side > 1 || (offset & 63) != 0 ||
        offset > header.w || header.w - offset < 64) {
        uint ignored; status.InterlockedOr(0, 1, ignored); return;
    }
    uint descriptor = leaves.Load(offset + 48);
    uint second = leaves.Load(offset + 56);
    uint selectors = leaves.Load(offset + 60);
    bool paired = (descriptor & 0x80u) != 0;
    uint selector = (selectors >> (side * 8)) & 255;
    float3 a = asfloat(leaves.Load3(offset + (side == 0 ? 0 : 12)));
    float3 b = asfloat(leaves.Load3(offset + (side == 0 ? 12 : 36)));
    float3 c = asfloat(leaves.Load3(offset + 24));
    if ((descriptor & 0x7fu) != 0 || selectors >> 16 != 0 ||
        paired != (second != 0xffffffffu) || paired != ((selectors >> 8) != 0) ||
        (side == 1 && !paired) || (selector != 2 && selector != 4 && selector != 9) ||
        !all(isfinite(a)) || !all(isfinite(b)) || !all(isfinite(c))) {
        uint ignored; status.InterlockedOr(0, 2, ignored); return;
    }
    float3 lo = min(a, min(b, c)), hi = max(a, max(b, c));
    lo = float3(outward(lo.x, false), outward(lo.y, false), outward(lo.z, false));
    hi = float3(outward(hi.x, true), outward(hi.y, true), outward(hi.z, true));
    if (!all(isfinite(lo)) || !all(isfinite(hi))) {
        uint ignored; status.InterlockedOr(0, 2, ignored); return;
    }
    bounds.Store3(id.x * 24, asuint(lo));
    bounds.Store3(id.x * 24 + 12, asuint(hi));
}
