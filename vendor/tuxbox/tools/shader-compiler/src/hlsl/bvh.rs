//! image_bvh_intersect_ray node test (five-star-recomp rdna_bvh.hlsli, GPL-3.0, credit).
//! On by default (TB_BVH_RAW=0 off): uses it for BVH T#s that live in a heap slot.

pub(super) fn on() -> bool {
    std::env::var("TB_BVH_RAW").map_or(true, |v| v.trim() != "0")
}

/// Reads behind MemLoad: the heap slot's raw view (tb_bvh_root = ~0u) and the
/// runtime's root raw views (10cbdc6: the k-th SRV root parameter at
/// t5+k space32766 over the title memory at its address, the T# base; no
/// length is passed, so GetDimensions bounds the read; a parameter that is no
/// placed AS reads the null sink's zeros).
pub(super) fn mem(heap: bool, roots: &std::collections::BTreeSet<u32>) -> String {
    let mut s = String::from("static uint tb_bvh_slot;
static uint tb_bvh_va;
static uint tb_bvh_root;
");
    for k in roots {
        s += &format!("ByteAddressBuffer tb_bvh_raw{k} : register(t{}, space32766);
", 5 + k);
    }
    s += "uint tb_bvh_mem(uint at) {
    uint n;
";
    for k in roots {
        s += &format!("    if (tb_bvh_root == {k}u) {{ tb_bvh_raw{k}.GetDimensions(n); return at + 4u <= n ? tb_bvh_raw{k}.Load(at & ~3u) : 0u; }}
");
    }
    if heap {
        s += "    if (tb_bvh_root == 0xffffffffu) { ByteAddressBuffer b = ResourceDescriptorHeap[tb_bvh_slot];
        return at + 4u <= tb_slot_views.Load(tb_bvh_slot * 16u + 8u) ? b.Load(at & ~3u) : 0u; }
";
    }
    s + "    return 0u;
}
"
}

pub(super) const HELPERS: &str = r#"// Copied from five-star-recomp src/FiveStar.Shaders/Translation/Runtime/rdna_bvh.hlsli
// (GPL-3.0-or-later, Five Star Recomp contributors; AMD GPURT IntersectCommon.hlsl model),
// with credit. Changes: MemLoad reads the BVH's raw heap view (runtime TUXBOX_BVH_RAW_SRV,
// 404c70c), node offsets are relative to the T# base, null child pointers and a zero
// triangle denominator miss (so zeroed memory cannot loop or hit).
uint MemLoad(uint a, uint o) { return tb_bvh_mem(a + o + tb_bvh_va); }
static const uint BVH_INVALID = 0xffffffffu;

float bvh_maxnum(float a, float b) { return isnan(a) ? b : isnan(b) ? a : max(a, b); }
float bvh_minnum(float a, float b) { return isnan(a) ? b : isnan(b) ? a : min(a, b); }
float bvh_div(float a, float b) { return (float)((double)a / (double)b); }   // correctly rounded f32 quotient

uint4 bvh_load4(uint addr) {
    return uint4(MemLoad(addr, 0u), MemLoad(addr, 4u), MemLoad(addr, 8u), MemLoad(addr, 12u));
}
float3 bvh_load3f(uint addr) { return asfloat(uint3(MemLoad(addr, 0u), MemLoad(addr, 4u), MemLoad(addr, 8u))); }

// GPURT fast_intersect_bbox: returns (min_t, max_t) after clamping to [0, extent]; NaN interval -> (inf, -inf)
float2 bvh_box(float3 org, float3 inv, float3 bmin, float3 bmax, float extent) {
    precise float3 rmin = bmin - org;
    precise float3 rmax = bmax - org;
    precise float3 tpmin = rmin * inv;
    precise float3 tpmax = rmax * inv;
    float3 lo, hi;
    lo.x = inv.x >= 0.0 ? tpmin.x : tpmax.x;  hi.x = inv.x >= 0.0 ? tpmax.x : tpmin.x;
    lo.y = inv.y >= 0.0 ? tpmin.y : tpmax.y;  hi.y = inv.y >= 0.0 ? tpmax.y : tpmin.y;
    lo.z = inv.z >= 0.0 ? tpmin.z : tpmax.z;  hi.z = inv.z >= 0.0 ? tpmax.z : tpmin.z;
    float tmin_i = bvh_maxnum(bvh_maxnum(lo.x, lo.y), lo.z);
    float tmax_i = bvh_minnum(bvh_minnum(hi.x, hi.y), hi.z);
    float2 r = float2(bvh_maxnum(tmin_i, 0.0), bvh_minnum(tmax_i, extent));
    if (isnan(tmin_i) || isnan(tmax_i)) r = float2(asfloat(0x7f800000u), asfloat(0xff800000u));
    return r;
}

// GPURT SORT(childA, childB, distA, distB): invalid children sink to the end, ties keep their order
void bvh_sort(inout uint ca, inout uint cb, inout float da, inout float db) {
    if ((cb != BVH_INVALID && db < da) || ca == BVH_INVALID) {
        float t0 = da; uint t1 = ca; ca = cb; da = db; cb = t1; db = t0;
    }
}

// Children of a box node. Written out per child rather than as [unroll] loops: each loop dxc unrolls costs a pass over
// the whole function, and a program can inline this thousands of times.
void bvh_child16(uint a, out float3 bmin, out float3 bmax) {
    // per child 3 dwords of f16 pairs: (minx|miny<<16, minz|maxx<<16, maxy|maxz<<16)
    uint d0 = MemLoad(a, 0u), d1 = MemLoad(a, 4u), d2 = MemLoad(a, 8u);
    bmin = float3(f16tof32(d0), f16tof32(d0 >> 16), f16tof32(d1));
    bmax = float3(f16tof32(d1 >> 16), f16tof32(d2), f16tof32(d2 >> 16));
}
void bvh_child32(uint a, out float3 bmin, out float3 bmax) {
    bmin = bvh_load3f(a);
    bmax = bvh_load3f(a + 12u);
}
uint bvh_child_hit(float3 org, float3 inv, float3 bmin, float3 bmax, float extent, float gf, uint child, out float key) {
    float2 s = bvh_box(org, inv, bmin, bmax, extent);
    precise float lim = s.y * gf;
    key = s.x;                            // closest-entry sort key (min_t)
    return (child != 0u && s.x <= lim) ? child : BVH_INVALID;
}

uint4 bvh_box_node(uint addr, bool box16, float extent, float3 org, float3 inv, uint grow, bool sort) {
    uint4 child = bvh_load4(addr);
    float3 bmin[4], bmax[4];
    if (box16) {
        // 64-byte node: 4 child pointers, then 12 bytes per child at 16 + 12*i
        bvh_child16(addr + 16u, bmin[0], bmax[0]);
        bvh_child16(addr + 28u, bmin[1], bmax[1]);
        bvh_child16(addr + 40u, bmin[2], bmax[2]);
        bvh_child16(addr + 52u, bmin[3], bmax[3]);
    } else {
        // 128-byte node: 4 child pointers, then per child min.xyz, max.xyz (f32) at 16 + 24*i
        bvh_child32(addr + 16u, bmin[0], bmax[0]);
        bvh_child32(addr + 40u, bmin[1], bmax[1]);
        bvh_child32(addr + 64u, bmin[2], bmax[2]);
        bvh_child32(addr + 88u, bmin[3], bmax[3]);
    }
    // growth factor 1 + grow * 2^-24 (one f32 multiply, one f32 add, as GPURT)
    precise float ge = (float)grow * 5.960464478e-8;
    precise float gf = 1.0 + ge;
    uint c[4]; float k[4];
    c[0] = bvh_child_hit(org, inv, bmin[0], bmax[0], extent, gf, child.x, k[0]);
    c[1] = bvh_child_hit(org, inv, bmin[1], bmax[1], extent, gf, child.y, k[1]);
    c[2] = bvh_child_hit(org, inv, bmin[2], bmax[2], extent, gf, child.z, k[2]);
    c[3] = bvh_child_hit(org, inv, bmin[3], bmax[3], extent, gf, child.w, k[3]);
    if (sort) {
        bvh_sort(c[0], c[2], k[0], k[2]);
        bvh_sort(c[1], c[3], k[1], k[3]);
        bvh_sort(c[0], c[1], k[0], k[1]);
        bvh_sort(c[2], c[3], k[2], k[3]);
        bvh_sort(c[1], c[2], k[1], k[2]);
    }
    return uint4(c[0], c[1], c[2], c[3]);
}

uint4 bvh_triangle_node(uint addr, uint type, bool return_ij, float3 org, float3 dir) {
    // 64-byte node: V0 @0, V1 @12, V2 @24, V3 @36, (V4 / sideband @48), triangle_id @60.
    // Vertex triple per node type (GPURT Common.hlsl CalcTriangleCompressionVertexIndices table 0x210/0x231/0x432/0x042)
    uint map = type == 0 ? 0x210u : type == 1 ? 0x231u : type == 2 ? 0x432u : 0x042u;
    float3 v1 = bvh_load3f(addr + 12u * (map & 0xfu));
    float3 v2 = bvh_load3f(addr + 12u * ((map >> 4) & 0xfu));
    float3 v3 = bvh_load3f(addr + 12u * ((map >> 8) & 0xfu));
    uint tri_id = MemLoad(addr, 60u);

    // GPURT fast_intersect_triangle
    precise float3 e1 = v2 - v1;
    precise float3 e2 = v3 - v1;
    precise float3 e3 = org - v1;
    precise float3 s1, s2;
    { precise float a = dir.y * e2.z; precise float b = dir.z * e2.y; s1.x = a - b; }   // s1 = cross(dir, e2)
    { precise float a = dir.z * e2.x; precise float b = dir.x * e2.z; s1.y = a - b; }
    { precise float a = dir.x * e2.y; precise float b = dir.y * e2.x; s1.z = a - b; }
    { precise float a = e3.y * e1.z; precise float b = e3.z * e1.y; s2.x = a - b; }    // s2 = cross(e3, e1)
    { precise float a = e3.z * e1.x; precise float b = e3.x * e1.z; s2.y = a - b; }
    { precise float a = e3.x * e1.y; precise float b = e3.y * e1.x; s2.z = a - b; }
    precise float4 r;
    { precise float a = e2.x * s2.x; precise float b = e2.y * s2.y; precise float c = e2.z * s2.z; precise float ab = a + b; r.x = ab + c; }   // t_num
    { precise float a = s1.x * e1.x; precise float b = s1.y * e1.y; precise float c = s1.z * e1.z; precise float ab = a + b; r.y = ab + c; }   // t_denom
    { precise float a = e3.x * s1.x; precise float b = e3.y * s1.y; precise float c = e3.z * s1.z; precise float ab = a + b; r.z = ab + c; }   // I num
    { precise float a = dir.x * s2.x; precise float b = dir.y * s2.y; precise float c = dir.z * s2.z; precise float ab = a + b; r.w = ab + c; } // J num
    float t = bvh_div(r.x, r.y), u = bvh_div(r.z, r.y), v = bvh_div(r.w, r.y);
    precise float uv = u + v;
    bool miss = !(r.y != 0.0) || (u < 0.0) || (u > 1.0) || (v < 0.0) || (uv > 1.0) || (t < 0.0);
    if (miss) { r.x = asfloat(0x7f800000u); r.y = 1.0; }
    if (!return_ij)   // triangle_return_mode 0 (unverified layout; unused by the game)
        return uint4(asuint(r.x), asuint(r.y), tri_id, miss ? 0u : 1u);
    // GPURT SwizzleBarycentrics: triangle_id byte per node type, bits [1:0] = I source vertex, [3:2] = J source
    precise float b0a = r.y - r.z;
    precise float b0 = b0a - r.w;
    float bary[4] = { b0, r.z, r.w, 0.0 };   // index 3 is out of range in GPURT (undefined); 0 here
    uint sh = 8u * type;
    return uint4(asuint(r.x), asuint(r.y), asuint(bary[(tri_id >> sh) & 3u]), asuint(bary[(tri_id >> (sh + 2u)) & 3u]));
}

// tdesc = T# words; node_lo/node_hi = node pointer (node_hi = 0 for the 32-bit form); returns the 4 result dwords.
uint4 bvh_intersect_ray(uint4 tdesc, uint node_lo, uint node_hi, float extent, float3 org, float3 dir, float3 inv_dir) {
    uint type = node_lo & 7u;
    uint addr = (node_lo & ~7u) << 3; // offset from the raw view, which starts at the T# base
    if (type >= 6u) return uint4(BVH_INVALID, BVH_INVALID, BVH_INVALID, BVH_INVALID);   // instance / user node
    if (type >= 4u)
        return bvh_box_node(addr, type == 4u, extent, org, inv_dir, (tdesc.y >> 23) & 0xffu, (tdesc.y >> 31) != 0);
    return bvh_triangle_node(addr, type, ((tdesc.w >> 24) & 1u) != 0, org, dir);
}
"#;
