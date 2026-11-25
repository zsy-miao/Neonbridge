//! Differential tests: HLSL compiled by the console's shader compiler and
//! translated, against the same HLSL compiled for PC, both drawn (or
//! dispatched) on the host GPU with the same inputs.
//!
//! Needs `VICTORY_CONSOLE_DXC` (the console SDK's `dxc.exe`) and a PC
//! `dxc.exe` (`VICTORY_PC_DXC`, else the newest Windows SDK's); skipped
//! without them. Set `VICTORY_WARP=1` to draw on WARP.

use std::path::{Path, PathBuf};

use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use super::host::{Draw, Host, Root};
use super::{HostShader, RootType, translate};

const RS: &str = "RootFlags(ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT), \
                  RootConstants(num32BitConstants=4, b0), \
                  DescriptorTable(SRV(t0)), DescriptorTable(Sampler(s0)), \
                  StaticSampler(s1, filter=FILTER_MIN_MAG_MIP_POINT, \
                      addressU=TEXTURE_ADDRESS_WRAP, addressV=TEXTURE_ADDRESS_MIRROR), \
                  StaticSampler(s2, filter=FILTER_MIN_MAG_LINEAR_MIP_POINT, \
                      addressU=TEXTURE_ADDRESS_BORDER, addressV=TEXTURE_ADDRESS_CLAMP, \
                      borderColor=STATIC_BORDER_COLOR_OPAQUE_WHITE, mipLODBias=0.5)";

const COMMON: &str = r#"
cbuffer K : register(b0) { float4 k; };
Texture2D tex : register(t0);
SamplerState smp : register(s0);
struct V { float4 pos : SV_Position; float2 uv : TEXCOORD0; float4 col : COLOR0; };
[RootSignature(RS)]
V vs(float4 p : POSITION, float2 uv : TEXCOORD0) {
    V o;
    o.pos = p;
    o.uv = uv;
    o.col = float4(uv * k.xy, p.x * k.z, 1);
    return o;
}
"#;

/// Pixel shaders, each `ps`.
const CASES: &[(&str, &str)] = &[
    (
        "constant",
        "[RootSignature(RS)] float4 ps(V i) : SV_Target { return float4(0.25, 0.5, 1, 0.75); }",
    ),
    (
        "interpolated",
        "[RootSignature(RS)] float4 ps(V i) : SV_Target { return i.col * 2 - float4(i.uv, 0, 0); }",
    ),
    (
        "sample and branch",
        r#"[RootSignature(RS)] float4 ps(V i) : SV_Target {
            float4 t = tex.Sample(smp, i.uv);
            float4 r = t * i.col + k.w;
            if (i.uv.x > 0.5) r.xy = sin(r.yx * 3.0);
            else r.z = rcp(r.z + 2.0);
            return r;
        }"#,
    ),
    // A wave-wide range scalarises a per-pixel loop (as the title's light
    // lists do): the console's 64-lane sections become host wave operations;
    // the result is the same for any wave the pixel lands in.
    (
        "wave range",
        r#"[RootSignature(RS)] float4 ps(V i) : SV_Target {
            uint idx = uint(i.pos.x * 0.37 + i.pos.y * 0.21) & 15;
            uint lo = WaveActiveMin(idx), hi = WaveActiveMax(idx);
            float s = 0;
            for (uint n = lo; n <= hi; n++) if (n == idx) s += n * k.x;
            return float4(s, lo <= idx, hi >= idx, WaveActiveCountBits(true) > 0);
        }"#,
    ),
    (
        "uniform loop",
        r#"[RootSignature(RS)] float4 ps(V i) : SV_Target {
            float s = 0;
            for (uint n = 0; n < (asuint(k.x) & 7); n++) s += i.uv.y * n;
            return float4(s, 0, 0, 1);
        }"#,
    ),
    (
        "bit loop",
        r#"[RootSignature(RS)] float4 ps(V i) : SV_Target {
            uint2 p = uint2(i.pos.xy);
            uint s = p.y;
            for (uint n = 0; n < (p.x & 7); n++) s = (s ^ (s << 3)) + n;
            return float4(s & 255, 0, 0, 1);
        }"#,
    ),
    (
        "integer loop",
        r#"[RootSignature(RS)] float4 ps(V i) : SV_Target {
            uint2 p = uint2(i.pos.xy);
            uint s = 0;
            for (uint n = 0; n < (p.x & 7); n++) s += n;
            return float4(s, 0, 0, 1);
        }"#,
    ),
    (
        "loop",
        r#"[RootSignature(RS)] float4 ps(V i) : SV_Target {
            uint2 p = uint2(i.pos.xy);
            float s = 0;
            for (uint n = 0; n < (p.x & 7); n++) s += i.uv.y * n + k.x;
            return float4(s, 0, 0, 1);
        }"#,
    ),
    (
        "discard",
        r#"[RootSignature(RS)] float4 ps(V i) : SV_Target {
            uint2 p = uint2(i.pos.xy);
            if ((p.x ^ p.y) & 1) discard;
            return i.col;
        }"#,
    ),
    (
        "immediate samplers",
        r#"SamplerState point_wrap : register(s1);
        SamplerState linear_border : register(s2);
        [RootSignature(RS)] float4 ps(V i) : SV_Target {
            return tex.Sample(point_wrap, i.uv * 3 - 1) + 2 * tex.Sample(linear_border, i.uv * 2.5 - 0.7);
        }"#,
    ),
    (
        "loop, discard and loads",
        r#"[RootSignature(RS)] float4 ps(V i) : SV_Target {
            uint2 p = uint2(i.pos.xy);
            if ((p.x ^ p.y) & 1) discard;
            float s = 0;
            for (uint n = 0; n < (p.x & 7); n++) s += i.uv.y * n + k.x;
            float4 t = tex.Load(int3(p & 3, 0));
            return float4(s, frac(i.uv.x * 7.3), max(t.x, t.y) * k.y, sqrt(abs(i.col.x)) + exp2(i.uv.y));
        }"#,
    ),
    (
        "level of detail",
        r#"[RootSignature(RS)] float4 ps(V i) : SV_Target {
            return float4(tex.CalculateLevelOfDetail(smp, i.uv * 4),
                tex.CalculateLevelOfDetailUnclamped(smp, i.uv * 7), 0, 1);
        }"#,
    ),
    // 16-bit outputs and inputs (one value per dword with the 2020
    // compiler; the title's two per dword cannot be compiled
    // here) and the front face.
    (
        "16-bit varyings and front face",
        r#"struct V16 { float4 pos : SV_Position; half2 uv : TEXCOORD0; float4 col : COLOR0; half4 h : TEXCOORD1; };
        [RootSignature(RS)] V16 vs16(float4 p : POSITION, float2 uv : TEXCOORD0) {
            V16 o;
            o.pos = p;
            o.uv = half2(uv * k.xy);
            o.col = float4(uv, p.x * k.z, 1);
            o.h = half4(uv.yx * 3 - 1, k.w, p.y);
            return o;
        }
        [RootSignature(RS)] float4 ps(V16 i, bool f : SV_IsFrontFace) : SV_Target {
            float4 c = i.col * float4(i.uv, i.h.xy) + float4(i.h.zw, 0, 0);
            return f ? c : -c;
        }"#,
    ),
];

const CS_RS: &str = "RootConstants(num32BitConstants=4, b0), \
                     DescriptorTable(SRV(t0, numDescriptors=4)), DescriptorTable(Sampler(s0)), \
                     DescriptorTable(UAV(u0, numDescriptors=4)), CBV(b1), SRV(t4), \
                     DescriptorTable(SRV(t0, space=2, numDescriptors=9)), \
\
                     StaticSampler(s3, filter=FILTER_COMPARISON_MIN_MAG_LINEAR_MIP_POINT, \
                         comparisonFunc=COMPARISON_LESS_EQUAL, \
                         addressU=TEXTURE_ADDRESS_CLAMP, addressV=TEXTURE_ADDRESS_CLAMP)";

const CS_COMMON: &str = r#"
cbuffer K : register(b0) { float4 k; };
Texture2D tex : register(t0);
Buffer<float4> typed : register(t1);
struct S { float a; uint b; float2 c; };
StructuredBuffer<S> structured : register(t2);
ByteAddressBuffer raw : register(t3);
SamplerState smp : register(s0);
SamplerComparisonState cmp : register(s3);
RWByteAddressBuffer outb : register(u0);
RWTexture2D<float4> outt : register(u1);
cbuffer R : register(b1) { float4 table[16]; };
ByteAddressBuffer rootraw : register(t4);
// `raw`'s bytes in typed views (the formats the console's typed loads name).
Buffer<float4> unorm8x4 : register(t0, space2);
Buffer<float4> snorm16x2 : register(t1, space2);
Buffer<uint4> uint8x4 : register(t2, space2);
Buffer<float4> unorm10x3_2 : register(t3, space2);
Buffer<uint4> uint16x4 : register(t4, space2);
Buffer<float> unorm8 : register(t5, space2);
Buffer<float> half16 : register(t6, space2);
Buffer<float4> snorm8x4 : register(t7, space2);
RWTexture2D<uint> ut : register(u2);
TextureCube cubet : register(t8, space2);
#define FMT(numeric, data) ((numeric) << 4 | (data))
#define CS [RootSignature(RS)] [numthreads(8, 8, 1)] \
    void cs(uint3 g : SV_GroupID, uint3 t : SV_GroupThreadID, \
            uint3 d : SV_DispatchThreadID, uint i : SV_GroupIndex)
"#;

/// Compute shaders, each `cs`, dispatched as 2×2 groups of 8×8 threads
/// over a 16×16 texture and a 256-dword buffer.
const CS_CASES: &[(&str, &str)] = &[
    (
        "ids and stores",
        r#"CS {
            outb.Store(4 * (d.y * 16 + d.x), asuint(d.x * k.x + d.y));
            outt[d.xy] = float4(g.x, g.y, t.x, t.y + i);
        }"#,
    ),
    (
        "shared memory and barrier",
        r#"groupshared float lds[64];
        groupshared uint cnt;
        CS {
            if (i == 0) cnt = 0;
            lds[i] = d.x * k.x + d.y;
            GroupMemoryBarrierWithGroupSync();
            InterlockedAdd(cnt, i);
            float v = lds[63 - i] + lds[(i + 9) & 63];
            GroupMemoryBarrierWithGroupSync();
            outb.Store(4 * (d.y * 16 + d.x), asuint(v));
            outt[d.xy] = float4(v, cnt, 0, 1);
        }"#,
    ),
    (
        "sample and divergent store",
        r#"CS {
            float4 s = tex.SampleLevel(smp, (d.xy + 0.5) / 16.0, 0);
            if ((d.x ^ d.y) & 1) outt[d.xy] = s * k.y;
            else outb.Store(4 * (d.y * 16 + d.x), asuint(s.x + s.w));
        }"#,
    ),
    (
        "buffers and root views",
        r#"CS {
            float4 a = typed[i & 15];
            S s = structured[(i * 3) & 7];
            uint r = raw.Load(4 * (i & 31));
            float4 c = table[d.x & 15];
            uint q = rootraw.Load(4 * ((i + 5) & 31));
            outb.Store(4 * (d.y * 16 + d.x), asuint(a.x + a.w * s.a) ^ s.b ^ r ^ q);
            outt[d.xy] = c + float4(s.c, a.yz);
        }"#,
    ),
    // (The 2020 compiler branches around every narrowed region, so these
    // become branches, not guards.)
    (
        "conditional memory access",
        r#"CS {
            float v = d.x * k.x + d.y;
            if (i & 1) v = v * 3 + 1;
            if (i & 4) v += tex.Load(int3(d.xy & 3, 0)).y;
            if (i & 2) outb.Store(4 * (d.y * 16 + d.x), asuint(v));
            if (i & 8) outt[d.xy] = float4(v, 0, 0, 1);
        }"#,
    ),
    // Each side of a per-group branch leaves a compare in VCC; the select
    // after the join reads the joined mask (as `postfx_splines` does).
    (
        "select on joined compares",
        r#"CS {
            float v = d.x * k.x + d.y;
            float r;
            bool ok;
            [branch] if ((g.x ^ g.y) & 1) {
                r = sqrt(v) * k.z + sin(v * k.w);
                ok = r < 5;
            } else {
                r = v * k.y + 1;
                ok = r > -4;
            }
            float o = ok ? r : -1;
            outb.Store(4 * (d.y * 16 + d.x), asuint(o));
            outt[d.xy] = float4(o, r, v, 1);
        }"#,
    ),
    // One host wave of 32 lanes holds the group, as the console's wave of 64
    // does, so lane numbers agree with the PC compile.
    (
        "lane reads",
        r#"[RootSignature(RS)] [numthreads(8, 4, 1)]
        void cs(uint3 d : SV_DispatchThreadID) {
            float v = d.x * k.x + d.y;
            float w = WaveReadLaneAt(v, 5);
            uint lane = WaveGetLaneIndex();
            uint q = QuadReadAcrossX(lane * 10) + QuadReadAcrossDiagonal(lane);
            outb.Store(4 * (d.y * 16 + d.x), q);
            outt[d.xy] = float4(v, w, QuadReadAcrossY(v), lane);
        }"#,
    ),
    // Bytes and halves: SDWA operand and result selection, byte permutes,
    // f16 conversions.
    (
        "sub-dword operations",
        r#"CS {
            uint n = d.y * 16 + d.x;
            uint x = raw.Load(4 * (n & 31)), y = raw.Load(4 * ((n * 7) & 31));
            float a = float(x >> 16) + float((y >> 8) & 255) * k.x;
            float b = f16tof32(x >> 16) + f16tof32(y);
            uint p = (x & 0xff) | ((y & 0xff) << 8) | (x & 0xff0000) | ((y >> 8) << 24);
            uint q = f32tof16(a) | (f32tof16(b) << 16);
            int s = int(x << 24) >> 24;
            uint u = (x & 0xffff) + (y >> 16) + ((x >> 24) ^ (y & 0xffff)) + asuint(k.w) * 1000 + 77;
            outb.Store4(16 * n, uint4(asuint(a), asuint(b) ^ p, q, asuint(float(s)) + u));
        }"#,
    ),
    // 64-bit additions (VALU carries as lane masks) and explicit gradients.
    (
        "carries and gradients",
        r#"CS {
            uint n = d.y * 16 + d.x;
            uint64_t a = (uint64_t(raw.Load(4 * (n & 31))) << 32) | raw.Load(4 * ((n * 3) & 31));
            uint64_t b = (uint64_t(raw.Load(4 * ((n * 5) & 31))) << 32) | raw.Load(4 * ((n * 7) & 31));
            uint64_t s = a + b, u = a - b;
            float4 gr = tex.SampleGrad(smp, (d.xy + 0.5) / 16.0, float2(k.x, 0) * 0.1, float2(0, k.z) * 0.1);
            outb.Store4(16 * n, uint4(uint(s), uint(s >> 32) ^ uint(u), uint(u >> 32), asuint(gr.x + gr.y * 3)));
        }"#,
    ),
    // Comparison sampling at level 0 with an immediate comparison sampler.
    (
        "comparison sampling",
        r#"CS {
            float r = tex.SampleCmpLevelZero(cmp, (d.xy + 0.25) / 15.0, k.x * d.x / 16.0);
            outb.Store(4 * (d.y * 16 + d.x), asuint(r));
        }"#,
    ),
    // 16-bit texture results, two channels per register (`d16`).
    (
        "16-bit texture reads",
        r#"Texture2D<half4> htex : register(t0);
        CS {
            half4 a = htex.SampleLevel(smp, (d.xy + 0.5) / 16.0, 0);
            half3 b = htex.Load(int3(d.xy & 3, 0)).xyz;
            outb.Store4(16 * (d.y * 16 + d.x), uint4(asuint(float(a.x) + a.w),
                asuint(float(a.y) * a.z), asuint(float(b.x)), asuint(float(b.y) - b.z)));
        }"#,
    ),
    // Sizes: the console compiler computes them from descriptor bits
    // (width, height, levels, record counts).
    (
        "sizes from descriptors",
        r#"CS {
            uint w, h, l, n, b, ns, st;
            tex.GetDimensions(0, w, h, l);
            typed.GetDimensions(n);
            raw.GetDimensions(b);
            structured.GetDimensions(ns, st);
            // Read too: the translation takes the views' kinds from those reads.
            float r = tex.Load(int3(d.xy & 3, 0)).x + typed[i & 15].y + structured[i & 7].a;
            outb.Store4(16 * (d.y * 16 + d.x),
                uint4(w * 7 + h, l + asuint(r), n * 5 + ns * 3 + st, b + raw.Load(4 * (i & 31))));
        }"#,
    ),
    // Typed loads (`tbuffer_load_format_*`, the title's vertex
    // fetch) convert by the format the instruction names: the PC reads the
    // same bytes through typed views in those formats. Channels past the
    // format's read (0, 0, 0, 1); a byte format at unaligned offsets.
    (
        "typed loads",
        r#"CS {
            uint n = d.y * 16 + d.x, e = n & 31, f = n >> 5;
            uint4 a, b, c, dd, ee, ff, gg, hh;
        #ifdef CONSOLE
            a = raw.XB_TypedLoad4(4 * e, FMT(0, 10));
            b = raw.XB_TypedLoad4(4 * e, FMT(1, 5));
            c = raw.XB_TypedLoad4(4 * e, FMT(4, 10));
            dd = raw.XB_TypedLoad4(4 * e, FMT(0, 9));
            ee = raw.XB_TypedLoad4(8 * (e & 15), FMT(4, 12));
            ff = uint4(raw.XB_TypedLoad((3 * e) & 127, FMT(0, 1)), 0, 0, 0);
            gg = uint4(raw.XB_TypedLoad2(2 * e, FMT(7, 2)), 0, 0);
            hh = raw.XB_TypedLoad4(4 * e, FMT(1, 10));
        #else
            a = asuint(unorm8x4[e]);
            b = asuint(snorm16x2[e]);
            c = uint8x4[e];
            dd = asuint(unorm10x3_2[e]);
            ee = uint16x4[e & 15];
            ff = uint4(asuint(unorm8[(3 * e) & 127]), 0, 0, 0);
            gg = uint4(asuint(half16[e]), 0, 0, 0);
            hh = asuint(snorm8x4[e]);
        #endif
            uint4 v = f == 0 ? a : f == 1 ? b : f == 2 ? c : f == 3 ? dd
                : f == 4 ? ee : f == 5 ? ff : f == 6 ? gg : hh;
            outb.Store4(16 * n, v);
        }"#,
    ),
    // Wave operations: the console expands them into whole-wave sections
    // (DPP, `v_permlanex16`, lane reads) and lane masks read as numbers
    // (`s_bcnt1`, `s_ff1`, `v_mbcnt`). 32 threads: one wave on the console
    // (lanes 32-63 inactive) and on the host; the branch leaves lanes out.
    (
        "wave operations",
        r#"[RootSignature(RS)] [numthreads(8, 4, 1)]
        void cs(uint3 d : SV_DispatchThreadID, uint i : SV_GroupIndex) {
            uint n = d.y * 16 + d.x;
            float v = d.x * k.x + d.y;
            uint u = raw.Load(4 * (i & 31));
            uint4 o = 0;
            if (u & 3) {
                o.x = WaveActiveSum(u >> 8) + WaveActiveCountBits((u & 4) != 0);
                o.y = asuint(WaveActiveMax(v)) ^ WaveActiveMin(u) ^ WaveActiveBitOr(u)
                    ^ WaveActiveBitAnd(u | 1) ^ asuint(WaveActiveMin(v)) ^ WaveActiveMax(int(u));
                o.z = WavePrefixSum(u & 255) + WavePrefixCountBits(v > 2) * 1000;
                // (The PC compile evaluates a wave operation in a `?:` arm
                // in that arm's lanes only, so it stays outside.)
                o.w = asuint(WaveActiveSum(v)) + (WaveIsFirstLane() ? WaveReadLaneFirst(u) : 0);
            }
            outb.Store4(16 * n, o);
        }"#,
    ),
    // Atomics the console aggregates per wave (one add of the lane count,
    // each lane's offset from `v_mbcnt`), and texture atomics. Results
    // that depend on the lanes' order are left out.
    (
        "aggregated and texture atomics",
        r#"CS {
            uint u = raw.Load(4 * (i & 31)), r, q;
            outb.InterlockedAdd(0, 1, r);
            outb.InterlockedAdd(4 * (1 + r % 8), 1);
            if (u & 1) outb.InterlockedAdd(64, u & 255);
            uint2 p = d.xy & 3;
            InterlockedMax(ut[p], u);
            InterlockedOr(ut[p + uint2(4, 0)], u >> (i & 7));
            InterlockedMin(ut[p + uint2(8, 0)], u);
            InterlockedAdd(ut[p + uint2(12, 0)], u & 15, q);
            InterlockedXor(ut[p + uint2(0, 4)], u);
        }"#,
    ),
    // A loop whose count comes from a view that reads zeros (as a null or
    // not yet mirrored one does) runs four billion times on the console;
    // the translation leaves it after `LOOP_LIMIT` iterations. Run on WARP
    // only (`VICTORY_WARP=1`), ours only: without the limit it would hang
    // the GPU.
    (
        "terminates on zero data",
        r#"CS {
            uint cnt = outb.Load(4 * 1000), s = d.x;
            for (uint j = 0; j != cnt - 1; j++) s = s * 3 + raw.Load(4 * (j & 31));
            outb.Store(4 * (d.y * 16 + d.x), s);
        }"#,
    ),
    // Cube sampling: the console computes face coordinates (`v_cube*`)
    // and samples the face; the host samples the direction rebuilt from
    // them. Every face and texel: a major axis of ±2 and the others at
    // texel centres, so both sides compute them exactly.
    (
        "cube sampling",
        r#"CS {
            float sg = (i & 8) ? -2 : 2;
            float a = (d.x & 3) - 1.5, b = (d.y & 3) - 1.5;
            uint ax = i % 3;
            float3 dir = ax == 0 ? float3(sg, a, b) : ax == 1 ? float3(a, sg, b) : float3(a, b, sg);
            float4 p = cubet.SampleLevel(smp, dir, 0);
            float4 q = cubet.SampleLevel(smp, -dir.zxy, 0);
            outb.Store4(16 * (d.y * 16 + d.x), asuint(p + q * 3));
        }"#,
    ),
    // A local array indexed per lane (M0-relative VGPR moves), bit fields
    // and first bits from the top.
    (
        "indexed registers and bit fields",
        r#"CS {
            uint n = d.y * 16 + d.x;
            uint x = raw.Load(4 * (n & 31)), y = raw.Load(4 * ((n * 5) & 31));
            uint arr[24];
            [unroll] for (uint j = 0; j < 24; j++) arr[j] = x ^ (j * 0x01010101);
            [loop] for (uint m = 0; m < (y & 3); m++) arr[(y >> (m * 5)) % 24] = y + m;
            uint v = arr[(x >> 3) % 24];
            uint w = asuint(k.x) & 31, o = n & 15;
            uint bf = (x & ((1u << w) - 1)) << o;
            uint fb = firstbithigh(asuint(k.y)) + firstbithigh(x) * 3 + firstbithigh(int(y)) * 7;
            outb.Store4(16 * n, uint4(v, bf, fb, arr[n % 24]));
        }"#,
    ),
    // Size queries at a level known per lane (`image_get_resinfo`).
    (
        "size queries",
        r#"CS {
            uint w, h, l, cw, ch, cl;
            tex.GetDimensions(d.x >> 4, w, h, l);
            cubet.GetDimensions(d.y >> 4, cw, ch, cl);
            float r = tex.Load(int3(d.xy & 3, 0)).x;
            outb.Store4(16 * (d.y * 16 + d.x), uint4(w * 7 + h, l + asuint(r), cw * 3 + ch, cl));
        }"#,
    ),
    // Gathers with offsets computed per lane (programmable offsets).
    (
        "gather with offsets",
        r#"CS {
            int2 o = int2(int(d.x & 7) - 4, int(d.y % 5) - 2);
            float4 r = tex.GatherRed(smp, (d.xy + 0.5) / 16.0, o);
            float4 q = tex.GatherGreen(smp, (d.yx + 0.25) / 16.0, -o);
            outb.Store4(16 * (d.y * 16 + d.x), asuint(r + q * 3));
        }"#,
    ),
    // Constant arrays: the console compiler appends them to the code and
    // reads them through `s_getpc_b64` (scalar loads for a uniform index,
    // global loads per lane, 16-bit ones too).
    (
        "constant tables",
        r#"static const uint tbl[16] = { 3, 1, 4, 1, 5, 9, 2, 6, 5, 3, 5, 8, 9, 7, 9, 3 };
        static const float ftbl[8] = { 0.5, -1, 2.25, 3, -4.5, 6, 7.75, -8 };
        // (16-bit types: compiled with half precision enabled.)
        static const uint16_t stbl[12] = { 100, 200, 300, 400, 500, 600, 700, 800, 900, 1000, 1100, 65535 };
        CS {
            uint n = d.y * 16 + d.x;
            uint u = asuint(k.x) & 15;
            uint a = tbl[i & 15] ^ tbl[u] << 8;
            float f = ftbl[(i * 3) & 7] * ftbl[u & 7];
            uint h = stbl[(n * 5) % 12] + stbl[u % 12];
            outb.Store4(16 * n, uint4(a, asuint(f), h, tbl[(n + u) & 15]));
        }"#,
    ),
    // 16-bit arithmetic (scalar and packed), 16-bit compares, typed stores
    // of halves (`buffer_store_format_d16`), 64-bit shifts and compares.
    (
        "16-bit and 64-bit arithmetic",
        r#"RWBuffer<half4> hb : register(u3);
        CS {
            uint n = d.y * 16 + d.x;
            uint x = raw.Load(4 * (n & 31)), y = raw.Load(4 * ((n * 7) & 31));
            half a = half(d.x * k.x - 2), b = half(d.y + k.y), c = half(k.z);
            half2 p = half2(a, b) * half2(b, c) + half2(c, a);
            half m = max(a, b) - min(a, c) * a;
            bool lt = a < b, ge = p.x >= p.y;
            hb[n] = half4(p.x, m, lt ? a : b, ge ? p.y : a - b);
            uint64_t q = (uint64_t(x) << (n & 31)) | y, w = uint64_t(y) << 3;
            uint z = q < w ? 1 : q == w ? 2 : 3;
            outb.Store2(8 * n, uint2(uint(q >> 7) ^ z, uint(w >> (n & 15))));
        }"#,
    ),
];

const MS_COMMON: &str = r#"
#define RS "RootConstants(num32BitConstants=4, b0)"
cbuffer K : register(b0) { float4 k; };
struct V { float4 pos : SV_Position; float4 col : COLOR0; float2 uv : TEXCOORD0; };
[RootSignature(RS)] float4 mps(V i) : SV_Target { return i.col + float4(i.uv, 0, 0); }
#define MS_ATTR [RootSignature(RS)] [outputtopology("triangle")]
"#;

/// Mesh shaders, each `ms` (the console runs one thread per output lane, so
/// groups of 32 run as 64 lanes there), with their group counts. Drawn
/// with `mps` into 16×16 targets. The 2020 compiler's containers do not
/// record the thread count, so cases reading the group id keep
/// max(threads, vertices, primitives) at the lane count (see `emit`).
const MS_CASES: &[(&str, u32, &str)] = &[
    (
        "one triangle",
        1,
        r#"MS_ATTR [numthreads(32, 1, 1)]
        void ms(uint i : SV_GroupIndex, out vertices V verts[3], out indices uint3 tris[1]) {
            SetMeshOutputCounts(3, 1);
            if (i < 3) {
                verts[i].pos = float4(i == 1 ? 3 : -1, i == 2 ? 3 : -1, 0.5, 1);
                verts[i].col = float4(k.x * i, k.y, k.z * (i == 2), 1);
                verts[i].uv = float2(i * 0.5, k.w);
            }
            if (i == 0) tris[0] = uint3(0, 1, 2);
        }"#,
    ),
    // A quad per group in its quadrant; a run-time vertex count, and a
    // primitive count per group whose third triangle is culled.
    (
        "groups, counts and culling",
        4,
        r#"struct P { bool cull : SV_CullPrimitive; };
        MS_ATTR [numthreads(64, 1, 1)]
        void ms(uint i : SV_GroupIndex, uint3 g : SV_GroupID, out vertices V verts[8],
                out indices uint3 tris[4], out primitives P prims[4]) {
            uint nv = 4 + (k.z > 1);
            uint np = 2 + (g.x & 1);
            SetMeshOutputCounts(nv, np);
            float2 o = float2(g.x & 1, g.x >> 1) - 1;
            if (i < nv) {
                float2 p = i < 4 ? float2(i & 1, i >> 1) : float2(-0.75, 0.5);
                verts[i].pos = float4(o + p, 0.5, 1);
                verts[i].col = float4(k.x * i, g.x * 0.25, k.z, 1);
                verts[i].uv = float2(i, g.x);
            }
            if (i < np) {
                tris[i] = i == 0 ? uint3(0, 1, 2) : i == 1 ? uint3(1, 3, 2) : uint3(1, 4, 3);
                prims[i].cull = i == 2;
            }
        }"#,
    ),
    // Two waves per group, as the title's mesh programs run: the
    // second wave's lanes are threads 64-127.
    (
        "two waves",
        1,
        r#"MS_ATTR [numthreads(128, 1, 1)]
        void ms(uint i : SV_GroupIndex, out vertices V verts[81], out indices uint3 tris[128]) {
            SetMeshOutputCounts(81, 128);
            if (i < 81) {
                float2 p = float2(i % 9, i / 9) / 8.0;
                verts[i].pos = float4(p * 2 - 1, 0.5, 1);
                verts[i].col = float4(p.x * k.x, p.y, (i & 7) * 0.125, 1);
                verts[i].uv = float2(i, k.y);
            }
            uint c = i / 2 % 8 + i / 16 * 9;
            tris[i] = i & 1 ? uint3(c + 1, c + 10, c + 9) : uint3(c, c + 1, c + 9);
        }"#,
    ),
    // Quad reads (DPP quad permutations on the console; the title's
    // tessellating mesh programs swizzle within quads too).
    (
        "quad reads",
        1,
        r#"MS_ATTR [numthreads(128, 1, 1)]
        void ms(uint i : SV_GroupIndex, out vertices V verts[81], out indices uint3 tris[128]) {
            SetMeshOutputCounts(81, 128);
            float w = i * k.x + k.y;
            float a = QuadReadLaneAt(w, 0);
            float b = QuadReadLaneAt(w, 2);
            float c = QuadReadAcrossX(w);
            if (i < 81) {
                float2 p = float2(i % 9, i / 9) / 8.0;
                verts[i].pos = float4(p * 2 - 1, 0.5, 1);
                verts[i].col = float4(a, b, c, 1);
                verts[i].uv = float2(i, k.y);
            }
            uint cc = i / 2 % 8 + i / 16 * 9;
            tris[i] = i & 1 ? uint3(cc + 1, cc + 10, cc + 9) : uint3(cc, cc + 1, cc + 9);
        }"#,
    ),
    // Vertices and indices written by other threads than their lanes: the
    // console passes them through shared memory, with a loop and barriers.
    (
        "grid through shared memory",
        2,
        r#"MS_ATTR [numthreads(32, 1, 1)]
        void ms(uint i : SV_GroupIndex, uint3 g : SV_GroupID, out vertices V verts[36],
                out indices uint3 tris[64]) {
            SetMeshOutputCounts(36, 50);
            for (uint v = i; v < 36; v += 32) {
                float2 p = float2(v % 6, v / 6) / 5.0;
                verts[v].pos = float4(p.x - g.x, p.y * 2 - 1, 0.5, 1);
                verts[v].col = float4(p * k.x, k.z * g.x, 1);
                verts[v].uv = float2(v, k.y);
            }
            if (i < 25) {
                uint c = i % 5 + i / 5 * 6;
                tris[2 * i] = uint3(c, c + 1, c + 6);
                tris[2 * i + 1] = uint3(c + 1, c + 7, c + 6);
            }
        }"#,
    ),
];

fn tools() -> Option<(PathBuf, PathBuf)> {
    let console = PathBuf::from(std::env::var_os("VICTORY_CONSOLE_DXC")?);
    let pc = match std::env::var_os("VICTORY_PC_DXC") {
        Some(p) => PathBuf::from(p),
        None => super::validator::Validator::default_path()?.with_file_name("dxc.exe"),
    };
    (console.exists() && pc.exists()).then_some((console, pc))
}

fn compile(dxc: &Path, dir: &Path, src: &str, entry: &str, profile: &str) -> Vec<u8> {
    let input = dir.join("t.hlsl");
    let output = dir.join(format!("{entry}.bin"));
    let text = if entry == "cs" {
        format!("#define RS \"{CS_RS}\"\n{CS_COMMON}\n{src}\n")
    } else if entry.starts_with('m') {
        format!("{MS_COMMON}\n{src}\n")
    } else {
        format!("#define RS \"{RS}\"\n{COMMON}\n{src}\n")
    };
    std::fs::write(&input, text).unwrap();
    let out = std::process::Command::new(dxc)
        .args(["-T", profile, "-E", entry, "-Fo"])
        .arg(&output)
        .arg(&input)
        .args(src.contains("half").then_some("-enable-16bit-types"))
        // `CONSOLE` marks the console compile (cases spell console-only
        // intrinsics there).
        .args(
            std::env::var_os("VICTORY_CONSOLE_DXC")
                .is_some_and(|c| Path::new(&c) == dxc)
                .then_some("-DCONSOLE"),
        )
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::read(output).unwrap()
}

/// Root arguments for a translated stage: user data from the root values.
fn user_data(s: &HostShader, k: &[u32; 4]) -> Vec<u32> {
    let mut data = vec![0; (s.user_data_dwords() as usize).max(1)];
    for u in &s.user_data {
        data[u.index as usize] = match (u.root, u.slot) {
            (RootType::Constants, 0) => k[u.dword as usize],
            // The texture and sampler tables start at heap index 1, the UAV
            // table (compute) at 5, in the console's units.
            (RootType::Table, 1) => 32,
            (RootType::Table, 2) => 16,
            (RootType::Table, 3) => 5 * 32,
            // The typed views of `raw` (compute) from 9.
            (RootType::Table, 6) => 9 * 32,
            other => panic!("unexpected user data {other:?}"),
        };
    }
    data
}

#[test]
fn translated_shaders_draw_as_pc_ones() {
    let Some((console, pc)) = tools() else {
        crate::emit::test_skip::skip("no console or PC compiler; skipped");
        return;
    };
    let Ok(host) = Host::new(false) else {
        crate::emit::test_skip::skip("no SM 6.6 device; skipped");
        return;
    };
    let dir = std::env::temp_dir().join(format!("victory-diff-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let k = [0.75f32, -1.5, 2.0, 0.125].map(f32::to_bits);
    let texels: Vec<[f32; 4]> = (0..16)
        .map(|i| {
            [
                i as f32 / 16.0,
                (i % 4) as f32 * 0.3,
                1.0 - i as f32 / 20.0,
                0.5,
            ]
        })
        .collect();
    let dummy = [[9.0f32; 4]];
    let sampler = |filter, address| D3D12_SAMPLER_DESC {
        Filter: filter,
        AddressU: address,
        AddressV: address,
        AddressW: address,
        MaxAnisotropy: 1,
        MaxLOD: f32::MAX,
        ..Default::default()
    };
    let samplers = [
        sampler(
            D3D12_FILTER_MIN_MAG_MIP_POINT,
            D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        ),
        sampler(
            D3D12_FILTER_MIN_MAG_MIP_LINEAR,
            D3D12_TEXTURE_ADDRESS_MODE_WRAP,
        ),
    ];
    // A full-screen triangle: POSITION (4 floats), TEXCOORD0 (2 floats).
    let vertices = [
        -1.0, -1.0, 0.5, 1.0, 0.0, 1.0, //
        -1.0, 3.0, 0.5, 1.0, 0.0, -1.0, //
        3.0, -1.0, 0.5, 1.0, 2.0, 1.0,
    ];
    let textures: [(u32, u32, &[[f32; 4]]); 2] = [(1, 1, &dummy), (4, 4, &texels)];
    let draw = |roots| Draw {
        roots,
        textures: &textures,
        samplers: &samplers,
        vertices: &vertices,
        stride: 24,
        size: (16, 16),
        mesh_groups: 0,
    };
    let mut failures = Vec::new();
    let only = std::env::var("VICTORY_DIFF_ONLY").ok();
    for (name, src) in CASES
        .iter()
        .filter(|c| only.as_ref().is_none_or(|o| o == c.0))
    {
        // A case may bring its own vertex shader.
        let vs = if src.contains(" vs16(") { "vs16" } else { "vs" };
        let vs_c = translate(&compile(&console, &dir, src, vs, "vs_6_5")).unwrap();
        let ps_c = match translate(&compile(&console, &dir, src, "ps", "ps_6_5")) {
            Ok(s) => s,
            Err(e) => {
                failures.push(format!("{name}: {e}"));
                continue;
            }
        };
        if let Ok(out) = std::env::var("VICTORY_EMIT_DUMP") {
            let stem = name.replace([' ', ','], "_");
            std::fs::write(
                format!("{out}/{stem}.ps.cdxil"),
                compile(&console, &dir, src, "ps", "ps_6_5"),
            )
            .unwrap();
            std::fs::write(format!("{out}/{stem}.ps.dxil"), &ps_c.dxil).unwrap();
        }
        if let Ok(mut v) = super::validator::Validator::load(
            super::validator::Validator::default_path().as_deref(),
        ) {
            for s in [&vs_c, &ps_c] {
                v.validate(&mut s.dxil.clone())
                    .unwrap_or_else(|e| panic!("{name}: {e}"));
            }
        }
        let (root_t, pso_t) = host.pipeline(&vs_c, &ps_c).unwrap();
        let ours = host
            .draw(
                &root_t,
                &pso_t,
                &draw(vec![
                    (0, Root::Constants(user_data(&vs_c, &k))),
                    (1, Root::Constants(user_data(&ps_c, &k))),
                ]),
            )
            .unwrap();

        let (vs_p, ps_p) = (
            compile(&pc, &dir, src, vs, "vs_6_5"),
            compile(&pc, &dir, src, "ps", "ps_6_5"),
        );
        // SAFETY: the PC container carries its root signature.
        let root_p: ID3D12RootSignature =
            unsafe { host.device.CreateRootSignature(0, &ps_p) }.unwrap();
        let pso_p = host.pipeline_with(&root_p, &vs_p, &ps_p).unwrap();
        let theirs = host
            .draw(
                &root_p,
                &pso_p,
                &draw(vec![
                    (0, Root::Constants(k.to_vec())),
                    (
                        1,
                        Root::Table {
                            sampler: false,
                            index: 1,
                        },
                    ),
                    (
                        2,
                        Root::Table {
                            sampler: true,
                            index: 1,
                        },
                    ),
                ]),
            )
            .unwrap();

        let worst = ours
            .iter()
            .flatten()
            .zip(theirs.iter().flatten())
            .enumerate()
            .map(|(i, (a, b))| (((a - b).abs() / b.abs().max(1.0)), i, *a, *b))
            .max_by(|x, y| x.0.total_cmp(&y.0))
            .unwrap();
        eprintln!(
            "{name}: worst relative difference {:.2e} (at {}: {} vs {})",
            worst.0, worst.1, worst.2, worst.3
        );
        if worst.0.is_nan() || worst.0 > 1e-3 {
            failures.push(format!("{name}: {worst:?}"));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn translated_mesh_shaders_draw_as_pc_ones() {
    let Some((console, pc)) = tools() else {
        crate::emit::test_skip::skip("no console or PC compiler; skipped");
        return;
    };
    let Ok(host) = Host::new(false) else {
        crate::emit::test_skip::skip("no SM 6.6 device; skipped");
        return;
    };
    let dir = std::env::temp_dir().join(format!("victory-diff-ms-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let k = [0.75f32, -1.5, 2.0, 0.125].map(f32::to_bits);
    let draw = |roots, mesh_groups| Draw {
        roots,
        textures: &[],
        samplers: &[],
        vertices: &[],
        stride: 0,
        size: (16, 16),
        mesh_groups,
    };
    let mut failures = Vec::new();
    let only = std::env::var("VICTORY_DIFF_ONLY").ok();
    for &(name, groups, src) in MS_CASES
        .iter()
        .filter(|c| only.as_ref().is_none_or(|o| o == c.0))
    {
        let console_ms = compile(&console, &dir, src, "ms", "ms_6_5");
        let ms_c = match translate(&console_ms) {
            Ok(s) => s,
            Err(e) => {
                failures.push(format!("{name}: {e}"));
                continue;
            }
        };
        let ps_c = translate(&compile(&console, &dir, src, "mps", "ps_6_5")).unwrap();
        if let Ok(out) = std::env::var("VICTORY_EMIT_DUMP") {
            let stem = name.replace([' ', ','], "_");
            std::fs::write(format!("{out}/{stem}.ms.cdxil"), &console_ms).unwrap();
            std::fs::write(format!("{out}/{stem}.ms.dxil"), &ms_c.dxil).unwrap();
        }
        if let Ok(mut v) = super::validator::Validator::load(
            super::validator::Validator::default_path().as_deref(),
        ) {
            v.validate(&mut ms_c.dxil.clone())
                .unwrap_or_else(|e| panic!("{name}: {e}"));
        }
        let (root_t, pso_t) = host.mesh_pipeline(&ms_c, &ps_c).unwrap();
        let ours = host
            .draw(
                &root_t,
                &pso_t,
                &draw(
                    vec![
                        (0, Root::Constants(user_data(&ms_c, &k))),
                        (1, Root::Constants(user_data(&ps_c, &k))),
                    ],
                    groups,
                ),
            )
            .unwrap();
        let (ms_p, ps_p) = (
            compile(&pc, &dir, src, "ms", "ms_6_5"),
            compile(&pc, &dir, src, "mps", "ps_6_5"),
        );
        // SAFETY: the PC container carries its root signature.
        let root_p: ID3D12RootSignature =
            unsafe { host.device.CreateRootSignature(0, &ms_p) }.unwrap();
        let pso_p = host.mesh_pipeline_with(&root_p, &ms_p, &ps_p).unwrap();
        let theirs = host
            .draw(
                &root_p,
                &pso_p,
                &draw(vec![(0, Root::Constants(k.to_vec()))], groups),
            )
            .unwrap();
        let worst = ours
            .iter()
            .flatten()
            .zip(theirs.iter().flatten())
            .enumerate()
            .map(|(i, (a, b))| (((a - b).abs() / b.abs().max(1.0)), i, *a, *b))
            .max_by(|x, y| x.0.total_cmp(&y.0))
            .unwrap();
        let drawn = theirs.iter().filter(|t| t[3] != 0.0).count();
        eprintln!(
            "{name}: {drawn} texels drawn; worst relative difference {:.2e} (at {}: {} vs {})",
            worst.0, worst.1, worst.2, worst.3
        );
        if worst.0.is_nan() || worst.0 > 1e-3 || drawn == 0 {
            // Which texels each drew (alpha set), row by row.
            for (o, t) in ours.chunks(16).zip(theirs.chunks(16)) {
                let row = |r: &[[f32; 4]]| -> String {
                    r.iter()
                        .map(|x| if x[3] != 0.0 { '#' } else { '.' })
                        .collect()
                };
                eprintln!("  {}  {}", row(o), row(t));
            }
            failures.push(format!("{name}: {worst:?}"));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(failures.is_empty(), "{failures:#?}");
}

/// Where PC code and the console differ by design, the translation must
/// give the console's results: float-to-int conversions saturate and give
/// 0 for NaN (`v_cvt_i32/u32_f32`), the legacy multiply the compiler
/// uses for `*` gives 0 for 0 × NaN, `frac` never reaches 1.0 (it gives
/// the largest value below; some hosts' own `frac` gives 1.0), and sine
/// and cosine take turns: the sine of a half turn and the cosine of a
/// quarter turn are exactly +0 (the host's functions of the rounded angle
/// give about 1e-7). The compiler takes whole turns off itself, so large
/// angles cannot be tried from here.
#[test]
fn float_semantics_are_the_consoles() {
    use super::host::{Dispatch, View};
    let Some((console, _)) = tools() else {
        crate::emit::test_skip::skip("no console compiler; skipped");
        return;
    };
    let Ok(host) = Host::new(false) else {
        crate::emit::test_skip::skip("no SM 6.6 device; skipped");
        return;
    };
    let dir = std::env::temp_dir().join(format!("victory-float-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = r#"[RootSignature(RS)] [numthreads(4, 1, 1)]
        void cs(uint i : SV_GroupIndex) {
            float x = i == 0 ? k.x : i == 1 ? -k.x : i == 2 ? k.y : k.z;
            outb.Store(4 * i, asuint(int(x)));
            outb.Store(16 + 4 * i, uint(x));
            outb.Store(32 + 4 * i, asuint(x * k.w));
            float y = i == 0 ? k.w - 1e-10 : i == 1 ? k.z : i == 2 ? k.w - 1e-8 : k.z + 0.25;
            outb.Store(48 + 4 * i, asuint(frac(y)));
            float r = i == 0 ? k.w + 3.14159274 : i == 1 ? k.w + 1.0
                    : i == 2 ? k.w - 3.14159274 : k.w + 1.57079637;
            outb.Store(64 + 4 * i, asuint(i == 3 ? cos(r) : sin(r)));
        }"#;
    let cs = translate(&compile(&console, &dir, src, "cs", "cs_6_5")).unwrap();
    let (root, pso) = host.compute_pipeline(&cs).unwrap();
    // 1e30, NaN, 3.5, 0.
    let k = [1e30f32.to_bits(), 0x7FC0_0000, 3.5f32.to_bits(), 0];
    let views = [
        View::Texture(1, 1, &[[0.0; 4]]),
        View::Texture(1, 1, &[[0.0; 4]]),
        View::Texture(1, 1, &[[0.0; 4]]),
        View::Texture(1, 1, &[[0.0; 4]]),
        View::Texture(1, 1, &[[0.0; 4]]),
        View::RawUav(20),
        View::TextureUav(1, 1),
    ];
    let out = host
        .dispatch(
            &root,
            &pso,
            &Dispatch {
                roots: vec![(0, Root::Constants(user_data(&cs, &k)))],
                views: &views,
                samplers: &[],
                groups: [1, 1, 1],
            },
        )
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        out[0][..16],
        [
            0x7FFF_FFFF,
            0x8000_0000,
            0,
            3, // int
            u32::MAX,
            0,
            0,
            3, // uint
            0,
            0,
            0,
            0, // x * 0, NaN included
            0x3F7F_FFFF,
            0.5f32.to_bits(),
            0x3F7F_FFFF,
            0.75f32.to_bits(), // frac
        ]
    );
    // sin(pi), sin(-pi) and cos(pi/2): the compiler turns radians into
    // exactly 0.5, -0.5 and 0.25 turns.
    assert_eq!([out[0][16], out[0][18], out[0][19]], [0, 0, 0]);
    // An ordinary angle is untouched: sin(1 rad).
    let one = f32::from_bits(out[0][17]);
    assert!((one - 0.841_470_96).abs() < 1e-4, "{one}");
}

/// Equal bits, both zero (the console's legacy multiply gives +0 where PC
/// code gives -0), or two normal floats within 1e-3 relative.
fn close(a: u32, b: u32) -> bool {
    let (x, y) = (f32::from_bits(a), f32::from_bits(b));
    a == b
        || x == 0.0 && y == 0.0
        || x.is_normal() && y.is_normal() && (x - y).abs() <= 1e-3 * y.abs().max(1.0)
}

#[test]
fn translated_compute_matches_pc() {
    use super::host::{Dispatch, Stride, View};
    let Some((console, pc)) = tools() else {
        crate::emit::test_skip::skip("no console or PC compiler; skipped");
        return;
    };
    let Ok(host) = Host::new(false) else {
        crate::emit::test_skip::skip("no SM 6.6 device; skipped");
        return;
    };
    let dir = std::env::temp_dir().join(format!("victory-diff-cs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let k = [0.75f32, -1.5, 2.0, 0.125].map(f32::to_bits);
    let texels: Vec<[f32; 4]> = (0..16)
        .map(|i| [i as f32 / 16.0, (i % 4) as f32 * 0.3, 0.5, 1.0])
        .collect();
    let dummy = [[9.0f32; 4]];
    let cube_texels: Vec<[f32; 4]> = (0..96)
        .map(|i| {
            [
                (i / 16) as f32,
                (i % 4) as f32 * 0.25,
                (i / 4 % 4) as f32 * 0.25,
                1.0,
            ]
        })
        .collect();
    // Buffers: 16 float4, 8 structures of 16 bytes, 32 raw dwords; the
    // root CBV's 16 float4 and the root SRV's 32 dwords.
    let floats = |n: usize, scale: f32| -> Vec<u32> {
        (0..n).map(|i| (i as f32 * scale - 3.0).to_bits()).collect()
    };
    let typed_data = floats(64, 0.25);
    let structured_data: Vec<u32> = (0..32u32)
        .map(|i| {
            if i % 4 == 1 {
                i * 0x0101_0101
            } else {
                (i as f32 * 0.5).to_bits()
            }
        })
        .collect();
    let raw_data: Vec<u32> = (0..32u32).map(|i| i.wrapping_mul(0x9E37_79B9)).collect();
    let views = [
        View::Texture(1, 1, &dummy),
        View::Texture(4, 4, &texels),
        View::Buffer {
            data: &typed_data,
            stride: Stride::Typed,
        },
        View::Buffer {
            data: &structured_data,
            stride: Stride::Structured(16),
        },
        View::Buffer {
            data: &raw_data,
            stride: Stride::Raw,
        },
        View::RawUav(1024),
        View::TextureUav(16, 16),
        View::UintTextureUav(16, 16),
        View::HalfUav(256),
    ];
    let typed_view = |format, bytes| View::Buffer {
        data: &raw_data,
        stride: Stride::Format(format, bytes),
    };
    let views: Vec<View> = views
        .into_iter()
        .chain([
            typed_view(DXGI_FORMAT_R8G8B8A8_UNORM, 4),
            typed_view(DXGI_FORMAT_R16G16_SNORM, 4),
            typed_view(DXGI_FORMAT_R8G8B8A8_UINT, 4),
            typed_view(DXGI_FORMAT_R10G10B10A2_UNORM, 4),
            typed_view(DXGI_FORMAT_R16G16B16A16_UINT, 8),
            typed_view(DXGI_FORMAT_R8_UNORM, 1),
            typed_view(DXGI_FORMAT_R16_FLOAT, 2),
            typed_view(DXGI_FORMAT_R8G8B8A8_SNORM, 4),
            View::Cube(4, &cube_texels),
        ])
        .collect();
    let root_cbv = floats(64, -0.125);
    let root_srv: Vec<u32> = (0..32u32).map(|i| i * 0x0001_0003 + 7).collect();
    let samplers = [D3D12_SAMPLER_DESC {
        Filter: D3D12_FILTER_MIN_MAG_MIP_POINT,
        AddressU: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        AddressV: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        AddressW: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        MaxAnisotropy: 1,
        MaxLOD: f32::MAX,
        ..Default::default()
    }; 2];
    let dispatch = |roots| Dispatch {
        roots,
        views: &views,
        samplers: &samplers,
        groups: [2, 2, 1],
    };
    let mut failures = Vec::new();
    let only = std::env::var("VICTORY_DIFF_ONLY").ok();
    let warp = std::env::var("VICTORY_WARP").is_ok_and(|v| v == "1");
    for (name, src) in CS_CASES
        .iter()
        .filter(|c| only.as_ref().is_none_or(|o| o == c.0))
        .filter(|c| warp || !c.0.starts_with("terminates"))
    {
        let console_cs = compile(&console, &dir, src, "cs", "cs_6_5");
        if let Ok(out) = std::env::var("VICTORY_EMIT_DUMP") {
            let stem = name.replace([' ', ','], "_");
            std::fs::write(format!("{out}/{stem}.cs.cdxil"), &console_cs).unwrap();
        }
        let cs_c = match translate(&console_cs) {
            Ok(s) => s,
            Err(e) => {
                failures.push(format!("{name}: {e}"));
                continue;
            }
        };
        if let Ok(out) = std::env::var("VICTORY_EMIT_DUMP") {
            let stem = name.replace([' ', ','], "_");
            std::fs::write(format!("{out}/{stem}.cs.dxil"), &cs_c.dxil).unwrap();
        }
        if let Ok(mut v) = super::validator::Validator::load(
            super::validator::Validator::default_path().as_deref(),
        ) {
            v.validate(&mut cs_c.dxil.clone())
                .unwrap_or_else(|e| panic!("{name}: {e}"));
        }
        let (root_t, pso_t) = match host.compute_pipeline(&cs_c) {
            Ok(p) => p,
            // WARP runs no fixed wave size of 64.
            Err(e) if std::env::var("VICTORY_WARP").is_ok_and(|v| v == "1") => {
                crate::emit::test_skip::skip(&format!("{name}: refused on WARP ({e})"));
                continue;
            }
            Err(e) => panic!("{name}: {e}"),
        };
        // Our root views follow the user data in root parameter order.
        let mut roots = vec![(0, Root::Constants(user_data(&cs_c, &k)))];
        let mut root_views: Vec<(u8, bool)> = cs_c.root_cbvs.iter().map(|&s| (s, false)).collect();
        root_views.extend(cs_c.root_srvs.iter().map(|&s| (s, true)));
        root_views.sort_unstable();
        for (n, (slot, srv)) in root_views.into_iter().enumerate() {
            assert_eq!(slot, 4 + u8::from(srv), "{name}: root view");
            let data = if srv {
                root_srv.clone()
            } else {
                root_cbv.clone()
            };
            roots.push((1 + n as u32, Root::View { srv, data }));
        }
        let ours = host.dispatch(&root_t, &pso_t, &dispatch(roots));
        if name.starts_with("terminates") {
            match ours {
                Ok(_) => eprintln!("{name}: finished"),
                Err(e) => failures.push(format!("{name}: {e}")),
            }
            continue;
        }
        let ours = ours.unwrap();
        let cs_p = compile(&pc, &dir, src, "cs", "cs_6_5");
        // SAFETY: the PC container carries its root signature.
        let root_p: ID3D12RootSignature =
            unsafe { host.device.CreateRootSignature(0, &cs_p) }.unwrap();
        let pso_p = host.compute_pipeline_with(&root_p, &cs_p).unwrap();
        let table = |sampler, index| Root::Table { sampler, index };
        let theirs = host
            .dispatch(
                &root_p,
                &pso_p,
                &dispatch(vec![
                    (0, Root::Constants(k.to_vec())),
                    (1, table(false, 1)),
                    (2, table(true, 1)),
                    (3, table(false, 5)),
                    (
                        4,
                        Root::View {
                            srv: false,
                            data: root_cbv.clone(),
                        },
                    ),
                    (
                        5,
                        Root::View {
                            srv: true,
                            data: root_srv.clone(),
                        },
                    ),
                    (6, table(false, 9)),
                ]),
            )
            .unwrap();
        let mismatch = ours
            .iter()
            .flatten()
            .zip(theirs.iter().flatten())
            .enumerate()
            .find(|(_, (a, b))| !close(**a, **b));
        match mismatch {
            None => eprintln!("{name}: equal"),
            Some((at, (a, b))) => failures.push(format!(
                "{name}: word {at}: {a:#x} ({}) vs {b:#x} ({})",
                f32::from_bits(*a),
                f32::from_bits(*b)
            )),
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(failures.is_empty(), "{failures:#?}");
}
