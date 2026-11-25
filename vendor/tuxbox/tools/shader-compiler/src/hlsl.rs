//! RDNA2 → HLSL translator (stage 3 of the shader pipeline).
//!
//! Model: one HLSL invocation == one RDNA2 *lane*. Every VGPR/SGPR is a `uint` holding the
//! raw 32-bit pattern (floats via asfloat/asuint), so no type inference is needed. Lane
//! masks used for control flow are represented by their bit for *this* lane (0/1).
//! Recognized wave64 ballots consumed as integer data retain both mask words. Descriptor-table pointers in user SGPRs are tracked symbolically: an
//! `s_load_dwordxN` from a never-written SGPR is a descriptor fetch from a root table, and
//! the way the descriptor is consumed (s_buffer_load → CBV, MIMG resource → SRV, MIMG
//! sampler → sampler) classifies the table. Slots map 1:1 to HLSL registers because the
//! title's root signature (UE layout) uses one SRV/CBV/sampler table per stage from
//! register 0.
//!
//! Scope: forward-only control flow (no loops yet). Anything not understood returns `Err`
//! naming the instruction — the caller must fall back; nothing is guessed.

use crate::meta::SigElement;
use crate::rdna2::{disassemble, Enc, Insn, Opnd};
use crate::{ShaderExtract, Stage};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;

mod code_table;
mod wave_alloc;
mod group_extrema;
mod compare;
mod uniform_texel;
mod pixel_quad;
mod mubuf_offsets;
mod scalar_values;
mod code_offset;
mod scalar_bits;
#[cfg(test)]
mod raw_width;
#[cfg(test)]
mod raw_idxen;
#[cfg(test)]
mod wide_atomic_test;
#[cfg(test)]
mod cpu_oracle;
#[cfg(test)]
mod heap_resinfo;
#[cfg(test)]
mod compute_swizzle;
#[cfg(test)]
mod sat_pk_u8_i16;
mod fssm_bindings;
mod micropoly;
mod mesh;
mod mesh_exports;
mod exec_group;
mod exec_snapshot;
mod hair_cluster;
mod hair_cluster_export;
mod instance_cull;
mod instance_cull_tail;
mod instance_cull_hzb;
mod instance_cull_post;
mod instance_cull_views;
mod classify_materials;
mod int16;
mod int32;
mod float64;
mod indexed_lane;
mod particle_sort;
mod persistent;
mod persistent_lanes;
mod persistent_resources;
mod lds_tile;
mod lds_order;
mod ps09_classify;
mod subsurface_setup;
mod hair_compact;
mod hair_multisample;
mod hair_visibility_single;
mod hair_visibility_indirect;
mod packed_root;
mod mesh_bindings;
mod params;
pub(crate) use params::flat_registers;
use params::Param;
mod light_grid;
mod lone_lane;
mod page_waves;
mod wave_cull;
mod ray_resolve;
mod fspx_wave;
mod singleton_wave;
mod wave_sum;
mod lockstep;
mod wave_loops;
mod raw_index;
mod reflection_compact;
pub(crate) mod radiance_probe;
mod screen_gather;
mod reflection_far;
mod sampler_heap;
mod lane_reads;
mod heap_offsets;
mod heap_origins;
use sampler_heap::DirectSampler;
mod reflection_probe;
mod wave_scan;
use packed_root::{PackedRoot, RootAbi};

pub struct Translated {
    pub hlsl: String,
    pub entry: &'static str,
    /// Target profile for the generated HLSL.
    pub profile: &'static str,
    /// Per-vertex parameter reads require a shader-model 6.1 pixel shader.
    pub needs_barycentrics: bool,
    /// A two-word typed image atomic requires shader model 6.6.
    pub typed_int64_atomics: bool,
    pub packed_dot_product: bool,
    /// Required wave size for compute shaders with width-specific lane reads.
    pub required_wave_size: Option<u32>,
    /// Workgroup dimensions for the subset verified to tolerate either Scarlett wave width.
    pub portable_compute_group: Option<[u32; 3]>,
    /// Pixel shaders: the interpolation modifier of each input. Pass it to
    /// [`translate_linked`] when translating the stage that feeds this shader.
    pub ps_input_interp: InputInterp,
}

/// Interpolation modifiers of a pixel shader's inputs, by semantic (`TEXCOORD0`), in input
/// signature order. Only inputs with a modifier are listed; the text is the HLSL modifier
/// as emitted (it may be the `TB_VERTEX_INPUT` macro).
///
/// DXC packs two signature elements into one register only if they have the same
/// interpolation mode. The pixel shader picks modifiers from how its microcode reads each
/// parameter (per-vertex, flat, centroid, integer), while the stage feeding it used to
/// declare none, so the two sides could be packed differently: D3D12 then rejects the
/// pipeline (`CREATEGRAPHICSPIPELINESTATE_SHADER_LINKAGE_REGISTERINDEX` /
/// `_REGISTERMASK`). Seen in Fortnite: a geometry shader wrote TEXCOORD0.xy and
/// SV_RenderTargetArrayIndex to r0.xy and r2.x, its pixel shader read TEXCOORD0 per vertex
/// (`nointerpolation`) and DXC packed SV_RenderTargetArrayIndex into r0.z.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct InputInterp(pub Vec<(String, String)>);

impl InputInterp {
    fn get(&self, semantic: &str) -> Option<&str> {
        self.0.iter().find(|(s, _)| s.eq_ignore_ascii_case(semantic)).map(|(_, m)| m.as_str())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sym {
    DirectRaw(u32),
    /// Root SRV address words 0 and 1, or word 1 after setting the byte stride.
    MeshRawAddress { register: u8, word: u8 },
    SamplerOffset(DirectSampler),
    HeapSampler(DirectSampler),
    HeapOffset,
    HeapDescriptor(u32, Option<usize>),
    /// Dword 1 and up of a descriptor fetched at a run-time heap index. The index itself
    /// lives in dword 0, so a use is valid only while the others are still the loaded ones.
    HeapWord(u8, Option<usize>),
    /// Guest sampler-heap index read at a run-time byte offset (compute sampler heap).
    HeapSamplerIndex(Option<usize>),
    /// Byte offset of a 16-byte sampler descriptor: a scalar shifted left by four.
    HeapSamplerOffset,
    /// See `lane_reads`.
    NoLaneRead,
    InlineWord(u32),
    Table(u8),
    Desc { table: u8, off: u32, k: u32 },
    /// Dword `k` of one of several descriptors (control-flow join of different `s_load`s,
    /// e.g. a loop that picks texture i of N). `set` indexes `Emitter::multi_sets`; which
    /// candidate is live is tracked at run time in a `tb_dsel<reg>` variable.
    Multi { set: u16, k: u32 },
    /// A PC-relative pointer into the shader's own code section (`s_getpc_b64` + offsets),
    /// used to read constant tables embedded after the program.
    Code,
}

/// An image op whose texture and/or sampler register holds one of several descriptors.
/// Each candidate is `(descriptor id, resolved resource)`; the id is what the run-time
/// selector `tb_dsel<reg>` holds.
#[derive(Clone, Debug)]
struct MSel {
    tex_reg: u16,
    samp_reg: u16,
    tex: Vec<(u32, Res)>,
    samp: Vec<(u32, Res)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Res {
    HeapLoad(u16),
    HeapBuffer(u16),
    /// Byte-address buffer named by a heap index held in this SGPR (compute heap).
    HeapRaw(u16),
    HeapTexture(u16, u32),
    Cb(u32),
    Tex(u32, u32), // slot, dim
    Samp(u32),
    HeapSampler(DirectSampler),
    /// Sampler-heap byte offset in this SGPR becomes a sampler index (16-byte descriptors).
    HeapSamplerLoad(u16),
    /// Sampler named by the sampler-heap index held in this SGPR.
    HeapSamplerDyn(u16),
    Uav(u32, u32), // slot, dim
    Buf(u32, bool), // slot, is_uav (structured buffer)
    Raw(u32, bool), // slot, is_uav (byte-address buffer)
    TBuf(u32, bool), // slot, is_uav (typed buffer)
    CbDyn(u32),      // constant buffer read through MUBUF (per-lane offset)
    CodeTbl,         // s_load from the code section (embedded constant table)
}

const XYZW: [&str; 4] = ["x", "y", "z", "w"];

/// RDNA2 cube-map helpers. v_cube{ma,sc,tc,id}_f32 follow the ISA definition (major axis ×2,
/// face-local S/T, face index 0..5 = +X,-X,+Y,-Y,+Z,-Z). Hardware cube sampling takes
/// (s, t, face) with s,t = sc/|ma| + 1.5 (so each is in [1, 2]); `tb_cube_dir` inverts that
/// to the direction vector HLSL TextureCube sampling expects.
const CUBE_HELPERS: &str = r#"uint tb_perm_byte(uint hi, uint lo, uint sel) { if (sel >= 13u) return 0xffu; if (sel == 12u) return 0u; if (sel == 11u) return (hi >> 31) * 0xffu; if (sel == 10u) return ((hi >> 15) & 1u) * 0xffu; if (sel == 9u) return (lo >> 31) * 0xffu; if (sel == 8u) return ((lo >> 15) & 1u) * 0xffu; return sel >= 4u ? ((hi >> ((sel - 4u) * 8u)) & 0xffu) : ((lo >> (sel * 8u)) & 0xffu); }
uint tb_perm(uint hi, uint lo, uint sel) { return tb_perm_byte(hi, lo, sel & 0xffu) | (tb_perm_byte(hi, lo, (sel >> 8) & 0xffu) << 8) | (tb_perm_byte(hi, lo, (sel >> 16) & 0xffu) << 16) | (tb_perm_byte(hi, lo, sel >> 24) << 24); }
bool tb_cmp_class(uint b, uint mask, uint fractionBits, uint exponentMask) {
    uint fractionUnit = 1u << fractionBits;
    uint e = (b >> fractionBits) & exponentMask;
    uint m = b & (fractionUnit - 1u);
    bool neg = (b & ((exponentMask + 1u) << fractionBits)) != 0u;
    uint c;
    if (e == exponentMask) { c = (m != 0u) ? (((m & (fractionUnit >> 1)) != 0u) ? 1u : 0u) : (neg ? 2u : 9u); } else if (e == 0u) { c = (m == 0u) ? (neg ? 5u : 6u) : (neg ? 4u : 7u); } else { c = neg ? 3u : 8u; } return ((mask >> c) & 1u) != 0u; }uint tb_bfe_i32(uint a, uint b) { uint o = b & 31u, w = (b >> 16) & 127u; if (w == 0u) return 0u; if (w + o >= 32u) return asuint(asint(a) >> o); return asuint(asint(a << (32u - w - o)) >> (32u - w)); }uint tb_mulhi_u32(uint a, uint b) { uint a0 = a & 0xffffu, a1 = a >> 16, b0 = b & 0xffffu, b1 = b >> 16; uint p00 = a0 * b0, p01 = a0 * b1, p10 = a1 * b0, p11 = a1 * b1; uint mid = (p00 >> 16) + (p01 & 0xffffu) + (p10 & 0xffffu); return p11 + (p01 >> 16) + (p10 >> 16) + (mid >> 16); }
uint tb_dot4_i8(uint a, uint b, uint acc) {
    [unroll] for (uint i = 0; i < 4; ++i) {
        acc += asuint((asint(a << 24) >> 24) * (asint(b << 24) >> 24));
        a >>= 8; b >>= 8;
    }
    return acc;
}
uint tb_mulhi_i32(uint a, uint b) { uint h = tb_mulhi_u32(a, b); if (asint(a) < 0) h -= b; if (asint(b) < 0) h -= a; return h; }
float tb_cubema(float x, float y, float z) { float ax = abs(x), ay = abs(y), az = abs(z); if (az >= ax && az >= ay) return 2.0 * z; if (ay >= ax) return 2.0 * y; return 2.0 * x; }
float tb_cubesc(float x, float y, float z) { float ax = abs(x), ay = abs(y), az = abs(z); if (az >= ax && az >= ay) return z < 0.0 ? -x : x; if (ay >= ax) return x; return x < 0.0 ? z : -z; }
float tb_cubetc(float x, float y, float z) { float ax = abs(x), ay = abs(y), az = abs(z); if (az >= ax && az >= ay) return -y; if (ay >= ax) return y < 0.0 ? -z : z; return -y; }
float tb_cubeid(float x, float y, float z) { float ax = abs(x), ay = abs(y), az = abs(z); if (az >= ax && az >= ay) return z < 0.0 ? 5.0 : 4.0; if (ay >= ax) return y < 0.0 ? 3.0 : 2.0; return x < 0.0 ? 1.0 : 0.0; }
float3 tb_cube_dir(float s, float t, float face) { float u = (s - 1.5) * 2.0, v = (t - 1.5) * 2.0; int f = (int)round(face); if (f == 0) return float3(1.0, -v, -u); if (f == 1) return float3(-1.0, -v, u); if (f == 2) return float3(u, 1.0, v); if (f == 3) return float3(u, -1.0, -v); if (f == 4) return float3(u, -v, 1.0); return float3(-u, -v, -1.0); }
float tb_mull(float a, float b) { return (a == 0.0 || b == 0.0) ? 0.0 : a * b; }
uint tb_f16_cvt(float x, bool rtz) { uint u = asuint(x); uint s = (u >> 16) & 0x8000u; uint a = u & 0x7fffffffu; if (a > 0x7f800000u) return s | 0x7e00u | ((a >> 13) & 0x3ffu); if (a == 0x7f800000u) return s | 0x7c00u; if (rtz) { if (a >= 0x477fe000u) return s | 0x7bffu; } else { if (a >= 0x477ff000u) return s | 0x7c00u; } if (a >= 0x38800000u) { uint r = (a >> 13) - 0x1c000u; uint rem = a & 0x1fffu; if (!rtz && (rem > 0x1000u || (rem == 0x1000u && (r & 1u) != 0u))) r += 1u; return s | r; } if (a < 0x33000000u) return s; uint e = a >> 23; uint mant = (a & 0x7fffffu) | 0x800000u; uint sh = 126u - e; uint q = mant >> sh; uint rem = mant & ((1u << sh) - 1u); uint half = 1u << (sh - 1u); if (!rtz && (rem > half || (rem == half && (q & 1u) != 0u))) q += 1u; return s | q; }
"#;
fn clamped_product24(ins: &Insn) -> bool {
    matches!(ins.mnemonic.as_str(), "v_mul_u32_u24" | "v_mul_i32_i24" | "v_mad_u32_u24" | "v_mad_i32_i24")
        && (ins.aux.imm != 0 || ins.aux.sdwa.is_some_and(|ext| ext & (1 << 13) != 0))
}

const MAD24_SAT: &str = r#"
uint tb_mad24_sat(uint a, uint b, uint acc, bool isSigned) {
    a = isSigned ? asuint(asint(a << 8) >> 8) : (a & 0xffffffu);
    b = isSigned ? asuint(asint(b << 8) >> 8) : (b & 0xffffffu);
    uint product = a * b;
    uint low = product + acc;
    uint carry = uint(low < product);
    if (!isSigned) return tb_mulhi_u32(a, b) + carry == 0u ? low : 0xffffffffu;
    // Add the sign-extended accumulator before testing the full sum for overflow.
    int high = asint(tb_mulhi_i32(a, b)) + (asint(acc) >> 31) + int(carry);
    if (high == (asint(low) >> 31)) return low;
    return high < 0 ? 0x80000000u : 0x7fffffffu;
}
"#;
const HALF_RNE: &str = "uint tb_f16_rne(float x) { return tb_f16_cvt(x, false); }\n";
// The branch-free implementation keeps the driver control-flow graph small
// when a filter applies the conversion hundreds of times per invocation.
const HALF_RNE_STRAIGHT_LINE: &str = r#"uint tb_f16_rne(float x) {
    uint bits = asuint(x), a = bits & 0x7fffffffu;
    uint sign = (bits >> 16) & 0x8000u;
    uint normal = min(((a + 0xfffu + ((a >> 13) & 1u)) >> 13) - 0x1c000u, 0x7c00u);
    // FP32 spacing at 0.5 is exactly one half-subnormal unit, 2^-24.
    precise float sub = 0.5f + asfloat(a);
    uint finite = a >= 0x38800000u ? normal : asuint(sub) - 0x3f000000u;
    return sign | (a > 0x7f800000u ? 0x7e00u | ((a >> 13) & 0x3ffu) : finite);
}
// The min/max of two half values is already representable, including after
// saturation. Keep the guest quiet-NaN payload rule for the exceptional case.
uint tb_repack_half(float value) {
    uint bits = asuint(value);
    return (bits & 0x7fffffffu) > 0x7f800000u ? (((bits >> 16u) & 0x8000u) | 0x7e00u | ((bits >> 13u) & 0x3ffu)) : f32tof16(value);
}
"#;
const HALF_HELPERS: &str = r#"uint tb_trig_f16(uint bits, bool cosine) {
    uint magnitude = bits & 0x7fffu;
    if (magnitude > 0x7c00u) return (bits & 0xffffu) | 0x200u;
    if (magnitude == 0x7c00u) return 0xfe00u;
    if (magnitude == 0u) return cosine ? 0x3c00u : (bits & 0x8000u);
    // Guest arguments are turns. Reduce before converting to radians so that
    // large integral turns and multiples of a quarter turn stay exact.
    float turn = frac(f16tof32(magnitude));
    if (cosine) turn = frac(turn + 0.25);
    float halfTurn = turn >= 0.5 ? turn - 0.5 : turn;
    float angle = halfTurn <= 0.25 ? halfTurn : 0.5 - halfTurn;
    uint result = tb_f16_rne(sin(angle * 6.283185307179586));
    bool negative = (turn >= 0.5) != (!cosine && (bits & 0x8000u) != 0u);
    return result | ((result != 0u && negative) ? 0x8000u : 0u);
}
uint tb_f16_rtz(float x) { return tb_f16_cvt(x, true); }
bool tb_isnan(float x) { return (asuint(x) & 0x7fffffffu) > 0x7f800000u; }
float tb_min(float a, float b) { return tb_isnan(a) ? b : (tb_isnan(b) ? a : (a < b ? a : b)); }
float tb_max(float a, float b) { return tb_isnan(a) ? b : (tb_isnan(b) ? a : (a > b ? a : b)); }
float tb_med3(float a, float b, float c) { if (tb_isnan(a) || tb_isnan(b) || tb_isnan(c)) return tb_min(tb_min(a, b), c); float mx = tb_max(tb_max(a, b), c); if (mx == a) return tb_max(b, c); if (mx == b) return tb_max(a, c); return tb_max(a, b); }
"#;

/// Prove the non-returning 2D, DMASK=3 UMAX encoding and return its X/Y VGPRs.
/// NSA accepts one extension word with zero unused bytes; A16/D16 stay rejected.
fn wide_atomic_coordinates(ins: &Insn) -> Result<[u8; 2], String> {
    let nsa = match ins.raw.as_slice() {
        [0xf05c1308, flags] if flags & 0xc000_0000 == 0 => false,
        [0xf05c130a, flags, coordinates]
            if flags & 0xc000_0000 == 0 && coordinates & 0xffff_ff00 == 0 => true,
        _ => return Err("unsupported 64-bit image access flags".into()),
    };
    if ins.enc != Enc::Mimg || ins.mnemonic != "image_atomic_umax"
        || ins.aux.imm != 1 || ins.aux.mask != 3 || ins.aux.imm2 != i64::from(nsa)
    {
        return Err("unsupported 64-bit image access flags".into());
    }
    let Some(Opnd::Vgpr(x)) = ins.src.first().copied() else {
        return Err("invalid image coordinates".into());
    };
    if nsa {
        let Some(Opnd::Vgpr(y)) = ins.src.get(1).copied() else {
            return Err("invalid image coordinates".into());
        };
        if x != ins.raw[1] as u8 || y != ins.raw[2] as u8 {
            return Err("invalid image coordinates".into());
        }
        Ok([x, y])
    } else {
        Ok([x, x.checked_add(1).ok_or("64-bit image coordinate register overflow")?])
    }
}

/// Number of VGPRs occupied by the MIMG data operand. Gather selects a channel
/// with DMASK but returns four texels. D16 packs two components per register.
fn image_data_registers(ins: &Insn) -> u8 {
    let components = if ins.mnemonic.starts_with("image_gather4") {
        4
    } else {
        (ins.aux.mask & 0xf).count_ones() as u8
    };
    if ins.raw.get(1).is_some_and(|word| word >> 31 != 0) {
        components.div_ceil(2)
    } else {
        components
    }
}

fn name(o: &Opnd) -> Result<String, String> {
    Ok(match o {
        Opnd::Sgpr(n) => format!("s{n}"),
        Opnd::Vgpr(n) => format!("v{n}"),
        Opnd::VccLo => "vcc_lo".into(),
        Opnd::VccHi => "vcc_hi".into(),
        Opnd::ExecLo => "exec_lo".into(),
        Opnd::ExecHi => "exec_hi".into(),
        Opnd::M0 => "m0".into(),
        Opnd::Scc => "scc".into(),
        Opnd::Null => "nul".into(),
        Opnd::Temp(n) => format!("tb_s{n}"),
        other => return Err(format!("operand {other:?} is not assignable")),
    })
}

// Return the bits of this encoded field that can be read back or updated,
// relative to its starting offset. MODE has a gap before FP16_OVFL (bit 23).
fn modeled_hwreg_mask(word: u32) -> u32 {
    let known = match word & 63 {
        1 => 0x0080_03ff,
        20 | 21 => u32::MAX, // FLAT_SCRATCH_LO/HI
        _ => 0,
    };
    let shift = (word >> 6) & 31;
    let encoded_width = (word >> 11) & 31;
    (known >> shift) & (u32::MAX >> (31 - encoded_width))
}

/// Operand as a `uint` rvalue expression.
fn r(o: &Opnd) -> Result<String, String> {
    Ok(match o {
        Opnd::ConstInt(v) => format!("{:#x}u", *v as i32 as u32),
        Opnd::ConstF32(f) => format!("{:#x}u", f.to_bits()),
        Opnd::Literal(v) => format!("{v:#x}u"),
        Opnd::Raw(c) => return Err(format!("unmodelled operand code {c}")),
        other => name(other)?,
    })
}

fn f(o: &Opnd) -> Result<String, String> {
    Ok(format!("asfloat({})", r(o)?))
}

// Selector for tb_unary_f16, constant at each translated instruction.
fn half_unary_op(mnemonic: &str) -> Option<u8> {
    match mnemonic {
        "v_floor_f16" => Some(0),
        "v_ceil_f16" => Some(1),
        "v_trunc_f16" => Some(2),
        "v_rndne_f16" => Some(3),
        "v_fract_f16" => Some(4),
        _ => None,
    }
}

fn integer_to_half(mnemonic: &str) -> bool {
    matches!(mnemonic, "v_cvt_f16_u16" | "v_cvt_f16_i16")
}

fn normalized_half_to_integer(mnemonic: &str) -> bool {
    matches!(mnemonic, "v_cvt_norm_u16_f16" | "v_cvt_norm_i16_f16")
}

fn half_round_or_conversion(m: &str) -> bool {
    half_unary_op(m).is_some() || integer_to_half(m) || half_to_integer(m)
}

// Selector 248 is the only inline float whose binary16 value differs from its binary32 value.
fn inline_half_value(value: f32) -> f32 {
    if value.to_bits() == 0x3e22_f983 { f32::from_bits(0x3e23_0000) } else { value }
}

fn half_operand_bits(o: &Opnd) -> Result<String, String> {
    match o {
        Opnd::ConstF32(_) => Ok(format!("tb_f16_rne({})", f(o)?)),
        _ => r(o),
    }
}

fn half_to_integer(mnemonic: &str) -> bool {
    matches!(mnemonic, "v_cvt_u16_f16" | "v_cvt_i16_f16") || normalized_half_to_integer(mnemonic)
}

fn mul24_expr(op: &str, a: &str, b: &str) -> Option<String> {
    Some(match op {
        "v_mul_u32_u24" => format!("(({a} & 0xffffffu) * ({b} & 0xffffffu))"),
        "v_mul_i32_i24" => format!("asuint((asint({a} << 8) >> 8) * (asint({b} << 8) >> 8))"),
        "v_mul_hi_u32_u24" => format!("tb_mulhi_u32({a} & 0xffffffu, {b} & 0xffffffu)"),
        "v_mul_hi_i32_i24" => format!("tb_mulhi_i32(asuint(asint({a} << 8) >> 8), asuint(asint({b} << 8) >> 8))"),
        _ => return None,
    })
}

/// Low `width` bits set, saturated to one word. Keep full-width shifts out of the generated code.
fn low_mask(width: &str) -> String {
    format!("(({width}) >= 32u ? 0xffffffffu : ((1u << (({width}) & 31u)) - 1u))")
}

/// Signed addition overflows when both operands have the same sign and the sum changes it.
/// SCC of `s_lshl<n>_add_u32`: set when `(a << shift) + b` exceeds 32 bits.
fn shift_add_carry(a: &str, b: &str, result: &str, shift: u32) -> String {
    format!("((({a}) >> {}) != 0u || {result} < {b}) ? 1u : 0u", 32 - shift)
}

fn signed_add_overflow(a: &str, b: &str, sum: &str) -> String {
    format!("((~({a} ^ {b}) & ({a} ^ {sum})) >> 31)")
}

/// Scalar register number of an operand (SGPRs, plus vcc_lo/vcc_hi = 106/107, which SMEM
/// loads can target directly: `s_buffer_load_dwordx2 vcc_lo, ...`).
fn sreg_num(o: &Opnd) -> Option<u8> {
    match o {
        Opnd::Sgpr(n) => Some(*n),
        Opnd::VccLo => Some(106),
        Opnd::VccHi => Some(107),
        _ => None,
    }
}

fn sreg_opnd(n: u8) -> Opnd {
    match n {
        106 => Opnd::VccLo,
        107 => Opnd::VccHi,
        n => Opnd::Sgpr(n),
    }
}

/// High half of a 64-bit scalar operand.
fn hi(o: &Opnd) -> Opnd {
    match o {
        Opnd::Sgpr(n) => Opnd::Sgpr(n + 1),
        Opnd::VccLo => Opnd::VccHi,
        Opnd::ExecLo => Opnd::ExecHi,
        Opnd::ConstInt(v) => Opnd::ConstInt(if *v < 0 { -1 } else { 0 }),
        Opnd::Null => Opnd::Null,
        _ => Opnd::ConstInt(0),
    }
}

/// Name of the SGPR used as an SMEM dynamic offset (SOFFSET field).
fn ssrc_name(code: u32) -> Result<String, String> {
    match code {
        0..=107 => r(&sreg_opnd(code as u8)),
        _ => Err(format!("unsupported SOFFSET register code {code}")),
    }
}

fn sig_type(e: &SigElement) -> (String, usize) {
    // Component count = number of mask bits (elements can be packed into a shared register,
    // e.g. TEXCOORD4.xy + TEXCOORD5.zw, so the highest bit is not the count).
    let comps = (e.mask.count_ones() as usize).max(1);
    let base = match e.comp_type {
        1 => "uint",
        2 => "int",
        _ => "float",
    };
    (if comps == 1 { base.to_string() } else { format!("{base}{comps}") }, comps)
}

/// Map a hardware (register, component) to (signature element index, component within
/// that element). RDNA2 attributes / parameter exports are whole registers; a register may
/// hold several packed signature elements.
fn sig_lookup(sig: &[SigElement], register: u32, chan: usize) -> Option<(usize, usize)> {
    sig.iter().enumerate().find_map(|(idx, e)| {
        if e.sysval == 0 && e.register == register && (e.mask >> chan) & 1 != 0 {
            Some((idx, (e.mask & ((1u8 << chan) - 1)).count_ones() as usize))
        } else {
            None
        }
    })
}

/// Distinct registers of the given elements, in signature order.
fn distinct_regs<'s>(it: impl Iterator<Item = &'s SigElement>) -> Vec<u32> {
    let mut v: Vec<u32> = Vec::new();
    for e in it {
        if !v.contains(&e.register) {
            v.push(e.register);
        }
    }
    v
}

fn semantic(e: &SigElement) -> String {
    format!("{}{}", e.name, e.index)
}

/// HLSL expression for one channel of a typed buffer element in format
/// 16_16_16_16_UINT (69): 8 bytes, 4 x 16-bit unsigned channels.
fn tbuf_16_16_16_16_uint_channel(channel: u32, packed: &str, packed1: &str) -> String {
    let word = if channel >= 2 { packed1 } else { packed };
    format!("(({word} >> {}u) & 65535u)", (channel % 2) * 16)
}

/// HLSL expression for `s_ff1_i32_b64`: first set bit of a 64-bit value read
/// through its two 32-bit words.
fn ff1_b64_expr(lo: &str, hi: &str) -> String {
    format!(
        "({lo} != 0u ? uint(firstbitlow({lo})) : {hi} != 0u ? 32u + uint(firstbitlow({hi})) : 0xffffffffu)"
    )
}

/// HLSL expression for `s_flbit_i32_b64`: first set bit from MSB to LSB over a 64-bit value.
fn flbit_b64_expr(lo: &str, hi: &str) -> String {
    format!("({hi} != 0u ? 31u - firstbithigh({hi}) : {lo} != 0u ? 63u - firstbithigh({lo}) : ~0u)")
}

/// HLSL expression for `s_ff0_i32_b32`: first zero bit from LSB to MSB over a 32-bit value.
fn ff0_b32_expr(val: &str) -> String {
    format!("firstbitlow(~{val})")
}

/// HLSL expression for `s_ff0_i32_b64`: first zero bit from LSB to MSB over a 64-bit value.
fn ff0_b64_expr(lo: &str, hi: &str) -> String {
    format!("(~{lo} != 0u ? uint(firstbitlow(~{lo})) : ~{hi} != 0u ? 32u + uint(firstbitlow(~{hi})) : 0xffffffffu)")
}

struct Emitter<'a> {
    lds_tile: Option<lds_tile::LdsTile>,
    packed: Option<PackedRoot>,
    ex: &'a ShaderExtract,
    insns: Vec<Insn>,
    res: HashMap<usize, Vec<Res>>, // per-instruction resolved resources
    cb_size: BTreeMap<u32, u32>,   // slot -> dwords
    zero_buffer_offsets: std::cell::OnceCell<BTreeSet<(usize, u8)>>,
    ordinary_bit_sites: std::cell::OnceCell<BTreeSet<usize>>,
    smem_constants: std::cell::OnceCell<BTreeMap<(usize, u8), u32>>,
    tex: BTreeMap<u32, u32>,       // slot -> dim
    samp: BTreeSet<u32>,
    samp_cmp: BTreeSet<u32>,
    uav: BTreeMap<u32, u32>,       // slot -> dim
    sbuf: BTreeMap<(bool, u32), u32>, // (is_uav, slot) -> element dwords used
    reflected_strides: BTreeSet<(bool, u32)>,
    /// PersistentClusterCull's validated buffer layout and cross-group UAV queue.
    persistent_queue: bool,
    raw: BTreeSet<(bool, u32)>,
    // Native sites with a proven byte-stride-one linear descriptor.
    raw_byte_index: BTreeSet<usize>,
    tbuf: BTreeSet<(bool, u32)>,      // typed buffers (is_uav, slot)
    tbuf_uint: BTreeSet<u32>,         // typed UAV slots whose element must be uint (atomics)
    /// Inferred integer element type of typed buffers: (is_uav, slot) -> 1 uint, 2 sint.
    tbuf_ty: BTreeMap<(bool, u32), u8>,
    /// Inferred integer texel type of textures read with texel fetches / written with image
    /// stores: (is_uav, slot) -> 1 uint, 2 sint. Absent = float (the default).
    img_ty: BTreeMap<(bool, u32), u8>,
    uses_heap_sampler: bool,
    resource_heap_base: Option<u8>,
    /// The heap base comes from FSSM user-data class 5. Compute reads that
    /// register directly (`compute_heap_base`); mesh adds the eight system SGPRs
    /// (`mesh_heap_base`) and then uses this same lowering.
    compute_heap: bool,
    /// A root-less pixel shader that indexes a descriptor heap (`pixel_heap_base`).
    pixel_heap: bool,
    /// Sampler registers bound through the root signature. A heap sampler with the same
    /// number would reuse the HLSL name.
    table_samplers: BTreeSet<u32>,
    /// Vertex programs: how the values of each typed heap load are used, as
    /// [float or unknown, uint, sint].
    heap_load_ty: HashMap<usize, [bool; 3]>,
    /// The root carries TuxBox's guest-to-host heap index map (root SRV t0, space32766).
    heap_map: bool,
    /// Compute sampler heap base SGPR (FSSM user-data class 6).
    sampler_heap_base: Option<u8>,
    /// Sampler slots read from that heap: bound from SamplerDescriptorHeap in main.
    heap_sampler_slots: BTreeSet<u32>,
    private_samplers: BTreeMap<usize, u32>,
    binding_spaces: BTreeMap<(u32, u32), u32>,
    mesh_bindings: Option<mesh_bindings::MeshBindings>,
    compute_derivatives: bool,
    /// Pixel-shader colour-export epilogues (see `find_export_epilogs`): pack instruction
    /// index -> capture id, and export instruction index -> the capture ids feeding its
    /// two packed operands (usize::MAX = operand not written).
    epilog_pack: HashMap<usize, usize>,
    epilog_exp: HashMap<usize, (usize, usize)>,
    /// Index of the instruction being emitted.
    cur_idx: usize,
    /// Structured buffers read with scalar loads (`s_buffer_load` through an SRV/UAV
    /// descriptor): they need the byte-offset helper.
    sbuf_flat: BTreeSet<(bool, u32)>,
    is_cs: bool,
    /// Nesting depth of emitted HLSL loops at the current instruction.
    loop_depth: u32,
    is_gs: bool,
    /// Compute shader uses LDS (groupshared memory).
    uses_lds: bool,
    /// Shader reads constant tables embedded in its code section.
    uses_code: bool,
    /// The program uses register-relative moves: VGPRs live in the `tb_v` array.
    vreg_array: bool,
    /// Interned candidate lists `(table, offset)` for `Sym::Multi`.
    multi_sets: Vec<Vec<(u8, u32)>>,
    /// Image ops whose texture and/or sampler is one of several descriptors.
    /// UAV texture slots used with image atomics (declared `<uint>`).
    uav_uint: BTreeSet<u32>,
    /// Typed image slots carrying a two-word visibility value in a 64-bit atomic.
    wide_visibility_uavs: BTreeSet<u32>,
    /// Instruction where all 128 rasterizer threads vote in their logical wave.
    micropoly_vote: Option<usize>,
    view_cull: Option<instance_cull_views::ViewProgram>,
    /// The shader uses wave intrinsics (needs shader model 6: DXC only).
    required_wave_size: Option<u32>,
    // Proven whole-wave EXECZ guards of verified masked reductions.
    reduction_execz_guards: BTreeSet<usize>,
    /// Set when every branch is a decision of the whole wave (see `lockstep`),
    /// with the condition codes to vote after the instructions that set them.
    lockstep: Option<BTreeMap<usize, lockstep::Vote>>,
    /// Structured loop latches with a verified guest trip bound.
    wave_bounded_latches: BTreeSet<usize>,
    portable_wave64: bool,
    msel: HashMap<usize, MSel>,
    /// Buffer ops (MUBUF, s_buffer_load) whose descriptor register holds one of several
    /// descriptors: (register key, candidates as (descriptor id, resource)).
    bsel: HashMap<usize, (u16, Vec<(u32, Res)>)>,
    /// Descriptor loads `(base register, descriptor id)` and base-dword moves `(dst, src)`
    /// per instruction, for maintaining the run-time `tb_dsel<reg>` selectors.
    dsel_loads: HashMap<usize, (u16, u32)>,
    dsel_runtime: HashMap<usize, (u16, u8, u8, u32)>,
    dsel_moves: HashMap<usize, (u16, u16)>,
    /// Registers read before written (entry ABI), from `live_in`.
    live: Vec<String>,
    /// Pixel shader needs its position within the 2x2 quad (ds_swizzle).
    uses_quad: bool,
    compute_swizzle: bool,
    vote_defs: BTreeSet<usize>,
    vote_uses: BTreeMap<usize, usize>,
    exec_snapshots: exec_snapshot::Snapshots,
    reflected_lds_words: Option<u32>,
    hoisted_swizzle: Option<usize>,
    indexed_lane: bool,
    /// Complete 32-thread wave using independent one-word vector comparison masks.
    single_wave32: bool,
    resolve_wave32: bool,
    wave32_compare_masks: bool,
    light_grid: bool,
    page_waves: bool,
    wave_cull: bool,
    /// Mesh first-lane reads for which a lane's own value is the one read.
    own_lane_reads: BTreeSet<usize>,
    /// Mesh samples whose sampler a lane picks among sampler-heap entries; the first word of
    /// such a descriptor then holds its heap index.
    chosen_samplers: BTreeSet<usize>,
    /// Instruction ranges of a mesh program's loops.
    loop_ranges: Vec<(usize, usize)>,
    fspx_wave: Option<u32>,
    hair_multisample: bool,
    hoist_wave_exchanges: bool,
    regs: BTreeSet<String>,
    body: String,
    tmp: usize,
    /// PS attribute index -> input register (registers the shader actually reads).
    ps_attrs: Vec<Param>,
    pulled_attrs: BTreeSet<usize>,
    /// Each pixel is a wave of its own.
    pixel_wave: bool,
    dispatched: bool,
    /// The reduction being emitted adds.
    pixel_sum: bool,
    flat_attrs: BTreeSet<usize>,
    /// VS parameter-export index -> output register.
    out_params: Vec<Param>,
    /// Pixel shader: the interpolation modifier given to each input (filled by `finish`).
    ps_in_interp: InputInterp,
    /// Stage feeding a pixel shader: modifiers to put on the matching outputs.
    out_interp: InputInterp,
    /// One function-scoped resource for each heap view of an SGPR. The local
    /// captures the descriptor at assignment.
    heap_locals: BTreeSet<(HeapView, u16)>,
    /// Views whose local still holds this SGPR's current heap index.
    heap_bound: BTreeSet<(HeapView, u16)>,
}

/// HLSL resource view of one `ResourceDescriptorHeap` index.
///
/// The index at every current heap subscript is an SGPR (a scalar shift, or
/// `v_readfirstlane` into an SGPR). That value is uniform for the lanes that
/// execute the assignment, so the subscript is `ResourceDescriptorHeap[sN]`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum HeapView {
    Raw,
    RawRw,
    Constants,
    Typed,
    TypedUint,
    TypedRw,
    Tex1D,
    Tex2D,
    Tex2DUint,
    Tex2DRwUint,
    Tex3D,
    TexCube,
    Tex1DArray,
    Tex2DArray,
}

impl HeapView {
    fn hlsl(self) -> &'static str {
        match self {
            HeapView::Raw => "ByteAddressBuffer",
            HeapView::RawRw => "RWByteAddressBuffer",
            HeapView::Constants => "ConstantBuffer<TBHeapConstants>",
            HeapView::Typed => "Buffer<float4>",
            HeapView::TypedUint => "Buffer<uint4>",
            HeapView::TypedRw => "RWBuffer<float4>",
            HeapView::Tex1D => "Texture1D<float4>",
            HeapView::Tex2D => "Texture2D<float4>",
            HeapView::Tex2DUint => "Texture2D<uint4>",
            HeapView::Tex2DRwUint => "RWTexture2D<uint4>",
            HeapView::Tex3D => "Texture3D<float4>",
            HeapView::TexCube => "TextureCube<float4>",
            HeapView::Tex1DArray => "Texture1DArray<float4>",
            HeapView::Tex2DArray => "Texture2DArray<float4>",
        }
    }

    fn local(self, sgpr: u16) -> String {
        let tag = match self {
            HeapView::Raw => "raw",
            HeapView::RawRw => "raww",
            HeapView::Constants => "cb",
            HeapView::Typed => "buf",
            HeapView::TypedUint => "bufu",
            HeapView::TypedRw => "rwbuf",
            HeapView::Tex1D => "t1",
            HeapView::Tex2D => "t2",
            HeapView::Tex2DUint => "t2u",
            HeapView::Tex2DRwUint => "rwt2u",
            HeapView::Tex3D => "t3",
            HeapView::TexCube => "tc",
            HeapView::Tex1DArray => "t1a",
            HeapView::Tex2DArray => "t2a",
        };
        format!("tb_heap_{tag}_{sgpr}")
    }
}

/// Distance between the names of resources that share a register in different spaces.
const SPACE_ALIAS: u32 = 1 << 16;
/// Synthetic opcode of the wave-reduction instruction produced by `lower_wave_reductions`.
const OP_WAVE_REDUCE: u32 = 0x7ff0;
const OP_DPP_BINARY: u32 = 0x7ff5;
const OP_LOCAL_EXTREMA: u32 = 0x7fe3;
const OP_QUAD_MOVE: u32 = 0x7fe1;
const OP_ROW_XOR_MOVE: u32 = 0x7fe2;
const OP_ROW_SHARE_MOVE: u32 = 0x7fe3;
const OP_ROW_ROR_MOVE: u32 = 0x7fe4;
const OP_ROW_SHIFT_MOVE: u32 = 0x7fe5;

fn is_row_move(i: &Insn) -> bool {
    i.enc == Enc::Vop1 && matches!(i.op, OP_ROW_XOR_MOVE | OP_ROW_SHARE_MOVE | OP_ROW_ROR_MOVE | OP_ROW_SHIFT_MOVE)
}

fn is_quad_exchange(i: &Insn) -> bool {
    (i.enc == Enc::Vop2 && i.op == OP_DPP_BINARY)
        || (i.enc == Enc::Vop1 && i.op == OP_QUAD_MOVE)
        || is_row_move(i)
}

fn lower_dpp_binary(ins: &mut Insn) {
    let control = ins.raw[1];
    let selector = if group_extrema::is_dpp8(ins) { control >> 8 } else { (control >> 8) & 0x1ff };
    ins.op = OP_DPP_BINARY;
    ins.aux.imm = i64::from(selector);
    ins.src[0] = Opnd::Vgpr(control as u8);
}

fn dpp_fp32_sources(ins: &Insn, first: String) -> Result<(String, String), String> {
    // DPP8 encodes lane selectors in the same bits, and ordinary VOP2
    // instructions can have a literal extension instead of a DPP control.
    let flags = if ins.raw.first().is_some_and(|word| word & 0x1ff == 250) {
        *ins.raw.get(1).ok_or("missing DPP16 controls")? >> 20
    } else {
        0
    };
    let modified = |bits, flags: u32| {
        compare::sign_modifiers(bits, 0x8000_0000, flags & 2 != 0, flags & 1 != 0)
    };
    Ok((modified(first, flags), modified(r(&ins.src[1])?, flags >> 2)))
}

fn is_binary_quad_perm(i: &Insn) -> bool {
    if i.enc != Enc::Vop2 || i.src.first() != Some(&Opnd::Raw(250)) {
        return false;
    }
    let Some(control) = i.raw.get(1) else { return false };
    // A quad selector uses eight bits. Row and bank masks govern the write.
    if control & 0x0003_0000 != 0 {
        return false;
    }
    match i.mnemonic.as_str() {
        "v_add_f32" | "v_sub_f32" | "v_subrev_f32" | "v_mul_f32" | "v_mul_legacy_f32"
        | "v_cvt_pkrtz_f16_f32" | "v_min_f32" | "v_max_f32" => true,
        "v_or_b32" | "v_and_b32" | "v_xor_b32" | "v_xnor_b32"
        | "v_lshlrev_b32" | "v_lshrrev_b32" | "v_ashrrev_i32"
        | "v_add_nc_u32" | "v_sub_nc_u32" | "v_subrev_nc_u32"
        | "v_min_i32" | "v_max_i32" | "v_min_u32" | "v_max_u32"
        | "v_mul_i32_i24" | "v_mul_hi_i32_i24" | "v_mul_u32_u24" | "v_mul_hi_u32_u24"
        | "v_cndmask_b32" => control & 0x00f0_0000 == 0, // No FP source modifiers.
        _ => false,
    }
}

// VOP2 CNDMASK reads the destination lane's VCC. A vector compare writes a
// per-invocation predicate here, while a scalar write can hold a packed mask.
fn quad_cndmasks_with_lane_vcc(insns: &[Insn]) -> BTreeSet<usize> {
    let targets = branch_targets(insns);
    let mut selects = BTreeSet::new();
    let mut lane_vcc = false;
    for (index, ins) in insns.iter().enumerate() {
        if targets.contains(&(ins.offset as i64)) { lane_vcc = false; }
        if lane_vcc && ins.mnemonic == "v_cndmask_b32" && is_binary_quad_perm(ins) {
            selects.insert(index);
        }
        if ins.mnemonic.starts_with("v_cmpx_") { continue; } // RDNA2 CMPX writes EXEC.
        if ins.dst.first() == Some(&Opnd::VccLo) {
            lane_vcc = ins.mnemonic.starts_with("v_cmp_")
                || (ins.mnemonic == "s_mov_b64" && matches!(ins.src[..], [Opnd::ConstInt(0 | -1)]));
        } else {
            lane_vcc &= match ins.enc {
                Enc::Vop1 | Enc::Vop2 => matches!(ins.dst[..], [Opnd::Vgpr(_)])
                    && !ins.mnemonic.contains("_co_"),
                Enc::Sop1 => matches!(ins.mnemonic.as_str(), "s_mov_b32" | "s_mov_b64" | "s_wqm_b32" | "s_wqm_b64")
                    && !ins.dst.contains(&Opnd::VccHi),
                Enc::Sopp => matches!(ins.mnemonic.as_str(), "s_nop" | "s_waitcnt"),
                _ => false,
            };
        }
    }
    selects
}

/// Whole-wave reductions (HLSL `WaveActiveMin` / `WaveActiveMax`; observed in
/// CocFlattenMainCS, 4f9670de0be37288). The compiled form is:
///
/// ```text
///   s_nand_saveexec_b64 sX, 0        ; save exec, enable every lane
///   v_mov_b32     vI, <identity>
///   v_cndmask_b32 vR, vI, vIn, sX    ; inactive lanes contribute the identity
///   v_min_f32     vR, <dpp>, vR      ; DPP row/quad permutes (src0 = 233 / 250)
///   ...  v_permlanex16_b32, v_min_f32, v_readlane_b32 (lane 32), v_min_f32 ...
///   v_readlane_b32 sRes, vR, 0       ; the result, a scalar
///   s_mov_b64     exec_lo, sX        ; restore exec
/// ```
///
/// A per-lane translation cannot express the cross-lane data movement, but the net effect
/// is exactly a wave reduction, so the region is replaced by: `sX = exec` (exec itself is
/// left alone), the identity move, and one synthetic instruction
/// `sRes = WaveActive{Min,Max}(lane active ? vIn : identity)`. Needs shader model 6 (DXC).
///
/// Inferred, not documented: operand codes 233/250 as DPP with a trailing control dword and
/// SOP1 0x29 as nand-saveexec are taken from the public GFX10 encoding and are consistent
/// with this shader's instruction stream.
/// Anything in the region that does not fit this shape is refused.
fn lower_wave_reductions(insns: &mut [Insn]) -> Result<Option<u32>, String> {
    for op in insns.iter_mut().filter(|op| op.enc == Enc::Vop1 && op.mnemonic == "v_mov_b32" && op.src == [Opnd::Raw(250)]) {
        let ctrl = *op.raw.get(1).ok_or("missing DPP16 move controls")?;
        let src0_reg = ctrl & 0xff;
        let ctrl_field = i64::from((ctrl >> 8) & 0x1ff);
        let neg0 = (ctrl >> 20) & 1;
        let abs0 = (ctrl >> 21) & 1;
        let unsupported = || format!("unsupported DPP16 move at {:#x}", op.offset);
        if neg0 != 0 || abs0 != 0 {
            return Err(unsupported());
        }
        let (opcode, selector) = match ctrl_field {
            0..=0xff => (OP_QUAD_MOVE, ctrl_field),
            0x101..=0x10f => (OP_ROW_SHIFT_MOVE, ctrl_field - 0x100), // row_shl
            0x111..=0x11f => (OP_ROW_SHIFT_MOVE, 0x110 - ctrl_field), // row_shr
            0x121..=0x12f => (OP_ROW_ROR_MOVE, ctrl_field - 0x120),
            0x140 => (OP_ROW_XOR_MOVE, 15), // row_mirror
            0x141 => (OP_ROW_XOR_MOVE, 7),  // row_half_mirror
            0x150..=0x15f => (OP_ROW_SHARE_MOVE, ctrl_field - 0x150), // row_share
            0x160..=0x16f => (OP_ROW_XOR_MOVE, ctrl_field - 0x160), // row_xmask
            _ => return Err(unsupported()),
        };
        op.op = opcode;
        op.src[0] = Opnd::Vgpr(src0_reg as u8);
        op.aux.imm = selector;
    }
    let mut width = None;
    let mut proven_cndmasks = None;
    let is_dpp = |i: &Insn| i.enc == Enc::Vop2 && matches!(i.src.first(), Some(Opnd::Raw(233 | 234 | 250)));
    let mut at = 0;
    while let Some(first) = (at..insns.len()).find(|&i| is_dpp(&insns[i])) {
        let off = insns[first].offset;
        let bad = |what: &str| format!("wave reduction at {off:#x}: {what}");
        let red = insns[first].mnemonic.clone();
        // A full saved-EXEC extrema reduction can begin with quad shuffles,
        // then combine different quads. A quad followed solely by a lane read
        // still selects within its quad and must not become a whole-wave min/max.
        let starts_wave_extrema = matches!(red.as_str(), "v_min_f32" | "v_max_f32")
            && first >= 3 && (insns[first - 3].enc, insns[first - 3].op) == (Enc::Sop1, 0x29)
            && insns[first - 2].mnemonic == "v_mov_b32" && insns[first - 1].mnemonic == "v_cndmask_b32"
            && insns[first + 1..].iter()
                .take_while(|i| is_dpp(i) && i.mnemonic == red && i.dst == insns[first].dst)
                .any(|i| !is_binary_quad_perm(i));
        if is_binary_quad_perm(&insns[first]) && !starts_wave_extrema {
            if red == "v_cndmask_b32" && !proven_cndmasks
                .get_or_insert_with(|| quad_cndmasks_with_lane_vcc(insns)).contains(&first) {
                return Err(bad("quad CNDMASK requires a known per-lane VCC predicate"));
            }
            lower_dpp_binary(&mut insns[first]);
            at = first + 1;
            continue;
        }
        if red == "v_add_f32" && insns[first].src.first() == Some(&Opnd::Raw(250)) {
            return Err(bad("unsupported DPP16 FP32 add controls"));
        }
        let saved_extrema = matches!(red.as_str(), "v_min_f32" | "v_max_f32")
            && (first.checked_sub(3).is_some_and(|p| matches!(insns[p].mnemonic.as_str(), "s_or_saveexec_b32" | "s_or_saveexec_b64"))
                || wave_sum::masked_floating(insns, first));
        if matches!(red.as_str(), "v_add_nc_u32" | "v_min_u32" | "v_max_u32" | "v_min_i32" | "v_max_i32" | "v_add_f32" | "v_and_b32" | "v_or_b32" | "v_xor_b32") || saved_extrema {
            let (required, next) = wave_sum::lower(insns, first).map_err(bad)?;
            if width.replace(required).is_some_and(|w| w != required) {
                return Err(bad("conflicting integer sum wave widths"));
            }
            at = next;
            continue;
        }
        if !matches!(red.as_str(), "v_min_f32" | "v_max_f32") {
            return Err(bad(&format!("unsupported reduction op {red}")));
        }
        let Some(Opnd::Vgpr(vr)) = insns[first].dst.first().copied() else { return Err(bad("no accumulator")) };
        // Prologue: nand_saveexec, identity move, cndmask.
        if first < 3 {
            return Err(bad("no prologue"));
        }
        let (a, mv, cm) = (first - 3, first - 2, first - 1);
        let saved = match (&insns[a].enc, insns[a].op, insns[a].dst.first(), insns[a].src.first()) {
            (Enc::Sop1, 0x29, Some(Opnd::Sgpr(x)), Some(Opnd::ConstInt(0))) => *x,
            _ => return Err(bad("prologue does not start with s_nand_saveexec_b64 sX, 0")),
        };
        let (ident_reg, ident) = match (insns[mv].mnemonic.as_str(), insns[mv].dst.first(), insns[mv].src.first()) {
            ("v_mov_b32", Some(Opnd::Vgpr(v)), Some(Opnd::Literal(k))) => (*v, *k),
            _ => return Err(bad("identity is not a literal move")),
        };
        let c = &insns[cm];
        let vin = match (c.mnemonic.as_str(), c.dst.first(), c.src.first(), c.src.get(1), c.src.get(2)) {
            ("v_cndmask_b32", Some(Opnd::Vgpr(d)), Some(Opnd::Vgpr(i)), Some(Opnd::Vgpr(x)), Some(Opnd::Sgpr(m)))
                if *d == vr && *i == ident_reg && *m == saved =>
            {
                *x
            }
            _ => return Err(bad("unexpected select before the reduction")),
        };
        // Body up to the exec restore.
        let mut end = None;
        let mut last_read = None;
        for i in first..insns.len() {
            let x = &insns[i];
            if x.mnemonic == "s_mov_b64" && x.dst.first() == Some(&Opnd::ExecLo) && x.src.first() == Some(&Opnd::Sgpr(saved)) {
                end = Some(i);
                break;
            }
            let ok = (x.enc == Enc::Vop2 && x.mnemonic == red && x.dst.first() == Some(&Opnd::Vgpr(vr)))
                || (x.enc == Enc::Vop3 && x.op == 0x378 && x.src.first() == Some(&Opnd::Vgpr(vr)))
                || (x.enc == Enc::Vop3 && x.op == 0x360 && x.src.first() == Some(&Opnd::Vgpr(vr)));
            if !ok {
                return Err(bad(&format!("unexpected {} at {:#x} inside the reduction", x.mnemonic, x.offset)));
            }
            if x.enc == Enc::Vop3 && x.op == 0x360 {
                last_read = Some(i);
            }
        }
        let (Some(end), Some(last)) = (end, last_read) else { return Err(bad("no exec restore / result read")) };
        if insns[last].src.get(1) != Some(&Opnd::ConstInt(0)) {
            return Err(bad("result is not read from lane 0"));
        }
        let res = match insns[last].dst.first() {
            Some(Opnd::Vgpr(n)) | Some(Opnd::Sgpr(n)) => *n,
            _ => return Err(bad("bad result register")),
        };
        // Rewrite.
        insns[a].mnemonic = "s_mov_b64".into();
        insns[a].op = 4;
        insns[a].src = vec![Opnd::ExecLo];
        for i in cm..end {
            let x = &mut insns[i];
            x.enc = Enc::Vop1;
            x.op = 0;
            x.mnemonic = "v_nop".into();
            x.dst = vec![Opnd::Vgpr(vr)];
            x.src = vec![];
            x.aux = Default::default();
        }
        let x = &mut insns[last];
        x.enc = Enc::Vop3;
        x.op = OP_WAVE_REDUCE;
        x.mnemonic = if red == "v_min_f32" { "tb_wave_min_f32".into() } else { "tb_wave_max_f32".into() };
        x.dst = vec![Opnd::Sgpr(res)];
        x.src = vec![Opnd::Vgpr(vin), Opnd::Sgpr(saved)];
        x.aux.imm = ident as i64;
        at = end + 1;
    }
    Ok(width)
}

use crate::rdna2::{wide_bit_result, writes_scalar};

/// Translator temporaries from this index up are lane-spill slots (declared up front).
const LANE_TEMP_BASE: u16 = 10_000;

/// SGPR spills through vector lanes.
///
/// When a shader runs out of scalar registers the console compiler parks scalars (observed:
/// texture/sampler descriptor dwords) in individual lanes of an otherwise unused VGPR with
/// `v_writelane_b32 vN, sX, <lane>` and brings them back with `v_readlane_b32 sY, vN, <lane>`
/// (VOP3 0x361 / 0x360; fixture `fortnite_lobby_desc.dxbc`). The translator models one lane
/// per VGPR, so each (VGPR, constant lane) slot is lowered to a function-scope translator
/// temporary and the two instructions become plain `s_mov_b32`. The descriptor dataflow
/// tracks those temporaries like registers (keys 1000+), so descriptors survive a spill.
///
/// Refused (explicit error): a non-constant lane index, a read of a slot that was never
/// written, or a spill VGPR that is also used as an ordinary vector.
/// A read of one lane of the caller's 64-lane block, exchanged like `ds_swizzle_b32`.
const GROUP_READ: &str = "tb_group_readlane";

fn exchanges(i: &Insn) -> bool { i.mnemonic == "ds_swizzle_b32" || i.mnemonic == GROUP_READ }

/// A pixel is a wave of its own, so the first lane of a mask that covers it is its own.
/// `s_ff1` names that lane for `v_readlane`; anything else that uses the lane is left alone.
fn lower_first_lane_reads(insns: &mut [Insn]) {
    for i in 0..insns.len() {
        if insns[i].mnemonic != "v_readlane_b32" { continue; }
        let Some(&Opnd::Sgpr(lane)) = insns[i].src.get(1) else { continue };
        let Some(first) = (i.saturating_sub(64)..i).rev()
            .find(|&j| insns[j].dst.contains(&Opnd::Sgpr(lane)) || insns[j].enc == Enc::Sopp && insns[j].mnemonic.contains("branch"))
        else { continue };
        if !insns[first].mnemonic.starts_with("s_ff1_i32_b") { continue; }
        // The destination field names an SGPR (decoded generically as a VGPR).
        let Some(&(Opnd::Vgpr(result) | Opnd::Sgpr(result))) = insns[i].dst.first() else { continue };
        let reads = |j: usize| j != i && insns[j].src.contains(&Opnd::Sgpr(lane));
        if (first + 1..i).any(reads) { continue; }
        if result != lane {
            let end = (i + 1..insns.len()).find(|&j| insns[j].dst.contains(&Opnd::Sgpr(lane)) || insns[j].mnemonic.contains("branch"));
            if end.is_none_or(|end| (i + 1..=end).any(reads) || insns[end].mnemonic.contains("branch")) { continue; }
        }
        insns[first].mnemonic = "s_mov_b32".into();
        insns[first].op = 3;
        insns[first].src = vec![Opnd::ConstInt(0)];
        let ins = &mut insns[i];
        ins.enc = Enc::Vop1;
        ins.op = 2;
        ins.mnemonic = "v_readfirstlane_b32".into();
        ins.dst = vec![Opnd::Sgpr(result)];
        ins.src.truncate(1);
    }
}

fn lower_lane_spills(insns: &mut [Insn], wave32: bool, group_reads: bool) -> Result<(), String> {
    const READLANE: u32 = 0x360;
    const WRITELANE: u32 = 0x361;
    let is_lane = |i: &Insn| i.enc == Enc::Vop3 && (i.op == READLANE || i.op == WRITELANE);
    if !insns.iter().any(is_lane) {
        return Ok(());
    }
    let mut slots: BTreeMap<(u8, i64), Opnd> = BTreeMap::new();
    let mut spill_vregs: BTreeSet<u8> = BTreeSet::new();
    let written_vregs: BTreeSet<_> = insns.iter()
        .filter(|ins| wave32 && ins.enc == Enc::Vop3 && ins.op == WRITELANE)
        .filter_map(|ins| match ins.dst.first() {
            Some(Opnd::Vgpr(v)) => Some(*v),
            _ => None,
        })
        .collect();
    let any_written: BTreeSet<_> = insns.iter()
        .filter(|ins| ins.enc == Enc::Vop3 && ins.op == WRITELANE)
        .filter_map(|ins| match ins.dst.first() {
            Some(Opnd::Vgpr(v)) => Some(*v),
            _ => None,
        })
        .collect();
    for ins in insns.iter_mut() {
        if !is_lane(ins) {
            continue;
        }
        let at = ins.offset;
        if wave32 && ins.op == READLANE {
            if let ([Opnd::Vgpr(d) | Opnd::Sgpr(d)], [Opnd::Vgpr(v), lane, ..]) =
                (ins.dst.as_slice(), ins.src.as_slice())
            {
                if !written_vregs.contains(v) {
                    ins.dst = vec![Opnd::Sgpr(*d)];
                    ins.src = vec![Opnd::Vgpr(*v), *lane];
                    ins.enc = Enc::Sop1;
                    ins.mnemonic = ray_resolve::READ_LANE.into();
                    continue;
                }
            }
        }
        // A uniform Sgpr lane operand is only expressible through the whole-wave
        // group read; anything else stays refused. Spill areas (vregs written
        // by v_writelane) are still slotted per constant lane, so they remain
        // refused for dynamic lanes.
        if !wave32 && group_reads && ins.op == READLANE {
            let converted = match (ins.src.first(), ins.src.get(1), ins.dst.first()) {
                (Some(Opnd::Vgpr(v)), Some(Opnd::Sgpr(_)),
                 Some(Opnd::Vgpr(_)) | Some(Opnd::Sgpr(_)))
                    if !any_written.contains(v) =>
                {
                    let v = *v;
                    let d = match ins.dst.first() {
                        Some(Opnd::Vgpr(n)) | Some(Opnd::Sgpr(n)) => *n,
                        _ => unreachable!(),
                    };
                    ins.dst = vec![Opnd::Sgpr(d)];
                    ins.src = vec![Opnd::Vgpr(v), ins.src[1].clone()];
                    ins.enc = Enc::Sop1;
                    ins.mnemonic = GROUP_READ.into();
                    true
                }
                _ => false,
            };
            if converted {
                continue;
            }
        }
        let lane = match ins.src.get(1) {
            Some(Opnd::ConstInt(l)) if (0..64).contains(l) => *l,
            other => return Err(format!("lane op at {at:#x}: non-constant lane operand {other:?}")),
        };
        if ins.op == WRITELANE {
            let Some(Opnd::Vgpr(v)) = ins.dst.first().cloned() else {
                return Err(format!("v_writelane at {at:#x}: destination is not a VGPR"));
            };
            let next = slots.len();
            let slot = match slots.get(&(v, lane)) {
                Some(s) => *s,
                None => {
                    let s = Opnd::Temp(LANE_TEMP_BASE + next as u16);
                    slots.insert((v, lane), s);
                    s
                }
            };
            spill_vregs.insert(v);
            let value = ins.src.first().cloned().ok_or_else(|| format!("v_writelane at {at:#x}: no source"))?;
            ins.dst = vec![slot];
            ins.src = vec![value];
        } else {
            // The destination field of v_readlane names an SGPR (decoded generically as a VGPR).
            let d = match ins.dst.first() {
                Some(Opnd::Vgpr(n)) | Some(Opnd::Sgpr(n)) => *n,
                other => return Err(format!("v_readlane at {at:#x}: unexpected destination {other:?}")),
            };
            let Some(Opnd::Vgpr(v)) = ins.src.first().cloned() else {
                return Err(format!("v_readlane at {at:#x}: source is not a VGPR"));
            };
            if group_reads && !any_written.contains(&v) {
                ins.dst = vec![Opnd::Sgpr(d)];
                ins.src = vec![Opnd::Vgpr(v), Opnd::ConstInt(lane)];
                ins.enc = Enc::Sop1;
                ins.mnemonic = GROUP_READ.into();
                continue;
            }
            let Some(&slot) = slots.get(&(v, lane)) else {
                return Err(format!("v_readlane at {at:#x}: v{v} lane {lane} read before any v_writelane"));
            };
            ins.dst = vec![Opnd::Sgpr(d)];
            ins.src = vec![slot];
        }
        ins.enc = Enc::Sop1;
        ins.op = 0x03;
        ins.mnemonic = "s_mov_b32".into();
    }
    for ins in insns.iter() {
        if ins.mnemonic == "v_nop" {
            continue; // the two header words at the start of every program decode as v_nop
        }
        for o in ins.dst.iter().chain(ins.src.iter()) {
            if let Opnd::Vgpr(v) = o {
                if spill_vregs.contains(v) {
                    return Err(format!("v{v} is used both as a lane spill area and as a vector at {:#x}", ins.offset));
                }
            }
        }
    }
    Ok(())
}

/// Resolve local PC transfers when GETPC and a constant add form one basic block.
fn lower_local_pc_transfers(insns: &mut [Insn]) -> Result<(), String> {
    if !insns
        .iter()
        .any(|i| matches!(i.mnemonic.as_str(), "s_setpc_b64" | "s_swappc_b64"))
    {
        return Ok(());
    }
    let mut entries: BTreeSet<i64> = insns
        .iter()
        .filter(|i| i.enc == Enc::Sopp && Emitter::branch_cond(i.op).is_some())
        .map(|i| i.offset as i64 + 4 + 4 * i.aux.imm)
        .collect();
    let mut local = Vec::new();
    for (i, seq) in insns.windows(3).enumerate() {
        let [pc, add, jump] = seq else { unreachable!() };
        if !matches!(jump.mnemonic.as_str(), "s_setpc_b64" | "s_swappc_b64")
            || pc.mnemonic != "s_getpc_b64"
            || add.mnemonic != "s_add_u32"
            || add.dst.first() != pc.dst.first()
            || add.src.first() != pc.dst.first()
            || jump.src.first() != pc.dst.first()
        {
            continue;
        }
        let offset = match add.src.get(1) {
            Some(Opnd::ConstInt(k)) => *k,
            Some(Opnd::Literal(k)) => i64::from(*k as i32),
            _ => continue,
        };
        let target = pc.offset as i64 + 4 + offset;
        if !insns.iter().any(|x| x.offset as i64 == target) {
            return Err(format!(
                "PC transfer at {:#x} targets outside the decoded instructions",
                jump.offset
            ));
        }
        entries.insert(target);
        local.push((i, target));
    }
    for (i, target) in local {
        if entries.contains(&(insns[i + 1].offset as i64)) || entries.contains(&(insns[i + 2].offset as i64)) {
            return Err("branch enters a local PC-transfer sequence after GETPC".into());
        }
        let jump = &mut insns[i + 2];
        jump.enc = Enc::Sopp;
        jump.op = 2;
        jump.aux.imm = (target - jump.offset as i64 - 4) / 4;
        if jump.mnemonic == "s_setpc_b64" {
            jump.dst.clear();
        }
    }
    Ok(())
}

// These instruction sequences elect the lowest active lane to reserve a contiguous
// range, restore EXEC, broadcast the atomic result and add a per-lane prefix.
// The two register allocations differ. Match the complete regions before changing
// a saved EXEC mask from its usual per-invocation predicate into wave-uniform data.
fn mark_wave_allocations(insns: &mut [Insn], wave32_mask: bool, election: Option<ElectionLowering>) -> Result<bool, String> {
    const REGIONS: [&[u32]; 2] = [
        &[
        0xd7650007, 0x000100c1, 0xbe8e147e, 0xbe8c047e, 0xd7660007, 0x00020ec1,
        0xd482000e, 0x00020e0e, 0x7e0e0280, 0xbe8e240e, 0xbf880005, 0xbf800000,
        0xbe8c100c, 0x7e0e020c, 0xe0c84000, 0x80070700, 0xbefe040e, 0xd7650008,
        0x0001007e, 0xbf8c3f70, 0x7e180507, 0xd7660007, 0x0002107f, 0x4a0e0e0c
        ],
        &[
        0xd7650007, 0x000100c1, 0xbe83147e, 0xbe8e047e, 0xd7660007, 0x00020ec1,
        0xd4820012, 0x00020e03, 0x7e0e0280, 0xbe922412, 0xbf880005, 0xbf800000,
        0xbe83100e, 0x7e0e0203, 0xe0c84000, 0x80070700, 0xbefe0412, 0xd7650008,
        0x0001007e, 0xbf8c3f70, 0x7e060507, 0xd7660007, 0x0002107f, 0x4a0e0e03
        ],
    ];
    const COUNT: usize = 18;
    let snapshots = election.is_some() && predicate_snapshots(insns);
    let mut found = false;
    for i in 0..insns.len() {
        // Wave32 programs keep the previous lowering unless the election verifies.
        // Modes that model EXEC as a whole-wave mask already lower it directly.
        if !wave32_mask && insns[i].mnemonic == "s_ff1_i32_b32" && insns[i].src == [Opnd::ExecLo] {
            let mut trial = insns.to_vec();
            match verify_lane_election(&mut trial, i, election.is_some()) {
                Ok(()) => {
                    trial[i].mnemonic = "tb_first_active_lane".into();
                    insns.clone_from_slice(&trial);
                }
                Err(e) if std::env::var_os("TB_DFDEBUG").is_some() => eprintln!("{e}"),
                Err(_) => {}
            }
            continue;
        }
        if insns[i].mnemonic != "s_ff1_i32_b64" || insns[i].src != [Opnd::ExecLo] { continue; }
        let region = i.checked_sub(1).and_then(|first| insns.get(first..first + COUNT))
            .filter(|region| REGIONS.iter().any(|words| region.iter().flat_map(|op| op.raw.iter()).eq(words.iter())));
        let Some(region) = region else {
            if !snapshots {
                verify_lane_election(insns, i, election.is_some())?;
            }
            if election == Some(ElectionLowering::Shared64) { insns[i].mnemonic = "tb_first_exec_lane".into(); }
            continue;
        };
        let first = region[0].offset;
        let end = region[COUNT - 1].offset + 4;
        for branch in insns.iter().filter(|b| b.enc == Enc::Sopp && Emitter::branch_cond(b.op).is_some()) {
            let target = branch.offset as i64 + 4 + 4 * branch.aux.imm;
            if !(first..end).contains(&branch.offset) && target > first as i64 && target < end as i64 {
                return Err("external branch enters wave allocation".into());
            }
        }
        insns[i + 1].mnemonic = "tb_capture_allocation_mask".into();
        insns[i + 12].mnemonic = "tb_allocation_prefix_lo".into();
        insns[i + 15].mnemonic = "tb_allocation_prefix_hi".into();
        found = true;
    }
    Ok(found)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ElectionLowering {
    Native32,
    Shared64,
}

/// Select the lane geometry only for ordinary straight-line compute kernels.
/// A group with at most 32 threads fits in one native wave even for a b64 search.
/// Larger wave64 elections reuse the workgroup exchange across both halves.
fn straight_election(ex: &ShaderExtract, insns: &[Insn]) -> Option<ElectionLowering> {
    if ex.stage != Stage::Compute || ex.fspx_config.is_some() { return None; }
    let threads = compute_group(ex).into_iter().fold(1u32, u32::saturating_mul);
    if threads == 0 || threads > 1024 { return None; }
    let first = insns.iter().find(|i| matches!(i.mnemonic.as_str(), "s_ff1_i32_b32" | "s_ff1_i32_b64"));
    // A small group fits one wave of either guest width, even without FF1.
    if first.is_none() && (threads > 32 || !insns.iter().any(exec_snapshot::captures_exec)) { return None; }
    let (end, body) = insns.split_last()?;
    if end.mnemonic != "s_endpgm" || !body.iter().all(|i| match i.mnemonic.as_str() {
        "s_ff1_i32_b32" | "s_ff1_i32_b64" => first.is_some_and(|f| f.mnemonic == i.mnemonic) && i.src == [Opnd::ExecLo],
        "s_load_dwordx4" | "s_waitcnt" | "s_nop" | "s_mov_b32" | "s_mov_b64"
        | "s_and_saveexec_b32" | "s_and_saveexec_b64" | "v_mbcnt_lo_u32_b32" | "v_mbcnt_hi_u32_b32"
        | "v_mov_b32" | "v_and_b32" | "v_lshlrev_b32" | "v_lshrrev_b32"
        | "v_add_nc_u32" | "v_cndmask_b32" | "buffer_store_dword" => true,
        m => m.starts_with("v_cmp_") || m.starts_with("v_cmpx_"),
    }) { return None; }
    Some(if threads <= 32 || first.is_some_and(|f| f.mnemonic == "s_ff1_i32_b32") {
        ElectionLowering::Native32
    } else {
        ElectionLowering::Shared64
    })
}

/// In a straight-line compute shader, a compare/save/FF1/restore sequence
/// produces a uniform scalar value. Recognize every mask access before allowing
/// uses other than the usual comparison against the lane index.
fn predicate_snapshots(mut code: &[Insn]) -> bool {
    let mask = |o: &Opnd| matches!(o, Opnd::ExecLo | Opnd::ExecHi | Opnd::VccLo | Opnd::VccHi);
    // Numeric vector operands can read the ballot captured at SAVEEXEC. Other
    // consumers still require proof about the representation they expect.
    let mut saved_words = BTreeMap::new();
    let count = code.len();
    while let [op, rest @ ..] = code {
        if exec_snapshot::copy_scalar_words(op, &mut saved_words) {
            code = rest;
            continue;
        }
        let numeric = exec_snapshot::numeric_sources(op) != 0;
        if let ("v_cndmask_b32", Some(Opnd::Sgpr(lo))) = (op.mnemonic.as_str(), op.src.get(2)) {
            let pair = [saved_words.get(lo), saved_words.get(&(lo + 1))];
            if pair.iter().any(|w| matches!(w, Some(exec_snapshot::Word::Saved(..))))
                && (op.enc != Enc::Vop3 || op.aux.sdwa.is_some() || pair.contains(&None))
            {
                return false;
            }
        }
        if !numeric && saved_words.iter().any(|(&reg, word)| matches!(word, exec_snapshot::Word::Saved(..))
            && (reads_sgpr(op, reg) || op.mnemonic.ends_with("64") && op.src.iter().any(|o| may_touch_sgpr(o, reg))))
        {
            return false;
        }
        let i = count - code.len();
        if op.mnemonic.starts_with("v_cmp_") && op.dst == [Opnd::VccLo] {
            let [save, search, restore, tail @ ..] = rest else { return false };
            let [Opnd::Sgpr(saved)] = save.dst.as_slice() else { return false };
            let [Opnd::Sgpr(result)] = search.dst.as_slice() else { return false };
            if op.src.iter().any(mask) || save.mnemonic != "s_and_saveexec_b64" || save.src != [Opnd::VccLo]
                || search.mnemonic != "s_ff1_i32_b64" || search.src != [Opnd::ExecLo]
                || result == saved || u16::from(*result) == u16::from(*saved) + 1
                || restore.mnemonic != "s_mov_b64" || restore.dst != [Opnd::ExecLo]
                || restore.src != [Opnd::Sgpr(*saved)]
            {
                return false;
            }
            exec_snapshot::record_scalar_write(save, i + 1, &mut saved_words);
            exec_snapshot::record_scalar_write(search, i + 2, &mut saved_words);
            code = tail;
        } else {
            if op.src.iter().chain(&op.dst).any(mask) || op.mnemonic.starts_with("v_cmpx_") {
                return false;
            }
            exec_snapshot::record_scalar_write(op, i, &mut saved_words);
            code = rest;
        }
    }
    true
}

/// True when `o` may name SGPR `n` as part of a 32- or 64-bit operand.
fn may_touch_sgpr(o: &Opnd, n: u8) -> bool {
    matches!(*o, Opnd::Sgpr(k) if u16::from(k) + 1 >= u16::from(n) && u16::from(k) <= u16::from(n) + 1)
}

/// Whether `ins` may read SGPR `n`, by operand width: scalar 64-bit operations,
/// vector lane-mask sources, and descriptor or address bases of memory operations.
fn reads_sgpr(ins: &Insn, n: u8) -> bool {
    let m = ins.mnemonic.as_str();
    // GETPC has no source. Its unused SOP1 source field decodes as s0.
    if m == "s_getpc_b64" { return false; }
    // Scalar loads encode their offset register separately from the base operands.
    if ins.enc == Enc::Smem && n <= 105 && ins.raw.get(1).is_some_and(|w| w >> 25 == u32::from(n)) {
        return true;
    }
    let memory = if m.starts_with("image_") { 8 }
        else if m.starts_with("s_buffer") || m.starts_with("buffer_") || m.starts_with("tbuffer_") { 4 }
        else if m.starts_with("s_load") || m.starts_with("global_") { 2 } else { 0 };
    ins.src.iter().enumerate().any(|(i, o)| {
        // VOP3 decodes three source fields; LDEXP uses only the value and exponent.
        if m == "v_ldexp_f32" && i == 2 { return false; }
        let Opnd::Sgpr(k) = *o else { return false };
        let width: u16 = if memory != 0 { memory } else if m.starts_with("s_") && (m.ends_with("64") || m.contains("saveexec_b64")) { 2 }
            else if m.starts_with("v_") && i >= 2 { 2 } else { 1 };
        u16::from(k) <= u16::from(n) && u16::from(n) < u16::from(k) + width
    })
}

fn branch_pc(ins: &Insn) -> i64 {
    ins.offset as i64 + 4 + ins.aux.imm * 4
}

fn branch_targets(insns: &[Insn]) -> BTreeSet<i64> {
    insns.iter()
        .filter(|b| b.enc == Enc::Sopp && Emitter::branch_cond(b.op).is_some())
        .map(|b| b.offset as i64 + 4 + 4 * b.aux.imm)
        .collect()
}

/// The last instruction before `at` that writes VGPR `v`, if no branch enters between.
fn vgpr_def(insns: &[Insn], targets: &BTreeSet<i64>, at: usize, v: u8) -> Option<usize> {
    for j in (0..at).rev() {
        if targets.contains(&(insns[j + 1].offset as i64)) { return None; }
        if insns[j].dst.contains(&Opnd::Vgpr(v)) { return Some(j); }
    }
    None
}

/// Verify a first-active-lane election that the generic lowering already expresses:
/// `s_ff1_i32_b64 s, exec` (or `_b32`) compared with the full-mask `v_mbcnt` lane index. The
/// elected lane usually adds the number of active lanes to a counter; that count
/// reads an EXEC copy as a wave mask, so capture the copy as a ballot.
fn verify_lane_election(insns: &mut [Insn], ff1: usize, straight_compute: bool) -> Result<(), String> {
    let at = insns[ff1].offset;
    let bad = |what: &str| format!("unverified EXEC election at {at:#x}: {what}");
    let Some(&Opnd::Sgpr(lane)) = insns[ff1].dst.first() else { return Err(bad("no scalar destination")) };
    let targets = branch_targets(insns);
    // Two-source VOP3 forms decode an unused third source; ignore it.
    let sources = |ins: &Insn| {
        let two = ins.mnemonic.starts_with("v_cmp_") || ins.mnemonic.starts_with("v_mbcnt_");
        ins.src.iter().take(if two { 2 } else { usize::MAX }).cloned().collect::<Vec<_>>()
    };
    let cmp = (ff1 + 1..insns.len())
        .find(|&j| {
            let ins = &insns[j];
            if straight_compute && ins.mnemonic == "s_mov_b32" {
                return ins.src.iter().chain(&ins.dst).any(|o| *o == Opnd::Sgpr(lane));
            }
            sources(ins).iter().chain(&ins.dst).any(|o| may_touch_sgpr(o, lane))
        })
        .ok_or_else(|| bad("unused lane"))?;
    if (ff1 + 1..=cmp).any(|j| targets.contains(&(insns[j].offset as i64))) {
        return Err(bad("branch enters the election"));
    }
    let c = &insns[cmp];
    // The compare either fills a mask for a later saveexec or narrows EXEC itself.
    let narrows = matches!(c.mnemonic.as_str(), "v_cmpx_eq_i32" | "v_cmpx_eq_u32");
    if !(narrows || matches!(c.mnemonic.as_str(), "v_cmp_eq_i32" | "v_cmp_eq_u32")) {
        return Err(bad("lane is not compared for equality"));
    }
    // Equality is symmetric. The VOP3 encoding permits the elected scalar
    // on either side; preserve all existing full-lane-index proof checks.
    let index = match c.src.get(..2) {
        Some([Opnd::Sgpr(reg), Opnd::Vgpr(v)]) | Some([Opnd::Vgpr(v), Opnd::Sgpr(reg)]) if *reg == lane => *v,
        _ => return Err(bad("lane is not compared for equality with a VGPR")),
    };
    let bits = if insns[ff1].mnemonic.ends_with("_b32") { "b32" } else { "b64" };
    let full_low = |l: &Insn| l.mnemonic == "v_mbcnt_lo_u32_b32" && l.aux.mask == 0
        && l.src.get(..2) == Some(&[Opnd::ConstInt(-1), Opnd::ConstInt(0)][..]);
    let last = vgpr_def(insns, &targets, cmp, index).ok_or_else(|| bad("no lane index"))?;
    let h = &insns[last];
    // Wave32 counts the low lanes only; wave64 adds the high half.
    let lane_index = if bits == "b32" { full_low(h) } else {
        let Some(&Opnd::Vgpr(low)) = h.src.get(1) else { return Err(bad("lane index is not v_mbcnt")) };
        let lo = vgpr_def(insns, &targets, last, low).ok_or_else(|| bad("no low lane count"))?;
        h.mnemonic == "v_mbcnt_hi_u32_b32" && h.src[0] == Opnd::ConstInt(-1) && h.aux.mask == 0 && full_low(&insns[lo])
    };
    if !lane_index {
        return Err(bad("lane index is not a full-mask v_mbcnt"));
    }
    // Inside the elected branch, an active-lane count reads an EXEC copy made earlier.
    let (save, restore) = if narrows {
        // EXEC was copied before the compare. The branch lasts until any copy is
        // restored, or to the end of a program that does not need the mask again.
        let restore = (cmp + 1..insns.len())
            .find(|&j| insns[j].dst.first() == Some(&Opnd::ExecLo) && insns[j].mnemonic.starts_with("s_"))
            .unwrap_or(insns.len());
        (cmp, restore)
    } else {
        let Some(save) = (cmp + 1..insns.len()).find(|&j| insns[j].mnemonic == format!("s_and_saveexec_{bits}")) else { return Ok(()) };
        // A different comparison replaced VCC before SAVEEXEC. In straight-line
        // code that save starts an independent masked region.
        if straight_compute && c.dst == [Opnd::VccLo] && insns[save].src == [Opnd::VccLo]
            && insns[cmp + 1..save].iter().any(|i| i.mnemonic.starts_with("v_cmp_") && i.dst == [Opnd::VccLo])
        {
            return Ok(());
        }
        let Some(&Opnd::Sgpr(saved)) = insns[save].dst.first() else { return Ok(()) };
        let restore = (save + 1..insns.len())
            .find(|&j| insns[j].mnemonic == format!("s_mov_{bits}") && insns[j].dst.first() == Some(&Opnd::ExecLo)
                && insns[j].src.first() == Some(&Opnd::Sgpr(saved)))
            .ok_or_else(|| bad("elected branch does not restore EXEC"))?;
        (save, restore)
    };
    for count in save + 1..restore {
        if insns[count].mnemonic != format!("s_bcnt1_i32_{bits}") { continue; }
        if narrows && bits == "b64" && counted_lanes(insns, &targets, cmp, count).is_some() && insns[count].src.first().is_some() {
            insns[count].mnemonic = COUNT_LANES.into();
            continue;
        }
        let Some(&Opnd::Sgpr(mask)) = insns[count].src.first() else { return Err(bad("count of a non-SGPR mask")) };
        let copy = (0..count).rev()
            .find(|&j| insns[j].dst.iter().any(|o| may_touch_sgpr(o, mask)))
            .ok_or_else(|| bad("counted mask has no definition"))?;
        if insns[copy].mnemonic != format!("s_mov_{bits}") || insns[copy].dst != [Opnd::Sgpr(mask)] || insns[copy].src != [Opnd::ExecLo] {
            return Err(bad("counted mask is not an EXEC copy"));
        }
        let entered = (copy + 1..=count).any(|j| targets.contains(&(insns[j].offset as i64)));
        let read = (copy + 1..count).any(|j| sources(&insns[j]).iter().any(|o| may_touch_sgpr(o, mask)));
        // Each half not overwritten by the count must die unread.
        let later_read = |half: u8| (count + 1..insns.len())
            .map(|j| &insns[j])
            .take_while(|ins| !ins.dst.iter().any(|o| *o == Opnd::Sgpr(half)
                || *o == Opnd::Sgpr(half.wrapping_sub(1)) && (ins.mnemonic.ends_with("_b64") || ins.mnemonic.starts_with("v_cmp"))
                || *o == Opnd::Sgpr(half) && ins.mnemonic.starts_with("s_")))
            .any(|ins| reads_sgpr(ins, half));
        let counted_into = |half: u8| insns[count].dst == [Opnd::Sgpr(half)];
        let halves: &[u8] = if bits == "b32" { &[mask] } else { &[mask, mask + 1] };
        let live = halves.iter().any(|&half| !counted_into(half) && later_read(half));
        if entered || read || live {
            return Err(bad(&format!("EXEC copy has other uses (entered {entered}, read {read}, live {live})")));
        }
        insns[copy].mnemonic = if bits == "b32" { "tb_capture_allocation_mask32" } else { "tb_capture_allocation_mask" }.into();
    }
    Ok(())
}

/// The population count of a lane predicate, taken over the wave where it was computed.
const COUNT_LANES: &str = "tb_count_lanes";

/// Where the lane predicate counted at `count` inside an elected branch is computed:
/// before the election at `cmp`, with no branch between, and from active lanes only.
fn counted_lanes(insns: &[Insn], targets: &BTreeSet<i64>, cmp: usize, count: usize) -> Option<usize> {
    let mask = *insns[count].src.first()?;
    let names = |o: &Opnd| match (*o, mask) {
        (Opnd::Sgpr(a), Opnd::Sgpr(b)) => a == b || a + 1 == b || a == b + 1,
        (a, b) => a == b || (b == Opnd::VccLo && a == Opnd::VccHi),
    };
    let def = (0..count).rev().find(|&j| insns[j].dst.iter().any(names))?;
    let d = &insns[def];
    if def >= cmp || d.dst.first() != Some(&mask) { return None; }
    // `v_cmpx` names VCC as a destination it does not write.
    if d.mnemonic.starts_with("v_cmpx") { return None; }
    let active_only = d.mnemonic == "s_mov_b64" && d.src == [Opnd::ExecLo]
        || d.mnemonic == "s_and_b64" && d.src.contains(&Opnd::ExecLo)
        || d.mnemonic.starts_with("v_cmp_");
    let straight = !(def + 1..=cmp).any(|j| targets.contains(&(insns[j].offset as i64))
        || (insns[j].enc == Enc::Sopp && Emitter::branch_cond(insns[j].op).is_some())
        || (j < cmp && insns[j].dst.first() == Some(&Opnd::ExecLo)));
    (active_only && straight).then_some(def)
}

fn compute_group(ex: &ShaderExtract) -> [u32; 3] {
    if let Some(threads) = ex.psv_threads { return threads; }
    let [_, x, y, z] = ex.isa_preamble;
    [x.max(1), y.max(1), z.max(1)]
}

pub fn translate(ex: &ShaderExtract) -> Result<Translated, String> {
    translate_linked(ex, None)
}

/// [`translate`] for the last stage before the rasteriser (VS or GS), given the
/// [`Translated::ps_input_interp`] of the pixel shader it feeds: the outputs that pixel
/// shader reads get the same interpolation modifiers, so both stages pack their signatures
/// into the same registers. Ignored for other stages.
pub fn translate_linked(ex: &ShaderExtract, feeds_ps: Option<&InputInterp>) -> Result<Translated, String> {
    translate_ordered(ex, feeds_ps, &[])
}

/// Preserve every output and its assignment. Declare shared fields in the pixel
/// signature's order so producer-only fields cannot displace rasterizer inputs.
pub fn translate_ordered(
    ex: &ShaderExtract, feeds_ps: Option<&InputInterp>, output_order: &[String],
) -> Result<Translated, String> {
    translate_outputs(ex, feeds_ps, output_order, None)
}

/// Redirect the pixel stencil reference to an unused integer render target.
/// The host binds a mirror of the original stencil plane at this index.
pub fn translate_stencil_target(ex: &ShaderExtract, slot: u32) -> Result<Translated, String> {
    if ex.stage != Stage::Pixel || slot >= 8
        || ex.output_sig.iter().filter(|e| e.sysval == 69 && e.comp_type == 1 && e.mask == 1).count() != 1
        || ex.output_sig.iter().any(|e| e.sysval == 64 && e.index >= slot)
    {
        return Err("invalid stencil render target".into());
    }
    translate_outputs(ex, None, &[], Some(slot))
}

fn translate_outputs(ex: &ShaderExtract, feeds_ps: Option<&InputInterp>, output_order: &[String],
    stencil_target: Option<u32>) -> Result<Translated, String> {
    let first = translate_outputs_once(ex, feeds_ps, output_order, stencil_target);
    if first.is_ok() || ex.stage != Stage::Compute || stencil_target.is_some() { return first; }
    // Once this exact exchange subset is recognized, a later binding or
    // emitter error must not silently replace its collectives with copies.
    let mut original = disassemble(&ex.isa);
    if original.iter().any(is_binary_quad_perm) || group_extrema::prepare_exchanges(ex, &mut original).unwrap_or(true) {
        // This standalone exchange observes its source lane even if the exact
        // translation failed elsewhere. A per-lane retry would drop that operand.
        return first;
    }
    if (ex.root_sig.is_some() || original.iter().all(|i| !matches!(i.enc, Enc::Smem | Enc::Mubuf)))
        && wave_sum::straight_exchange(&mut original, compute_group(ex)) {
        return first;
    }
    // Appends only: each lane may allocate for itself.
    let alone = lone_lane::retry(|| translate_outputs_once(ex, feeds_ps, output_order, stencil_target))
        .and_then(|t| lone_lane::check(&t.hlsl).map(|()| t));
    match alone {
        Ok(translated) => Ok(translated),
        Err(e) => {
            if std::env::var_os("TB_DFDEBUG").is_some() { eprintln!("{e}"); }
            first
        }
    }
}

fn translate_outputs_once(ex: &ShaderExtract, feeds_ps: Option<&InputInterp>, output_order: &[String],
    stencil_target: Option<u32>) -> Result<Translated, String> {
    if stencil_target.is_none() {
        if let Some(hlsl) = crate::independent_hlsl(ex) {
            let compute = ex.stage == Stage::Compute;
            return Ok(Translated {
                hlsl, entry: "main", profile: if crate::requires_inline_ray_query(ex) { "cs_6_5" } else if compute { "cs_5_0" } else { "ps_5_0" },
                needs_barycentrics: false, typed_int64_atomics: false, packed_dot_product: false, required_wave_size: None,
                portable_compute_group: if compute { ex.psv_threads } else { None },
                ps_input_interp: InputInterp::default(),
            });
        }
    }
    if ex.stage == Stage::Mesh {
        return mesh::translate(ex, feeds_ps, output_order);
    }
    if persistent::applies(ex) {
        return persistent::translate(ex);
    }
    let (profile, is_ps) = match ex.stage {
        Stage::Vertex => ("vs_5_0", false),
        Stage::Pixel => ("ps_5_0", true),
        Stage::Compute => ("cs_5_0", false),
        Stage::Geometry => ("gs_5_0", false),
        other => return Err(format!("stage {other:?} not supported yet")),
    };
    let mut insns = disassemble(&ex.isa);
    if lone_lane::on() { lone_lane::prepare(&mut insns)?; }
    let resolve_wave32 = !lone_lane::on() && ray_resolve::wave32(ex, &insns)?;
    let probe = radiance_probe::identify(ex);
    let probe_trace = probe.is_some();
    if let Some(probe) = probe { radiance_probe::prepare(probe, &mut insns); }
    // Slot 80 explicitly selects a wave width. Only opt in straight-line
    // loads, stores and lane-local arithmetic with ordinary operands.
    let portable_compute_group = (ex.stage == Stage::Compute && insns.iter().all(|ins| {
        if ins.mnemonic == "v_nop" { return true; } // driver hash marker
        matches!(ins.mnemonic.as_str(), "s_endpgm" | "s_waitcnt" | "s_load_dwordx4"
            | "buffer_load_dword" | "buffer_store_dword" | "v_mov_b32" | "v_add_nc_u32"
            | "v_lshlrev_b32" | "v_sub_nc_u32" | "v_mul_f32" | "v_add_f32" | "v_and_b32"
            | "v_or_b32" | "v_xor_b32")
            && ins.src.iter().chain(&ins.dst).all(|op| matches!(op, Opnd::Sgpr(_) | Opnd::Vgpr(_)
                | Opnd::ConstInt(_) | Opnd::ConstF32(_) | Opnd::Literal(_)))
    })).then(|| compute_group(ex));
    lower_local_pc_transfers(&mut insns)?;
    let entry_fallback = ex.stage == Stage::Compute && indexed_lane::skip_empty_wave_fallback(&mut insns);
    let single_wave32 = (entry_fallback || probe_trace) && compute_group(ex).into_iter().fold(1u32, u32::saturating_mul) == 32
        && insns.iter().any(|op| op.mnemonic == "s_and_saveexec_b32")
        && insns.iter().any(|op| op.mnemonic.starts_with("v_cmp_") && matches!(op.dst[..], [Opnd::Sgpr(n)] if n & 1 != 0));
    let particle_sort = particle_sort::prepare(ex, &mut insns);
    let hair_cluster_export = hair_cluster_export::prepare(ex, &mut insns);
    let hair_cluster = hair_cluster::prepare(ex, &mut insns);
    let instance_cull = instance_cull::prepare(ex, &mut insns);
    let hzb_cull = instance_cull_hzb::prepare(ex, &mut insns);
    let view_cull = instance_cull_views::prepare(ex, &mut insns);
    let post_cull = instance_cull_post::prepare(ex, &mut insns);
    if !post_cull && !instance_cull && !hzb_cull && view_cull.is_none() && ex.stage == Stage::Compute
        && ex.entry.as_deref() == Some("InstanceCull") && compute_group(ex) == [64, 1, 1]
    {
        return persistent::translate(ex);
    }
    let hair_visibility_indirect = hair_visibility_indirect::prepare(ex, &mut insns);
    let hair_visibility_single = hair_visibility_single::prepare(ex, &mut insns);
    let hair_compact = hair_compact::prepare(ex, &mut insns);
    let subsurface_setup = subsurface_setup::prepare(ex, &mut insns);
    let material_classify = ps09_classify::prepare(ex, &mut insns);
    let classify_tiles = classify_materials::prepare(ex, &mut insns);
    singleton_wave::prepare(ex, &mut insns);
    let hair_multisample = hair_multisample::recognize(ex);
    let fspx_wave = fspx_wave::width(ex, &insns);
    let wave_cull = fspx_wave.is_some() || hair_multisample || wave_cull::recognize(ex);
    let page_waves = page_waves::recognize(ex);
    let generic_waves = !lone_lane::on() && !wave_cull && !page_waves;
    let election = if generic_waves && !resolve_wave32 { straight_election(ex, &insns) } else { None };
    let allocation_wave64 = generic_waves && mark_wave_allocations(&mut insns, resolve_wave32, election)?;
    if ex.stage == Stage::Pixel { wave_alloc::prepare(&mut insns); }
    let reflection = reflection_compact::prepare(ex, &mut insns);
    group_extrema::prepare_exchanges(ex, &mut insns)?;
    let group_extrema = group_extrema::prepare(ex, &mut insns);
    let straight_exchange = ex.stage == Stage::Compute && !lone_lane::on()
        && !wave_cull && !page_waves && !resolve_wave32 && !allocation_wave64
        && (ex.root_sig.is_some() || insns.iter().all(|i| !matches!(i.enc, Enc::Smem | Enc::Mubuf)))
        && wave_sum::straight_exchange(&mut insns, compute_group(ex));
    let required_wave_size = if wave_cull {
        fspx_wave.filter(|&width| width == 32)
    } else if straight_exchange { None }
      else { lower_wave_reductions(&mut insns)? };
    let required_wave_size = if resolve_wave32 {
        if matches!(required_wave_size, Some(64)) || insns.iter().any(is_quad_exchange) {
            return Err("resolve Wave32 requires a compatible wave reduction".into());
        }
        Some(32)
    } else { required_wave_size };
    let uses_bpermute = insns.iter().any(|ins| ins.mnemonic == "ds_bpermute_b32");
    if uses_bpermute && insns.iter().any(|ins| ins.enc == Enc::Sopp && Emitter::branch_cond(ins.op).is_some()) {
        return Err("backward permute with scalar branches needs verified convergent control flow".into());
    }
    let uses_quad_add = insns.iter().any(is_quad_exchange);
    let scan = if wave_cull { None } else { wave_scan::prepare(ex, &mut insns)? };
    let scan_width = scan.as_ref().map(|s| s.width);
    let ordinary_wave = !wave_cull && !page_waves && !lone_lane::on();
    let indexed_lane = !resolve_wave32 && (election == Some(ElectionLowering::Shared64) || group_extrema || ex.stage == Stage::Compute
        && (indexed_lane::constant_reads(compute_group(ex), &mut insns, ordinary_wave)
            || indexed_lane::prepare(compute_group(ex), &mut insns)));
    // A uniform Sgpr lane operand is only expressible through the whole-wave
    // group read, which needs shared-memory exchange like a swizzle does.
    let has_dynamic_lane_read = insns.iter().any(|i| i.enc == Enc::Vop3
        && i.op == 0x360
        && matches!(i.src.get(1), Some(Opnd::Sgpr(_))));
    if !wave_cull && !page_waves { {
        // Swizzles already exchange values through group memory; so can a lane read.
        let [x, y, z] = compute_group(ex);
        // Swizzle shaders already handle EXEC; a fully uniform shader (no EXEC
        // instructions) is the simplest case for the group read. Both cases share
        // the swizzle validation below.
        let swizzle_group_reads = insns.iter().any(|i| i.mnemonic == "ds_swizzle_b32")
            && insns.iter().any(|i| i.mnemonic.ends_with("exec_b64") || i.mnemonic == "s_mov_b64");
        let group_reads = ex.stage == Stage::Compute && !resolve_wave32 && !lone_lane::on() && x * y * z % 32 == 0
            && (swizzle_group_reads || has_dynamic_lane_read)
            && !insns.iter().any(|i| i.enc == Enc::Ds && i.mnemonic != "ds_swizzle_b32" && i.mnemonic.starts_with("ds_ordered"));
        if ex.stage == Stage::Pixel && pixel_heap_base(ex, 5).is_some() { lower_first_lane_reads(&mut insns); }
        lower_lane_spills(&mut insns, resolve_wave32, group_reads)?;
    } }
    let mut em = Emitter::new(ex, insns);
    if let Some(mode) = election {
        if compute_group(ex).into_iter().product::<u32>() <= 32 || em.insns.iter().any(|i| matches!(i.mnemonic.as_str(), "tb_first_exec_lane" | "s_ff1_i32_b64") && i.src == [Opnd::ExecLo]) {
            em.exec_snapshots = exec_snapshot::analyze(&em.insns, mode);
        }
    }
    let shared_mask = em.exec_snapshots.sites.values().any(|site| matches!(site, exec_snapshot::Site::Capture(ElectionLowering::Shared64)));
    em.page_waves = page_waves;
    em.wave_cull = wave_cull;
    em.fspx_wave = fspx_wave;
    em.hair_multisample = hair_multisample;
    em.hoist_wave_exchanges = wave_cull && (compute_group(ex) == [16, 16, 1]
        || ex.entry.as_deref() == Some("GenerateReflectionRaysCS")
        || (ex.entry.as_deref() == Some("MainCS") && [
            (2356, 0xe4995b0e2291e61e), (2884, 0x1c9821104747da89),
        ].into_iter().any(|(size, hash)| packed_root::matches_program(&ex.isa, size, hash))));
    em.indexed_lane = indexed_lane;
    em.single_wave32 = single_wave32;
    em.resolve_wave32 = resolve_wave32;
    em.wave32_compare_masks |= reflection && matches!(ex.isa.len(), 1024 | 1028);
    if matches!(ex.stage, Stage::Vertex | Stage::Geometry) {
        em.out_interp = feeds_ps.cloned().unwrap_or_default();
    }
    em.vreg_array = em.insns.iter().any(|i| i.mnemonic.starts_with("v_movrel"));
    for count in 0..em.insns.len() {
        if em.insns[count].mnemonic != COUNT_LANES { continue; }
        let mask = em.insns[count].src[0];
        let def = (0..count).rev().find(|&j| em.insns[j].dst.first() == Some(&mask) && !em.insns[j].mnemonic.starts_with("v_cmpx"))
            .ok_or("lane count without its predicate")?;
        em.vote_defs.insert(def);
        em.vote_uses.insert(count, def);
        em.regs.insert(format!("tb_votes{def}"));
    }
    // Lane-spill slots beyond the tracked range are function-scope temporaries.
    for ins in &em.insns {
        for o in ins.dst.iter().chain(ins.src.iter()) {
            if let Opnd::Temp(n) = o {
                if *n >= LANE_TEMP_BASE {
                    em.regs.insert(format!("tb_s{n}"));
                }
            }
        }
    }
    if !em.resolve_packed(scan.as_ref())? {
        if scan_width.is_some() { return Err("wave scan needs a verified packed root layout".into()); }
        em.resolve_resources()?;
    }
    if allocation_wave64 {
        // Both following stores write list data to the same typed view: the light
        // index and the previous uint head returned by the indexed atomic exchange.
        // Validate the producer and addresses instead of inferring the element
        // type from the PSV kind alone.
        let mut list_slots = BTreeSet::new();
        for (i, _) in em.insns.iter().enumerate().filter(|(_, i)| i.mnemonic == "tb_allocation_prefix_hi") {
            let tail = em.insns.get(i + 9..i + 14).ok_or("missing allocated list writes")?;
            if tail[0].raw != [0xe0c06000, 0x80080801] || tail[1].raw != [0x38140e81]
                || tail[2].raw != [0xe0102000, 0x80060907] || tail[3].raw != [0xbf8c3f70]
                || tail[4].raw != [0xe0102000, 0x8006080a] {
                return Err("unverified list stores after wave allocation".into());
            }
            let Some([Res::TBuf(slot, true)]) = em.res.get(&(i + 11)).map(Vec::as_slice) else {
                return Err("missing list UAV for wave allocation".into());
            };
            if em.res.get(&(i + 13)) != em.res.get(&(i + 11)) || !em.ex.resources.iter().any(|r|
                r.space == 0 && r.res_type == 6 && r.lower <= *slot && *slot <= r.upper) {
                return Err("conflicting list UAV bindings".into());
            }
            list_slots.insert(*slot);
        }
        em.tbuf_uint.extend(list_slots);
    }
    let light_grid = allocation_wave64 && light_grid::prepare(&mut em)?;
    let requires_ballot = (allocation_wave64 && !light_grid) || em.packed.as_ref().is_some_and(|p| !p.ballots.is_empty());
    if (requires_ballot && scan_width.or(required_wave_size) == Some(32))
        || matches!((scan_width, required_wave_size), (Some(a), Some(b)) if a != b) {
        return Err("conflicting wave widths for ballot and reduction".into());
    }
    let register_wave_size = em.packed.as_ref().and_then(|plan| plan.register_wave_size);
    // A pixel is a wave of its own here (see `v_readfirstlane_b32`), so a reduction
    // yields that pixel's value.
    em.pixel_wave = ex.stage == Stage::Pixel && em.pixel_heap && scan_width.is_none() && register_wave_size.is_none()
        && em.insns.iter().all(|i| i.op != wave_sum::DPP_REDUCE || i.enc != Enc::Vop2
            || matches!(i.mnemonic.as_str(), "v_and_b32" | "v_or_b32" | "v_min_u32" | "v_max_u32" | "v_min_f32" | "v_max_f32" | "v_add_nc_u32" | "v_add_f32"));
    let required_wave_size = if em.pixel_wave { None } else { required_wave_size };
    let required_wave_size = required_wave_size.or(scan_width).or(register_wave_size).or((requires_ballot || !em.vote_uses.is_empty()).then_some(64));
    // With more than 32 threads, rows 2 and 3 could instead be rows 0 and 1 of
    // a second wave. The fallback host width cannot resolve an asymmetric guest mask.
    if uses_quad_add && required_wave_size.is_none() && !em.one_word_vector_mask()
        && compute_group(ex).into_iter().fold(1u32, u32::saturating_mul) > 32
        && em.insns.iter().filter(|i| is_quad_exchange(i) && i.enc == Enc::Vop1).any(|i| {
            let rows = i.raw[1] >> 28;
            i.raw[1] & 0x0f00_0000 != 0 && rows & 3 != rows >> 2
        })
    {
        return Err("asymmetric DPP16 row mask requires a known guest wave width".into());
    }
    let required_wave_size = required_wave_size.or((uses_quad_add && (!em.pixel_wave || em.insns.iter().any(is_row_move)) || uses_bpermute || group_extrema).then_some(32));
    // A ballot or minimum over active invocations also works in the final
    // partial subgroup. Keep the full-wave requirement for every other user.
    let partial_election = required_wave_size.is_none() && election == Some(ElectionLowering::Native32);
    let required_wave_size = required_wave_size.or_else(|| {
        (election == Some(ElectionLowering::Native32) && (!em.exec_snapshots.sites.is_empty() || em.insns.iter().any(|i|
            i.mnemonic == "tb_first_active_lane" || i.mnemonic == "s_ff1_i32_b64"))).then_some(32)
    });
    em.required_wave_size = required_wave_size;
    // DS bit-mask mode operates on the low five lane bits in wave32 and wave64.
    // Map guest lanes by workgroup index, independently of the Vulkan subgroup layout.
    em.compute_swizzle = ex.stage == Stage::Compute && scan_width.is_none() && !resolve_wave32
        && (em.insns.iter().any(|i| i.mnemonic == "ds_swizzle_b32") || has_dynamic_lane_read);
    if em.compute_swizzle {
        let [x, y, z] = compute_group(ex);
        let count = x.checked_mul(y).and_then(|n| n.checked_mul(z)).unwrap_or(0);
        if count == 0 || count > 1024 || count % 32 != 0 {
            return Err("compute swizzle requires complete blocks of 32 lanes".into());
        }
        let last = em.insns.iter().rposition(exchanges).unwrap_or(0);
        if em.insns[..last].iter().any(|i| i.mnemonic == "s_endpgm") {
            return Err("compute swizzle following an early exit".into());
        }
        // Moving a shared exchange out of a forward guest branch is safe once every
        // invocation rejoins. A backward branch could execute it different numbers of times.
        for branch in em.insns.iter().filter(|i| i.enc == Enc::Sopp && Emitter::branch_cond(i.op).is_some() && i.aux.imm < 0) {
            let head = branch.offset as i64 + 4 + 4 * branch.aux.imm;
            if em.insns.iter().any(|i| exchanges(i) && (head..=branch.offset as i64).contains(&(i.offset as i64))) {
                return Err("compute swizzle inside a loop".into());
            }
        }
    }
    if (em.compute_swizzle || em.indexed_lane) && wave_cull::lds_words(&em).is_none()
        && em.insns.iter().any(|i| i.enc == Enc::Ds && i.mnemonic != "ds_swizzle_b32") {
        let bytes = tuxbox_dxbc::fssm::compute_lds_bytes(&ex.fssm)
            .ok_or("shared exchange requires the native guest LDS allocation")?;
        let threads = compute_group(ex).into_iter().try_fold(1u32, u32::checked_mul).ok_or("invalid group size")?;
        if bytes == 0 || bytes + 4*threads > 32768 { return Err("guest LDS and exchange exceed the group shared budget".into()); }
        em.reflected_lds_words = Some(bytes/4);
    }
    if let Some(width) = required_wave_size {
        match wave_sum::check_control_flow(&em.insns, width) {
            Ok(guards) => em.reduction_execz_guards = guards,
            Err(reason) if em.resolve_wave32 && !requires_ballot && scan_width.is_none() => {
                if !wave_loops::uniform_dispatch(&em) {
                    em.wave_bounded_latches = wave_loops::bounded_latches(&em).ok_or(reason)?;
                }
            }
            Err(reason) => {
                let ordinary = ex.stage == Stage::Compute && !em.wave_cull && !em.page_waves
                    && !em.resolve_wave32 && !lone_lane::on();
                em.lockstep = Some(ordinary.then(|| lockstep::prove(&em.insns, width)).flatten().ok_or(reason)?);
            }
        }
        let [x, y, z] = compute_group(ex);
        if ex.stage != Stage::Compute || !x.checked_mul(y).and_then(|n| n.checked_mul(z)).is_some_and(|n| n % width == 0 || partial_election && n > 0 && n < width) {
            return Err(format!("wave operation requires complete {width}-lane compute waves"));
        }
    }
    em.portable_wave64 = (straight_exchange || (required_wave_size == Some(64)
        && compute_group(ex) == [64, 1, 1]))
        && em.lockstep.is_none()
        && ex.stage == Stage::Compute
        && !wave_cull && !page_waves && !allocation_wave64 && !requires_ballot
        && !em.compute_swizzle && !em.indexed_lane && !em.compute_heap
        && em.packed.is_none() && !uses_quad_add && !uses_bpermute && !probe_trace
        && scan_width.is_none() && !particle_sort && !hair_cluster_export && !hair_cluster
        && !instance_cull && !hzb_cull && view_cull.is_none() && !post_cull
        && !hair_visibility_indirect && !hair_visibility_single && hair_compact.is_none()
        && !subsurface_setup && !material_classify && !classify_tiles && !reflection && !group_extrema
        && (straight_exchange || wave_sum::portable_single_wave(&em.insns));
    if straight_exchange && !em.portable_wave64 {
        return Err("straight DPP8 exchange conflicts with a specialized lowering".into());
    }
    if is_ps {
        em.find_export_epilogs();
        em.classify_interpolation();
    }
    em.infer_tbuf_types();
    if matches!(probe, Some(radiance_probe::Probe::ReflectionFar)) {
        // Compacted reflection work reads a count, packed pixel coordinates,
        // and a trace-distance texture with flags in its high bits.
        em.tbuf_ty.insert((false, 13), 1);
        em.tbuf_ty.insert((false, 14), 1);
        em.img_ty.insert((false, 12), 1);
    }
    if wave_cull && ex.entry.as_deref() == Some("WaterTileCatergorisationCS") {
        // Both the dispatch count and the packed XY tile list use R32_UINT.
        em.tbuf_uint.extend([0, 1]);
    }
    if particle_sort {
        // R32_UINT particle count, sort keys and particle indices.
        em.tbuf_ty.extend([((false, 3), 1), ((true, 0), 1), ((true, 1), 1)]);
    }
    // Both 512-thread hair scans load integer counts from R32_UINT and store
    // (exclusive, inclusive) offsets to R32G32_UINT. Their additions pass
    // through LDS, beyond the local register type-propagation window.
    if compute_group(ex) == [512, 1, 1]
        && (packed_root::matches_program(&ex.isa, 444, 0xf9b02aae1588e09b)
            || packed_root::matches_program(&ex.isa, 588, 0x4884ae622354895b))
    {
        for &binding in &em.tbuf {
            em.tbuf_ty.insert(binding, 1);
        }
    }
    if subsurface_setup || (wave_cull && ex.entry.as_deref() == Some("SetupIndirectCS")) {
        // These two R32_UINT UAVs hold both the atomic head and the XY rows.
        em.tbuf_uint.extend([0, 1]);
    }
    if wave_cull && ex.entry.as_deref() == Some("MarkValid_PrepareCS") {
        // t2 contains the integer page-grid dimensions and base. u2 receives
        // pairs of packed coordinates for the deferred page-marking pass.
        em.tbuf_ty.extend([((false, 2), 1), ((true, 2), 1)]);
    }
    if hair_multisample { hair_multisample::resources(&mut em)?; }
    if let Some(layout) = hair_compact { hair_compact::resources(&mut em, layout)?; }
    if hair_visibility_single { hair_visibility_single::resources(&mut em)?; }
    if hair_visibility_indirect { hair_visibility_indirect::resources(&mut em)?; }
    if material_classify { ps09_classify::resources(&mut em)?; }
    if classify_tiles { classify_materials::resources(&mut em)?; }
    if instance_cull { instance_cull::resources(&mut em)?; }
    if post_cull { instance_cull_post::resources(&mut em)?; }
    if hzb_cull { instance_cull_hzb::resources(&mut em)?; }
    if ex.stage == Stage::Compute && ex.entry.as_deref() == Some("InstanceCullVSM") {
        // Scalar loads address individual words of the 800-byte packed views,
        // and the compacted ranges, counters and scene streams have distinct strides.
        for (&(uav, slot), words) in &mut em.sbuf {
            *words = fssm_bindings::structured_words(ex, slot, uav)?;
        }
    }
    if matches!(probe, Some(radiance_probe::Probe::ScreenGather | radiance_probe::Probe::ScreenGatherFar)) {
        // The screen trace packs distance and flags into the integer hit target.
        em.uav_uint.insert(1);
    }
    if let Some(program) = view_cull { program.resources(&mut em)?; }
    if matches!(ex.stage, Stage::Compute | Stage::Pixel)
        && em.packed.is_none() && !em.uav_uint.is_empty() {
        em.resolve_wide_image_atomics()?;
    }
    if ex.entry.as_deref() == Some("MicropolyRasterize") && ex.stage == Stage::Compute {
        micropoly::prepare(&mut em)?;
    }
    em.resolve_structured_strides();
    let live_in = em.live_in();
    em.live = live_in.clone();
    em.view_cull = view_cull;
    em.light_grid = light_grid;
    em.emit_body(is_ps)?;
    // Each guarded VGPR write would otherwise be its own branch. The region
    // closes before anything other than pure register arithmetic, so this is
    // the same program in every stage, with fewer blocks for DXC and dxil-spirv.
    em.body = exec_group::group(&em.body);
    if hair_cluster_export {
        em.body.insert_str(0, "    uint4 hair_record = 0;\n");
        em.body.push_str(hair_cluster_export::EPILOGUE);
    }
    if hair_cluster {
        em.body.insert_str(0, "    uint4 hair_record = 0;
");
        em.body.push_str(hair_cluster::EPILOGUE);
    }
    if instance_cull {
        em.body.insert_str(0, "    bool cull_candidate = false; uint4 cull_record = 0;\n");
        em.body.push_str(instance_cull::EPILOGUE);
    }
    if post_cull {
        em.body.insert_str(0, "    bool cull_candidate = false; uint4 cull_record = 0;\n");
        em.body.push_str(instance_cull_post::APPEND);
    }
    if hzb_cull {
        em.body.insert_str(0, "    bool cull_candidate = false, hidden = false; uint4 cull_record = 0; uint hidden_id = 0; const uint hidden_view = 0u;\n");
        em.body.push_str(instance_cull_hzb::HIDDEN_APPEND);
        em.body.push_str(instance_cull::EPILOGUE);
    }
    if particle_sort { em.body.push_str(particle_sort::EPILOGUE); }
    let mut hlsl = em.finish(is_ps, output_order, stencil_target)?;
    if hzb_cull {
        hlsl.insert_str(0, &instance_cull::queue_declarations(3));
        hlsl.insert_str(0, instance_cull_hzb::HIDDEN_DECLARATIONS);
    }
    if light_grid { hlsl.insert_str(0, light_grid::DECLARATIONS); }
    if em.portable_wave64 { hlsl.insert_str(0, wave_sum::PORTABLE_DECLARATIONS); }
    if shared_mask {
        hlsl.insert_str(0, &page_waves::ballot_declarations(compute_group(ex).into_iter().product()));
    }
    if page_waves || wave_cull {
        if fspx_wave == Some(32) {
            hlsl.insert_str(0, fspx_wave::WAVE32);
        } else {
            let branchless = wave_cull && wave_cull::needs_branchless_ballots(ex);
            hlsl.insert_str(0, &page_waves::declarations(compute_group(ex), branchless));
        }
    }
    if let Some(probe) = probe {
        hlsl.insert_str(0, radiance_probe::DECLARATIONS);
        if matches!(probe, radiance_probe::Probe::ReflectionFar | radiance_probe::Probe::Reflection | radiance_probe::Probe::ScreenGather | radiance_probe::Probe::ScreenGatherFar) { hlsl.insert_str(0, "#define TB_SCREEN_GATHER 1\n"); }
        if matches!(probe, radiance_probe::Probe::ReflectionFar | radiance_probe::Probe::FarField | radiance_probe::Probe::ScreenGatherFar) { hlsl.insert_str(0, "#define TB_PROBE_FAR_FIELD 1\n"); }
    }
    if particle_sort { hlsl.insert_str(0, particle_sort::DECLARATIONS); }
    if hair_cluster { hlsl.insert_str(0, hair_cluster::DECLARATIONS); }
    if hair_cluster_export { hlsl.insert_str(0, hair_cluster_export::DECLARATIONS); }
    if instance_cull { hlsl.insert_str(0, &instance_cull::queue_declarations(1)); }
    if post_cull { hlsl.insert_str(0, "groupshared uint cull_mask[2], cull_base;\n"); }
    if let Some(program) = view_cull { hlsl.insert_str(0, &program.declarations()); }
    if material_classify { hlsl.insert_str(0, ps09_classify::DECLARATIONS); }
    hlsl.insert_str(0, &format!("// entry-live registers (read before written): {}
", live_in.join(" ")));
    // ps_5_0 forces UAV slots to start after the render-target outputs (FXC X4509);
    // SM5.1 has no such rule, and D3D12 accepts SM5.1 bytecode.
    let profile = if em.compute_derivatives || em.resolve_wave32 {
        "cs_6_6"
    } else if em.compute_heap && !is_ps {
        "cs_6_6"
    } else if em.uses_heap_sampler || em.resource_heap_base.is_some() || em.pixel_heap {
        "ps_6_6"
    } else if probe_trace {
        "cs_6_5"
    } else if is_ps && !em.uav.is_empty() {
        "ps_5_1"
    } else {
        profile
    };
    Ok(Translated {
        hlsl,
        entry: "main",
        profile,
        needs_barycentrics: !em.pulled_attrs.is_empty(),
        packed_dot_product: fspx_wave.is_some(),
        typed_int64_atomics: !em.wide_visibility_uavs.is_empty() || em.packed.as_ref().is_some_and(|p| !p.wide_images.is_empty()),
        required_wave_size: if em.portable_wave64 { None } else { required_wave_size },
        portable_compute_group: portable_compute_group.or((light_grid || em.portable_wave64).then(|| compute_group(ex))),
        ps_input_interp: std::mem::take(&mut em.ps_in_interp),
    })
}

/// Dwords per thread in the I/O buffer of [`translate_valu_probe`]: `v0..v{n-1}`, `vcc_lo`,
/// then `exec_lo` (stored only: every thread starts active).
pub fn valu_probe_stride(nvgprs: u32) -> u32 {
    nvgprs + 2
}

/// Emit one LDS exchange inside a synthetic 32-lane control. This diagnostic
/// is not a translation of the surrounding program or its wave control flow.
pub fn translate_lds_exchange_probe(ex: &ShaderExtract, offset: usize) -> Result<String, String> {
    if ex.stage != Stage::Compute {
        return Err("LDS probe needs a compute shader".into());
    }
    let ins = disassemble(&ex.isa).into_iter().find(|i| i.offset == offset)
        .ok_or("missing LDS probe instruction")?;
    let (addr, data, dst) = match (ins.mnemonic.as_str(), ins.src.as_slice(), ins.dst.as_slice()) {
        ("ds_wrxchg_rtn_b32", [Opnd::Vgpr(a), Opnd::Vgpr(v), _], [Opnd::Vgpr(d)]) => (*a, *v, *d),
        _ => return Err("expected one LDS write-exchange instruction".into()),
    };
    let mut em = Emitter::new(ex, vec![ins.clone()]);
    for op in ins.src.iter().chain(ins.dst.iter()) { em.touch(op); }
    em.emit_insn(0, &ins, false, None)?;
    let mut h = String::from("RWBuffer<uint> tb_io : register(u0);\ngroupshared uint tb_lds[8192];\n[numthreads(32,1,1)] void main(uint3 tid : SV_DispatchThreadID) {\n");
    h.push_str("uint exec_lo=1u, exec_hi=0u, vcc_lo=0u, vcc_hi=0u, scc=0u, m0=0u, nul=0u;\n");
    h.push_str(&em.reg_decls());
    let _ = writeln!(h, "\nuint lane=tid.x; tb_lds[lane]=tb_io[lane*4+2]; GroupMemoryBarrierWithGroupSync();");
    let _ = writeln!(h, "v{addr}=tb_io[lane*4]; v{data}=tb_io[lane*4+1]; exec_lo=tb_io[lane*4+3];");
    h.push_str(&em.body);
    h.push_str("GroupMemoryBarrierWithGroupSync();\n");
    let _ = writeln!(h, "tb_io[lane*4+1]=v{dst}; tb_io[lane*4+2]=tb_lds[lane]; }}");
    Ok(h)
}

/// Conformance probe: straight-line vector-ALU code (VOP1/VOP2/VOP3/VOPC/VOP3P words, ending
/// with or without `s_endpgm`) translated with the same per-instruction emitter as real
/// shaders, wrapped in a compute shader that loads each thread's `v0..v{nvgprs-1}` and
/// `vcc_lo` from a raw UAV buffer, runs the code, and stores them back (layout:
/// [`valu_probe_stride`] dwords per thread). The root signature (`UAV(u0)`) is embedded, so
/// the compiled DXIL can create its own root signature.
///
/// This exists so instruction semantics can be compared against a reference model on a real
/// GPU (tests/valu_conformance.rs). Anything other than vector ALU code is refused.
pub fn translate_valu_probe(words: &[u32], nvgprs: u32) -> Result<String, String> {
    let isa: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let ex = ShaderExtract {
        stage: Stage::Compute,
        shader_model: (6, 0),
        required_features: None,
        bitcode_stripped: true,
        fspx_config: None,
        ps09_pixel_inputs: None,
        fssm: Vec::new(),
        entry: Some("main".into()),
        isa_data: isa.clone(),
        isa,
        isa_offset_in_fssm: 0,
        psv_threads: None,
        flat_inputs: Vec::new(),
        flat_outputs: Vec::new(),
        isa_preamble: [0, 64, 1, 1],
        has_input_sig: false,
        has_output_sig: false,
        has_psv0: false,
        has_root_sig: false,
        input_sig: Vec::new(),
        output_sig: Vec::new(),
        resources: Vec::new(),
        gs_info: None,
        mesh_info: None,
        primitive_sig: vec![],
        root_sig: None,
    };
    let insns = disassemble(&ex.isa);
    let mut em = Emitter::new(&ex, insns.clone());
    for (idx, ins) in insns.iter().enumerate() {
        if ins.mnemonic == "s_endpgm" {
            break;
        }
        if !matches!(ins.enc, Enc::Vop1 | Enc::Vop2 | Enc::Vop3 | Enc::Vopc) {
            return Err(format!("{} ({:?}) is not vector ALU code", ins.mnemonic, ins.enc));
        }
        em.emit_insn(idx, ins, false, None).map_err(|e| format!("{}: {e}", ins.mnemonic))?;
    }
    for n in 0..nvgprs {
        em.regs.insert(format!("v{n}"));
    }
    if let Some(r) = em.regs.iter().find(|r| {
        r.strip_prefix('v').and_then(|n| n.parse::<u32>().ok()).is_some_and(|n| n >= nvgprs)
    }) {
        return Err(format!("probe code uses {r}, beyond the {nvgprs} VGPRs loaded"));
    }
    let stride = valu_probe_stride(nvgprs) * 4;
    let mut h = String::new();
    em.push_helpers(&mut h);
    h.push_str("RWByteAddressBuffer tb_io : register(u0);\n");
    h.push_str("[RootSignature(\"UAV(u0)\")]\n[numthreads(64, 1, 1)]\nvoid main(uint3 tid : SV_DispatchThreadID) {\n");
    h.push_str("    uint exec_lo = 1u, exec_hi = 0u, vcc_lo = 0u, vcc_hi = 0u, scc = 0u, m0 = 0u, nul = 0u;\n");
    h.push_str(&em.reg_decls());
    let _ = writeln!(h, "\n    uint tb_base = tid.x * {stride}u;");
    for n in 0..nvgprs {
        let _ = writeln!(h, "    v{n} = tb_io.Load(tb_base + {}u);", n * 4);
    }
    let _ = writeln!(h, "    vcc_lo = tb_io.Load(tb_base + {}u);", nvgprs * 4);
    h.push_str(&em.body);
    for n in 0..nvgprs {
        let _ = writeln!(h, "    tb_io.Store(tb_base + {}u, v{n});", n * 4);
    }
    let _ = writeln!(h, "    tb_io.Store(tb_base + {}u, vcc_lo);", nvgprs * 4);
    let _ = writeln!(h, "    tb_io.Store(tb_base + {}u, exec_lo);", nvgprs * 4 + 4);
    h.push_str("}\n");
    Ok(h)
}

/// Descriptor-table sizes of a [`translate_mem_probe`] shader: how many SRV, sampler and UAV
/// descriptors its root signature declares (at least one each; unused slots may be null).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemProbeTables {
    pub srv: u32,
    pub samplers: u32,
    pub uav: u32,
}

/// User SGPRs holding the descriptor-table pointers in a [`translate_mem_probe`] shader.
pub const MEM_PROBE_SRV_TABLE: u32 = 4;
pub const MEM_PROBE_SAMPLER_TABLE: u32 = 5;
pub const MEM_PROBE_UAV_TABLE: u32 = 6;

/// Conformance probe for vector-memory code: like [`translate_valu_probe`] (same I/O buffer
/// layout, now at `u0, space1`), but the code may also fetch descriptors with `s_load_dword*`
/// from the root tables in s4 (SRVs: T#/V#), s5 (samplers: S#) and s6 (UAVs), and use them
/// in MIMG / MUBUF instructions. A T#/V# at byte offset `32 * i` of its table is HLSL
/// register `t<i>` / `u<i>`, an S# at `16 * i` is `s<i>` (the console's descriptor strides,
/// see `resolve_resources`). Descriptors are resolved by the same dataflow as game shaders,
/// so what is tested is the translation real shaders get.
///
/// The root signature (three descriptor tables, then the I/O UAV) is embedded; table sizes
/// are returned so the caller can fill every slot.
pub fn translate_mem_probe(words: &[u32], nvgprs: u32) -> Result<(String, MemProbeTables), String> {
    use crate::meta::{RootParam, RootParamKind, RootRange, RootSig};
    let isa: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let table = |range_type: u32| RootParam {
        visibility: 0,
        kind: RootParamKind::Table(vec![RootRange { range_type, num: 1, base_register: 0, space: 0, offset_in_table: 0 }]),
    };
    let ex = ShaderExtract {
        stage: Stage::Compute,
        shader_model: (6, 0),
        required_features: None,
        bitcode_stripped: true,
        fspx_config: None,
        ps09_pixel_inputs: None,
        fssm: Vec::new(),
        entry: Some("main".into()),
        isa_data: isa.clone(),
        isa,
        isa_offset_in_fssm: 0,
        psv_threads: None,
        flat_inputs: Vec::new(),
        flat_outputs: Vec::new(),
        isa_preamble: [0, 64, 1, 1],
        has_input_sig: false,
        has_output_sig: false,
        has_psv0: false,
        has_root_sig: true,
        input_sig: Vec::new(),
        output_sig: Vec::new(),
        resources: Vec::new(),
        gs_info: None,
        mesh_info: None,
        primitive_sig: vec![],
        // Root parameters 0, 1, 2 arrive in s4, s5, s6 (compute user-data base 4).
        root_sig: Some(RootSig { version: 1, flags: 0, params: vec![table(0), table(3), table(1)], num_static_samplers: 0, static_samplers: vec![] }),
    };
    let insns = disassemble(&ex.isa);
    let mut em = Emitter::new(&ex, insns.clone());
    em.resolve_resources()?;
    em.infer_tbuf_types();
    for (idx, ins) in insns.iter().enumerate() {
        if ins.mnemonic == "s_endpgm" {
            break;
        }
        let ok = match ins.enc {
            Enc::Vop1 | Enc::Vop2 | Enc::Vop3 | Enc::Vopc | Enc::Mimg | Enc::Mubuf => true,
            Enc::Smem => ins.op <= 4 || (8..=12).contains(&ins.op), // descriptor and buffer loads
            Enc::Sop1 => ins.mnemonic == "s_mov_b32" || ins.mnemonic == "s_mov_b64", // table-pointer copies
            Enc::Sopp => ins.mnemonic == "s_waitcnt" || ins.mnemonic == "s_nop",
            _ => false,
        };
        if !ok {
            return Err(format!("{} ({:?}) is not supported in a memory probe", ins.mnemonic, ins.enc));
        }
        em.emit_insn(idx, ins, false, None).map_err(|e| format!("{}: {e}", ins.mnemonic))?;
    }
    for n in 0..nvgprs {
        em.regs.insert(format!("v{n}"));
    }
    // The table-pointer registers may be copied (their values are meaningless in HLSL:
    // descriptors are resolved statically).
    for n in [MEM_PROBE_SRV_TABLE, MEM_PROBE_SAMPLER_TABLE, MEM_PROBE_UAV_TABLE] {
        em.regs.insert(format!("s{n}"));
    }
    if let Some(r) = em.regs.iter().find(|r| {
        r.strip_prefix('v').and_then(|n| n.parse::<u32>().ok()).is_some_and(|n| n >= nvgprs)
    }) {
        return Err(format!("probe code uses {r}, beyond the {nvgprs} VGPRs loaded"));
    }
    // Table sizes: highest slot used + 1.
    let top = |it: &mut dyn Iterator<Item = u32>| it.max().map_or(1, |m| m + 1);
    let tables = MemProbeTables {
        srv: top(&mut em.tex.keys().copied().chain(em.sbuf.keys().chain(em.tbuf.iter()).filter(|k| !k.0).map(|k| k.1))),
        samplers: top(&mut em.samp.iter().chain(em.samp_cmp.iter()).copied()),
        uav: top(&mut em.uav.keys().copied().chain(em.sbuf.keys().chain(em.tbuf.iter()).filter(|k| k.0).map(|k| k.1))),
    };
    if !em.cb_size.is_empty() || em.uses_code || em.uses_lds {
        return Err("memory probe code may only use SRV, sampler and UAV tables".into());
    }
    let stride = valu_probe_stride(nvgprs) * 4;
    let mut h = String::new();
    em.push_helpers(&mut h);
    h.push_str(&em.resource_decls()?);
    h.push_str("RWByteAddressBuffer tb_io : register(u0, space1);\n");
    let _ = writeln!(
        h,
        "[RootSignature(\"DescriptorTable(SRV(t0, numDescriptors={})), DescriptorTable(Sampler(s0, numDescriptors={})), \
         DescriptorTable(UAV(u0, numDescriptors={})), UAV(u0, space=1)\")]",
        tables.srv, tables.samplers, tables.uav
    );
    h.push_str("[numthreads(64, 1, 1)]\nvoid main(uint3 tid : SV_DispatchThreadID) {\n");
    h.push_str("    uint exec_lo = 1u, exec_hi = 0u, vcc_lo = 0u, vcc_hi = 0u, scc = 0u, m0 = 0u, nul = 0u;\n");
    h.push_str(&em.reg_decls());
    let _ = writeln!(h, "\n    uint tb_base = tid.x * {stride}u;");
    for n in 0..nvgprs {
        let _ = writeln!(h, "    v{n} = tb_io.Load(tb_base + {}u);", n * 4);
    }
    let _ = writeln!(h, "    vcc_lo = tb_io.Load(tb_base + {}u);", nvgprs * 4);
    h.push_str(&em.body);
    for n in 0..nvgprs {
        let _ = writeln!(h, "    tb_io.Store(tb_base + {}u, v{n});", n * 4);
    }
    let _ = writeln!(h, "    tb_io.Store(tb_base + {}u, vcc_lo);", nvgprs * 4);
    let _ = writeln!(h, "    tb_io.Store(tb_base + {}u, exec_lo);", nvgprs * 4 + 4);
    h.push_str("}\n");
    Ok((h, tables))
}

impl<'a> Emitter<'a> {
    fn one_word_vector_mask(&self) -> bool {
        self.wave32_compare_masks || self.single_wave32 || self.resolve_wave32
            || self.packed.as_ref().is_some_and(|p| p.abi == RootAbi::RegisterWave32)
    }

    fn new(ex: &'a ShaderExtract, insns: Vec<Insn>) -> Self {
        let has_mode = insns.iter().any(|i| matches!((i.enc, i.op), (Enc::Sopp, 0x24 | 0x25))
            || (i.enc == Enc::Sopk && matches!(i.op, 0x12 | 0x13 | 0x15) && i.raw[0] & 63 == 1));
        // A wave64 VALU mask occupies an even-aligned SGPR pair. An odd
        // VOP3 comparison destination proves one-word masks throughout the
        // guest program, including comparisons that target even registers.
        let wave32_compare_masks = insns.iter().any(|ins| {
            ins.enc == Enc::Vop3 && ins.mnemonic.starts_with("v_cmp_")
                && matches!(ins.dst.as_slice(), [Opnd::Sgpr(n)] if n & 1 != 0)
        });
        let ps_attrs = params::inputs(ex, &insns);
        Emitter {
            lds_tile: lds_tile::LdsTile::recognize(ex, &insns),
            packed: None,
            loop_depth: 0,
            ex,
            insns,
            res: HashMap::new(),
            cb_size: BTreeMap::new(),
            zero_buffer_offsets: Default::default(),
            ordinary_bit_sites: Default::default(),
            smem_constants: Default::default(),
            tex: BTreeMap::new(),
            samp: BTreeSet::new(),
            samp_cmp: BTreeSet::new(),
            uav: BTreeMap::new(),
            sbuf: BTreeMap::new(),
            reflected_strides: BTreeSet::new(),
            persistent_queue: false,
            raw: BTreeSet::new(),
            raw_byte_index: BTreeSet::new(),
            tbuf: BTreeSet::new(),
            tbuf_uint: BTreeSet::new(),
            tbuf_ty: BTreeMap::new(),
            img_ty: BTreeMap::new(),
            uses_heap_sampler: false,
            resource_heap_base: sampler_heap::resource_base(ex).or_else(|| compute_heap_base(ex)).or_else(|| pixel_heap_base(ex, 5)),
            compute_heap: sampler_heap::resource_base(ex).is_none() && (compute_heap_base(ex).is_some() || pixel_heap_base(ex, 5).is_some()),
            sampler_heap_base: compute_heap_base(ex).and_then(|_| compute_user_sgpr(ex, 6)).or_else(|| pixel_heap_base(ex, 6)),
            pixel_heap: pixel_heap_base(ex, 5).is_some() || pixel_heap_base(ex, 6).is_some(),
            table_samplers: BTreeSet::new(),
            heap_load_ty: HashMap::new(),
            heap_sampler_slots: BTreeSet::new(),
            heap_map: ex.root_sig.as_ref().is_some_and(|root| root.params.iter().any(|p| matches!(p.kind,
                crate::meta::RootParamKind::Descriptor { kind: 3, register: 0, space: 32766 }))),
            private_samplers: ex.root_sig.as_ref().filter(|r| matches!(ex.stage, Stage::Compute | Stage::Pixel)
                // Literal samplers in heap-indexing compute shaders are recovered the same
                // way; samplers read from the sampler heap stay unresolved and fail.
                && ex.required_features.is_some_and(|flags| flags & (3 << 25) == 0 || ex.stage == Stage::Compute)
                && r.static_samplers.iter().any(|s| s[11] == 32765))
                .and_then(|r| {
                    let recovered = tuxbox_dxbc::inline_sampler::sites(&ex.isa, if ex.stage == Stage::Pixel { 5 } else { 0 })?;
                    (recovered.descriptors == r.static_samplers).then_some(recovered.sites)
                })
                // Root-less programs that declare several samplers: the literal words cannot
                // be matched to the title's registers here, so use the private slots that
                // inline_sampler::sites derives from the ISA alone (space 32765). The runtime
                // appends the same descriptors to the host root signature.
                .or_else(|| ((ex.root_sig.is_none()
                    && ex.resources.iter().filter(|r| r.res_type == 1).count() > 1)
                    // A root with no static samplers: the runtime recovers the private ones
                    // (compute_table.rs, inline_sampler::private) from the same sites.
                    || (matches!(ex.stage, Stage::Compute | Stage::Pixel)
                        && ex.root_sig.as_ref().is_some_and(|r| r.static_samplers.is_empty())
                        && ex.resources.iter().any(|r| r.res_type == 1)))
                    .then(|| {
                        let visibility = if ex.stage == Stage::Pixel { 5 } else { 0 };
                        if ex.root_sig.is_none() { tuxbox_dxbc::inline_sampler::literal_sites(&ex.isa, visibility) }
                        else { tuxbox_dxbc::inline_sampler::sites(&ex.isa, visibility) }
                    })
                    .flatten()
                    .map(|recovered| recovered.sites))
                .unwrap_or_default(),
            binding_spaces: BTreeMap::new(),
            mesh_bindings: None,
            compute_derivatives: false,
            epilog_pack: HashMap::new(),
            epilog_exp: HashMap::new(),
            cur_idx: 0,
            sbuf_flat: BTreeSet::new(),
            is_cs: ex.stage == Stage::Compute,
            is_gs: ex.stage == Stage::Geometry,
            uses_lds: false,
            uses_code: false,
            vreg_array: false,
            multi_sets: Vec::new(),
            uav_uint: BTreeSet::new(),
            wide_visibility_uavs: BTreeSet::new(),
            micropoly_vote: None,
            view_cull: None,
            required_wave_size: None,
            reduction_execz_guards: BTreeSet::new(),
            lockstep: None,
            wave_bounded_latches: BTreeSet::new(),
            portable_wave64: false,
            msel: HashMap::new(),
            bsel: HashMap::new(),
            dsel_loads: HashMap::new(),
            dsel_runtime: HashMap::new(),
            dsel_moves: HashMap::new(),
            live: Vec::new(),
            uses_quad: false,
            compute_swizzle: false,
            vote_defs: BTreeSet::new(),
            vote_uses: BTreeMap::new(),
            exec_snapshots: Default::default(),
            reflected_lds_words: None,
            hoisted_swizzle: None,
            indexed_lane: false,
            single_wave32: false,
            resolve_wave32: false,
            wave32_compare_masks,
            light_grid: false,
            page_waves: false,
            wave_cull: false,
            own_lane_reads: BTreeSet::new(),
            chosen_samplers: BTreeSet::new(),
            loop_ranges: Vec::new(),
            fspx_wave: None,
            hair_multisample: false,
            hoist_wave_exchanges: false,
            regs: if has_mode { BTreeSet::from(["tb_fp_mode".into()]) } else { BTreeSet::new() },
            body: String::new(),
            heap_locals: BTreeSet::new(),
            heap_bound: BTreeSet::new(),
            tmp: 0,
            // PS attribute slot k = the k-th input *register* that carries an interpolant and is
            // read. A register counts as read if any element packed into it is read — including
            // a system value sharing it (observed: PARTICLE_RANDOM unread + SV_IsFrontFace read
            // in the same register still occupies an attribute slot; d26fd88ff8712f9e).
            pulled_attrs: BTreeSet::new(),
            pixel_wave: false,
            dispatched: false,
            pixel_sum: false,
            flat_attrs: BTreeSet::new(),
            ps_in_interp: InputInterp::default(),
            out_interp: InputInterp::default(),
            ps_attrs,
            out_params: params::outputs(ex, &ex.output_sig),
        }
    }

    /// Root-signature range type (0 SRV, 1 UAV, 2 CBV, 3 Sampler) of the descriptor table
    /// whose pointer lives in user SGPR `sgpr` at shader entry. The FSSM register table
    /// maps user SGPR (base + i) to root parameter i, with base 8 for graphics stages and
    /// 4 for compute (observed: PS s8/s9/s10 = params 0/1/2, VS s12 = param 4, CS s5/s7 =
    /// params 1/3). `None` when the root signature is unavailable or the SGPR is not a table.
    fn root_table(&self, sgpr: u8) -> Option<&[crate::meta::RootRange]> {
        // Root descriptor-table pointers arrive in consecutive user SGPRs: from s4 in compute
        // shaders and s8 in the other stages. Vertex shaders compiled to feed a geometry
        // shader ("MainForGS") start with `s_swappc_b64 s12, s12` and receive them from s19
        // 7 registers after the swapped PC register pair instead (observed: `s_swappc_b64
        // s12, s12` -> SRV table s19, CBV s20 in a55400ccab602283; `s_swappc_b64 s4, s4` ->
        // SRV s11, CBV s12 in 4d318a496eebf6d3).
        let chained = self.insns.iter().take(16).find(|i| i.enc == Enc::Sop1 && i.mnemonic == "s_swappc_b64").and_then(|i| match i.dst.first() {
            Some(Opnd::Sgpr(n)) => Some(*n as usize + 7),
            _ => None,
        });
        // Geometry shaders: SRV table in s22, CBV table in s23 (6a6bdf123db8b5b0).
        let base = match self.ex.stage {
            Stage::Vertex if sgpr == 8 && sampler_heap::vertex_s8_table(self.ex) => 8,
            Stage::Compute => 4,
            Stage::Geometry => 22,
            // In the NGG ABI s25/s26 are mesh-visible parameter tables 9/10.
            Stage::Mesh => 16,
            _ => chained.unwrap_or(8),
        };
        let idx = (sgpr as usize).checked_sub(base)?;
        match &self.ex.root_sig.as_ref()?.params.get(idx)?.kind {
            crate::meta::RootParamKind::Table(r) => Some(r),
            _ => None,
        }
    }

    /// Resolve the guest descriptor's physical table element to its shader register.
    /// Resources occupy 32-byte table elements; samplers occupy 16-byte elements.
    /// PS09 0x41 records give the exact offsets of mixed UAV and CBV ranges.
    fn table_binding(&mut self, sgpr: u8, offset: u32, stride: u32) -> Result<(Option<u32>, u32), String> {
        let (kind, slot, space) = self.table_binding_space(sgpr, offset, stride)?;
        if self.pixel_heap && kind.is_none() {
            return Err(format!("unmapped descriptor at s{sgpr} + {offset:#x} in a heap-indexing pixel shader"));
        }
        if let Some(kind) = kind {
            // Resources are named after their register. One number used again in another
            // space gets a name of its own.
            let free = (0..16).map(|n| slot + n * SPACE_ALIAS)
                .find(|name| self.binding_spaces.get(&(kind, *name)).is_none_or(|&used| used == space));
            let Some(name) = free.filter(|_| slot < SPACE_ALIAS) else {
                return Err(format!("descriptor class {kind} register {slot} is used in multiple spaces"));
            };
            self.binding_spaces.insert((kind, name), space);
            return Ok((Some(kind), name));
        }
        Ok((kind, slot))
    }

    /// The register a resource name stands for (see `table_binding`).
    fn register_of(&self, kind: u32, name: u32) -> u32 {
        let register = name % SPACE_ALIAS;
        if name >= SPACE_ALIAS && self.binding_spaces.contains_key(&(kind, name)) && self.binding_spaces.contains_key(&(kind, register)) {
            register
        } else {
            name
        }
    }

    // A console compiler can inline all four sampler words. When PSV names
    // exactly one sampler, its (register, space) must match the static root entry.
    fn static_sampler(&self) -> Option<(u32, u32)> {
        let mut samplers = self.ex.resources.iter().filter(|p| p.res_type == 1);
        let sampler = samplers.next()?;
        if samplers.next().is_some() || sampler.lower != sampler.upper {
            return None;
        }
        self.ex.root_sig.as_ref()?.static_samplers.iter()
            .any(|entry| entry[10] == sampler.lower && entry[11] == sampler.space && (entry[12] == 0 || (self.ex.stage == Stage::Pixel && entry[12] == 5)))
            .then_some((sampler.lower, sampler.space))
    }

    fn in_loop(&self, idx: usize) -> bool {
        self.loop_ranges.iter().any(|&(head, back)| (head..=back).contains(&idx))
    }

    fn space_suffix(&self, kind: u32, register: u32) -> String {
        match self.binding_spaces.get(&(kind, register)) {
            Some(space) if *space != 0 => format!(", space{space}"),
            _ => String::new(),
        }
    }

    fn table_binding_space(&self, sgpr: u8, offset: u32, stride: u32) -> Result<(Option<u32>, u32, u32), String> {
        if let Some(mesh) = &self.mesh_bindings {
            let binding = mesh.tables.get(&(sgpr, offset)).filter(|_| stride == 32)
                .ok_or_else(|| format!("unverified mesh descriptor at s{sgpr} + {offset:#x}"))?;
            return Ok((Some(binding.kind), binding.register, binding.space));
        }
        let index = offset / stride;
        let Some(ranges) = self.root_table(sgpr) else {
            if let Some(found) = self.rootless_fssm_descriptor(sgpr, offset, stride) {
                return Ok(found);
            }
            return Ok((None, index, 0));
        };
        // Existing FSSM single-range layouts may address beyond the declared range.
        if let [range] = ranges {
            if range.base_register == 0 && range.space == 0 && matches!(range.offset_in_table, 0 | u32::MAX) {
                // The runtime can append a reflected descriptor beyond a finite 1.1
                // table. Use that binding before extrapolating the single range.
                if index >= range.num && stride == 32 && self.ex.root_sig.as_ref().is_some_and(|r| r.version == 2) {
                    if let Some((kind, register, space)) = self.uncovered_fssm_descriptor(sgpr, offset) {
                        return Ok((Some(kind), register, space));
                    }
                }
                return Ok((Some(range.range_type), index, 0));
            }
        }
        let mut next = 0u64;
        for range in ranges {
            let base = if range.offset_in_table == u32::MAX { next } else { u64::from(range.offset_in_table) };
            next = base + u64::from(range.num);
            if (base..next).contains(&u64::from(index)) {
                if offset % stride != 0 { return Err("unaligned descriptor table offset".into()); }
                let register = range.base_register.checked_add((u64::from(index) - base) as u32).ok_or("descriptor register overflow")?;
                if sgpr == 8 && sampler_heap::index(self.ex, 9).is_some() &&
                    !sampler_heap::matches_table(self.ex, offset, range.range_type, register, range.space) {
                    return Err(format!("root table slot at {offset:#x} conflicts with the pixel stage physical binding"));
                }
                return Ok((Some(range.range_type), register, range.space));
            }
        }
        // The root table does not cover this 32-byte slot. Accept the load only
        // when one FSSM user record, its used-resource row and PSV name the same
        // register, and that register is not already declared elsewhere.
        if stride == 32 {
            if let Some((kind, register, space)) = self.uncovered_fssm_descriptor(sgpr, offset) {
                return Ok((Some(kind), register, space));
            }
            // Raster roots can omit console table slots entirely (PS_Composite reads s9 +
            // 0x540 past a 7-range table). Name the register the FSSM row and PSV agree on;
            // the runtime builds the host table from the same rows.
            if self.ex.stage != Stage::Compute {
                if let Some(found) = self.fssm_descriptor(sgpr, offset, stride) {
                    return Ok(found);
                }
            }
        }
        Err(format!("unmapped descriptor at s{sgpr} + {offset:#x}"))
    }

    /// Root-less programs (most RAGE shaders) carry no RTS0, so no root table names their
    /// descriptors. FSSM used-resource rows still give each descriptor's table SGPR
    /// (location bits 8..16), dword offset and register; PSV gives its space. Accept a
    /// load only when exactly one row sits at this SGPR and offset and exactly one PSV
    /// binding of the matching class names its register.
    fn rootless_fssm_descriptor(&self, sgpr: u8, offset: u32, stride: u32) -> Option<(Option<u32>, u32, u32)> {
        if self.ex.root_sig.is_some() {
            return None;
        }
        self.fssm_descriptor(sgpr, offset, stride)
    }

    fn fssm_descriptor(&self, sgpr: u8, offset: u32, stride: u32) -> Option<(Option<u32>, u32, u32)> {
        if stride != 32 || offset % 32 != 0 {
            return None;
        }
        let mut rows = tuxbox_dxbc::compute_table::records(&self.ex.fssm)?.filter(|&[class, pos, _, _, loc]| {
            matches!(class, 0x300 | 0x302 | 0x303) && (loc >> 8) & 0xff == u32::from(sgpr) && pos.checked_mul(4) == Some(offset)
        });
        let [class, _, register, _, _] = rows.next()?;
        if rows.next().is_some() {
            return None;
        }
        let (kind, types): (u32, &[u32]) = match class {
            0x300 => (0, &[3, 4, 5]),
            0x303 => (1, &[6, 7, 8]),
            _ => (2, &[2]),
        };
        let mut spaces = self.ex.resources.iter()
            .filter(|p| types.contains(&p.res_type) && p.lower <= register && register <= p.upper)
            .map(|p| p.space);
        let space = spaces.next()?;
        if spaces.next().is_some() {
            return None;
        }
        Some((Some(kind), register, space))
    }

    fn uncovered_fssm_descriptor(&self, sgpr: u8, offset: u32) -> Option<(u32, u32, u32)> {
        if self.ex.stage != Stage::Compute {
            return None;
        }
        let resources: Vec<[u32; 4]> = self
            .ex
            .resources
            .iter()
            .map(|resource| [resource.res_type, resource.space, resource.lower, resource.upper])
            .collect();
        tuxbox_dxbc::compute_table::uncovered_descriptor(
            &self.ex.fssm,
            &resources,
            5,
            sgpr,
            offset,
            self.ex.root_sig.as_ref()?,
        )
    }

    /// Standard adjacent P1/P2 pairs consume the rasterizer's interpolant. A shader
    /// that reads the partial term or provides its own weights needs the vertex values.
    fn classify_interpolation(&mut self) {
        let mut smooth = BTreeSet::new();
        // A branch into P2 bypasses P1. After a back edge the input registers
        // may already contain values written later in the instruction stream.
        let writes_weight =
            |ins: &Insn| ins.mnemonic != "v_nop" && ins.dst.iter().any(|o| matches!(o, Opnd::Vgpr(0 | 1)));
        let targets: BTreeSet<_> = self
            .insns
            .iter()
            .filter(|i| i.enc == Enc::Sopp && Self::branch_cond(i.op).is_some())
            .map(|i| i.offset as i64 + 4 + 4 * i.aux.imm)
            .collect();
        let has_back_edge = self
            .insns
            .iter()
            .any(|i| i.enc == Enc::Sopp && Self::branch_cond(i.op).is_some() && i.aux.imm < 0);
        let mut original = [!(has_back_edge && self.insns.iter().any(writes_weight)); 2];
        let mut original_weights = !(has_back_edge && self.insns.iter().any(writes_weight));
        let mut i = 0;
        while i < self.insns.len() {
            let ins = &self.insns[i];
            if ins.enc == Enc::Vop3 && matches!(ins.mnemonic.as_str(), "v_interp_p1ll_f16" | "v_interp_p1lv_f16" | "v_interp_p2_f16") && ins.raw.len() > 1 {
                // The rasterizer's own weights give its interpolant; others need the vertex values.
                let own = if ins.mnemonic == "v_interp_p2_f16" { 1 } else { 0 };
                if ins.src.get(1) != Some(&Opnd::Vgpr(own)) || !original[usize::from(own)] {
                    self.pulled_attrs.insert(Self::f16_interp_descriptor(ins.raw[1]).0);
                }
            }
            if ins.enc == Enc::Vinterp {
                let attr = ins.aux.imm as usize;
                if ins.op == 2 && ins.raw[0] & 255 == 2 {
                    self.flat_attrs.insert(attr);
                } else {
                    smooth.insert(attr);
                    let paired = ins.op == 0
                        && ins.src == [Opnd::Vgpr(0)]
                        && original_weights
                        && self.insns.get(i + 1).is_some_and(|p| {
                            p.enc == Enc::Vinterp
                                && p.op == 1
                                && p.src == [Opnd::Vgpr(1)]
                                && !targets.contains(&(p.offset as i64))
                                && p.dst == ins.dst
                                && p.aux.imm == ins.aux.imm
                                && p.aux.imm2 == ins.aux.imm2
                                && ins.dst != [Opnd::Vgpr(1)]
                        });
                    if paired {
                        i += 1;
                    } else {
                        self.pulled_attrs.insert(attr);
                    }
                }
            }
            if writes_weight(ins) {
                original_weights = false;
                for (n, kept) in original.iter_mut().enumerate() {
                    *kept &= !ins.dst.contains(&Opnd::Vgpr(n as u8));
                }
            }
            i += 1;
        }
        self.pulled_attrs.extend(self.flat_attrs.intersection(&smooth));
    }

    /// Classify descriptor loads and bind them to HLSL registers.
    ///
    /// Code is not laid out in execution order (blocks are reached through backward
    /// branches), so the symbolic SGPR state is computed as a forward dataflow fixpoint over
    /// the control-flow graph: state = (known symbolic values, set of written SGPRs), join =
    /// keep only values every predecessor agrees on / union of written sets. A final pass
    /// then walks each block with its entry state and records the resources.
    fn resolve_resources(&mut self) -> Result<(), String> {
        type State = (HashMap<u16, Sym>, BTreeSet<u16>);
        let n = self.insns.len();
        if n == 0 {
            return Ok(());
        }
        let off2idx: HashMap<usize, usize> = self.insns.iter().enumerate().map(|(i, x)| (x.offset, i)).collect();
        // Block leaders and per-instruction branch targets.
        let mut leaders: BTreeSet<usize> = BTreeSet::new();
        leaders.insert(0);
        let mut tgt: HashMap<usize, usize> = HashMap::new();
        for (i, ins) in self.insns.iter().enumerate() {
            if ins.enc != Enc::Sopp {
                continue;
            }
            if Self::branch_cond(ins.op).is_some() {
                let t = ins.offset as i64 + 4 + ins.aux.imm * 4;
                if let Some(&ti) = off2idx.get(&(t as usize)) {
                    tgt.insert(i, ti);
                    leaders.insert(ti);
                }
                leaders.insert(i + 1);
            } else if ins.op == 1 {
                leaders.insert(i + 1);
            }
        }
        let starts: Vec<usize> = leaders.into_iter().filter(|&s| s < n).collect();
        let block_of = |i: usize| starts.partition_point(|&s| s <= i) - 1;
        let ranges: Vec<std::ops::Range<usize>> =
            (0..starts.len()).map(|b| starts[b]..starts.get(b + 1).copied().unwrap_or(n)).collect();
        let succs: Vec<Vec<usize>> = ranges
            .iter()
            .map(|r| {
                let last = r.end - 1;
                let ins = &self.insns[last];
                let mut v = Vec::new();
                let is_sopp = ins.enc == Enc::Sopp;
                if let Some(&t) = tgt.get(&last) {
                    v.push(block_of(t));
                }
                let falls = !(is_sopp && (ins.op == 1 || ins.op == 2));
                if falls && r.end < n {
                    v.push(block_of(r.end));
                }
                v
            })
            .collect();

        // Both passes read the same decoded program. Snapshot it once for all blocks.
        let program = self.insns.clone();
        // Direct branches have explicit graph edges. Preserve the previous
        // behavior for other control transfers, not modeled by this graph.
        let track_heap_origin = (self.resource_heap_base.is_some() || self.sampler_heap_base.is_some())
            && program.iter().enumerate().all(|(i, ins)| {
            if ins.enc == Enc::Sopp && Self::branch_cond(ins.op).is_some() {
                tgt.contains_key(&i)
            } else {
                !["branch", "setpc", "swappc", "call", "rfe", "trap"]
                    .iter().any(|name| ins.mnemonic.contains(name))
            }
        }) && n <= (usize::MAX >> 17);
        // Fixpoint (analysis only: recordings are rolled back afterwards).
        let saved = (
            self.res.clone(),
            self.cb_size.clone(),
            self.tex.clone(),
            self.samp.clone(),
            self.samp_cmp.clone(),
            self.uav.clone(),
            self.sbuf.clone(),
            self.tbuf.clone(),
            self.tbuf_uint.clone(),
        );
        let mut first_err: Option<String> = None;
        let mut states: Vec<Option<State>> = vec![None; ranges.len()];
        let mut entry = HashMap::new();
        if let Some(mesh) = &self.mesh_bindings {
            for &register in mesh.raw.keys() {
                entry.insert(register.into(), Sym::MeshRawAddress { register, word: 0 });
                entry.insert(u16::from(register) + 1, Sym::MeshRawAddress { register, word: 1 });
            }
        }
        if self.pixel_heap {
            lane_reads::entry(&mut entry);
        }
        states[0] = Some((entry, BTreeSet::new()));
        let mut work = vec![0usize];
        let mut budget = 20_000usize;
        while let Some(b) = work.pop() {
            if budget == 0 {
                return Err("descriptor dataflow did not converge".into());
            }
            budget -= 1;
            let (mut sym, mut written) = states[b].clone().unwrap();
            if let Err(e) = self.res_block(&program, &mut sym, &mut written, ranges[b].clone(), track_heap_origin) {
                if std::env::var_os("TB_DFDEBUG").is_some() {
                    eprintln!("block {b} analysis error: {e}");
                }
                // Unknown effect: forget everything and treat every SGPR as written. Keep
                // the first failure — it is the root cause if the recording pass fails too.
                first_err.get_or_insert(e);
                sym.clear();
                written = (0u16..=105).collect();
            }
            for &s in &succs[b] {
                let changed = match &mut states[s] {
                    slot @ None => {
                        let mut next = sym.clone();
                        if track_heap_origin {
                            heap_origins::canonicalize(&mut next, s);
                        }
                        *slot = Some((next, written.clone()));
                        true
                    }
                    Some((osym, owr)) => {
                        let before = (osym.clone(), owr.len());
                        let heap = track_heap_origin.then(|| heap_origins::intersection(osym, &sym, s));
                        let keys: Vec<u16> = osym.keys().copied().collect();
                        for k in keys {
                            let a = osym[&k];
                            if track_heap_origin && heap_origins::tracked(a) {
                                osym.remove(&k);
                                continue;
                            }
                            match sym.get(&k).copied() {
                                Some(b) if a == b => {}
                                Some(b) => match self.join_desc(a, b) {
                                    Some(j) => {
                                        osym.insert(k, j);
                                    }
                                    None => {
                                        osym.remove(&k);
                                    }
                                },
                                // Known on one path only. A descriptor stays (see below);
                                // anything else is dropped.
                                None if matches!(a, Sym::Desc { .. } | Sym::Multi { .. } | Sym::Table(_)) => {}
                                None => {
                                    osym.remove(&k);
                                }
                            }
                        }
                        // Optimistic join for descriptors: a register that holds a descriptor
                        // on one incoming path and nothing known on the other is treated as
                        // holding the descriptor. Compiled code loads descriptors inside
                        // blocks that are skipped only when no lane is active
                        // (`s_cbranch_execz`), then uses them after the join; on the skipped
                        // path the result is unused. A later use as a descriptor would be
                        // invalid anyway if the register held something else.
                        for (k, v) in sym.iter() {
                            if matches!(v, Sym::Desc { .. } | Sym::Multi { .. } | Sym::Table(_)) && !osym.contains_key(k) {
                                osym.insert(*k, *v);
                            }
                        }
                        if let Some(heap) = heap {
                            osym.extend(heap);
                        }
                        owr.extend(written.iter().copied());
                        before != (osym.clone(), owr.len())
                    }
                };
                if changed && !work.contains(&s) {
                    work.push(s);
                }
            }
        }
        (self.res, self.cb_size, self.tex, self.samp, self.samp_cmp, self.uav, self.sbuf, self.tbuf, self.tbuf_uint) = saved;

        // TB_DFDEBUG=<sgpr>: print that register's symbol at every block entry (diagnostics).
        if let Some(reg) = std::env::var("TB_DFDEBUG").ok().and_then(|v| v.parse::<u16>().ok()) {
            for (b, r) in ranges.iter().enumerate() {
                let s = states[b].as_ref().map(|(sym, wr)| (sym.get(&reg).copied(), wr.contains(&reg)));
                eprintln!("block {b} @{:#x}..: s{reg} = {s:?} succs={:?}", self.insns[r.start].offset, succs[b]);
            }
            eprintln!("first analysis error: {first_err:?}");
        }
        self.msel.clear();
        self.bsel.clear();
        self.dsel_loads.clear();
        self.dsel_runtime.clear();
        self.dsel_moves.clear();
        self.raw_byte_index.clear();
        // Recording pass with the converged entry states.
        for (b, r) in ranges.iter().enumerate() {
            if let Some((mut sym, mut written)) = states[b].clone() {
                self.res_block(&program, &mut sym, &mut written, r.clone(), track_heap_origin).map_err(|e| first_err.clone().unwrap_or(e))?;
            }
        }
        Ok(())
    }

    /// Element type of each typed buffer, inferred from how the shader uses the values it
    /// loads from (or produces for) the buffer. The buffer view's format is not part of the
    /// shader: the hardware delivers floats for float/normalised formats and integers for
    /// integer formats, and the compiled code simply goes on to treat the registers as one
    /// or the other. HLSL needs the element type declared, so it is recovered from the
    /// first typed uses: conversions from / comparisons as / arithmetic on integers or
    /// floats, and use as a memory index. Bitwise logic and moves are followed, not counted.
    /// Slots with no evidence, or more float than integer evidence, stay float.
    fn infer_tbuf_types(&mut self) {
        // 0 float, 1 uint, 2 sint
        fn class(mn: &str) -> Option<u8> {
            let m = mn.strip_suffix("_e64").unwrap_or(mn);
            if let Some(rest) = m.strip_prefix("v_cvt_") {
                let last = rest.rsplit('_').next().unwrap_or("");
                return Some(if last.starts_with('f') { 0 } else if last.starts_with('i') { 2 } else { 1 });
            }
            if m.starts_with("v_cmp") {
                // The operand type is the last component (`v_cmp_f_i32` is an integer compare
                // whose condition is "false").
                let ty = m.rsplit('_').next().unwrap_or("");
                return if ty.starts_with('f') { Some(0) } else if ty.starts_with('i') { Some(2) } else if ty.starts_with('u') { Some(1) } else { None };
            }
            const SINT: [&str; 7] = ["v_ashr", "v_max_i", "v_min_i", "v_bfe_i", "v_mul_i32", "v_mad_i32", "v_mul_hi_i"];
            const UINT: [&str; 14] = [
                "v_lshl", "v_lshr", "v_bfe_u", "v_mul_lo", "v_mul_hi_u", "v_mul_u32", "v_mad_u32", "v_add_nc", "v_sub_nc", "v_subrev_nc", "v_add_co", "v_sub_co",
                "v_max_u", "v_min_u",
            ];
            if SINT.iter().any(|p| m.starts_with(p)) {
                return Some(2);
            }
            if UINT.iter().any(|p| m.starts_with(p)) {
                return Some(1);
            }
            if m.starts_with("v_") && (m.contains("_f32") || m.contains("_f16") || m.contains("_f64")) {
                return Some(0);
            }
            None
        }
        // Bitwise logic with a small constant (`x & 1`, `x & 0xf`, `x ^ 1`) is integer work:
        // the masks applied to float bit patterns are sign / exponent / mantissa masks, all
        // far larger.
        fn small_mask(u: &Insn) -> bool {
            ["v_and_b32", "v_or_b32", "v_xor_b32"].iter().any(|p| u.mnemonic.starts_with(p))
                && u.src.iter().any(|o| match o {
                    Opnd::ConstInt(c) => *c > 0 && *c <= 64,
                    Opnd::Literal(l) => *l > 0 && *l <= 0xffff,
                    _ => false,
                })
        }
        fn follows(mn: &str) -> bool {
            ["v_mov_b32", "v_and_b32", "v_or_b32", "v_xor_b32", "v_cndmask_b32", "v_not_b32"].iter().any(|p| mn.starts_with(p))
        }
        // Keys are (is image, is_uav, slot): typed buffers and textures are told apart.
        let mut votes: BTreeMap<(bool, bool, u32), [u32; 3]> = BTreeMap::new();
        // (stored-to resource, loaded-from resource): a store of registers loaded from another
        // typed resource says both hold the same kind of element.
        let mut links: Vec<((bool, bool, u32), (bool, bool, u32))> = Vec::new();
        // Textures that are sampled or gathered hold filterable (float) data.
        let mut sampled: BTreeSet<u32> = BTreeSet::new();
        let mut uniform_uint = BTreeMap::new();
        let targets = uniform_texel::branch_targets(&self.insns);
        let n = self.insns.len();
        // The typed resource an instruction reads or writes, with the data registers.
        let typed = |em: &Self, i: usize| -> Option<((bool, bool, u32), BTreeSet<u8>, bool, bool)> {
            let ins = &em.insns[i];
            let Some(&Opnd::Vgpr(vd)) = ins.dst.first() else { return None };
            match em.res.get(&i).and_then(|v| v.first()).copied()? {
                Res::TBuf(slot, is_uav) => {
                    let comps = match ins.mnemonic.rsplit('_').next() {
                        Some("x") => 1u8,
                        Some("xy") => 2,
                        Some("xyz") => 3,
                        Some("xyzw") => 4,
                        _ => return None,
                    };
                    let registers = if ins.mnemonic.contains("_d16_") { comps.div_ceil(2) } else { comps };
                    let regs = (0..registers).map(|k| vd.wrapping_add(k)).collect();
                    Some(((false, is_uav, slot), regs, ins.mnemonic.starts_with("buffer_load"), ins.mnemonic.starts_with("buffer_store")))
                }
                r @ (Res::Tex(..) | Res::Uav(..)) => {
                    let (slot, is_uav) = match r {
                        Res::Tex(s, _) => (s, false),
                        Res::Uav(s, _) => (s, true),
                        _ => unreachable!(),
                    };
                    let regs = (0..image_data_registers(ins)).map(|k| vd.wrapping_add(k)).collect();
                    let load = matches!(ins.mnemonic.as_str(), "image_load" | "image_load_mip");
                    Some(((true, is_uav, slot), regs, load, ins.mnemonic == "image_store"))
                }
                _ => None,
            }
        };
        for i in 0..n {
            if let Some(Res::Tex(slot, _)) = self.res.get(&i).and_then(|v| v.first()).copied() {
                let m = &self.insns[i].mnemonic;
                if m.starts_with("image_sample") || m.starts_with("image_gather") {
                    sampled.insert(slot);
                }
            }
            let Some((key, regs, is_load, is_store)) = typed(self, i) else { continue };
            if is_load && key.0 && !key.1 {
                let packed = uniform_texel::packed_uint(&self.insns[i], &self.insns[i + 1..], &targets);
                uniform_uint.entry(key.2).and_modify(|all| *all &= packed).or_insert(packed);
            }
            let v = votes.entry(key).or_insert([0; 3]);
            if is_load {
                // Forward: how are the loaded registers (and plain copies / masks of them) used?
                let mut live = regs;
                for j in i + 1..n {
                    if live.is_empty() {
                        break;
                    }
                    let u = &self.insns[j];
                    // The scan is linear: past an unconditional branch (or the program end)
                    // the following code belongs to another path.
                    if u.enc == Enc::Sopp && (u.op == 1 || u.op == 2) {
                        break;
                    }
                    let uses = u.src.iter().any(|o| matches!(o, Opnd::Vgpr(r) if live.contains(r)));
                    let mem_index = matches!(u.enc, Enc::Mubuf | Enc::Mimg | Enc::Ds) && matches!(u.src.first(), Some(Opnd::Vgpr(r)) if live.contains(r));
                    if mem_index && u.enc == Enc::Mimg && (u.mnemonic.starts_with("image_sample") || u.mnemonic.starts_with("image_gather")) {
                        // Sampling coordinates are floats; only texel fetches index by integer.
                        v[0] += 1;
                    } else if mem_index && !(u.mnemonic.contains("store") && matches!(u.enc, Enc::Mubuf)) {
                        v[1] += 1;
                    } else if uses && !self.epilog_pack.contains_key(&j) {
                        // (The export tail packs whatever type the output has: no evidence.)
                        if let Some(c) = class(&u.mnemonic) {
                            v[c as usize] += 1;
                        } else if small_mask(u) {
                            v[1] += 1;
                        }
                    }
                    let carry = uses && follows(&u.mnemonic);
                    for d in &u.dst {
                        if let Opnd::Vgpr(r) = d {
                            if carry {
                                live.insert(*r);
                            } else if !matches!(u.enc, Enc::Mubuf | Enc::Ds | Enc::Exp) || u.mnemonic.contains("load") || u.mnemonic.contains("read") {
                                live.remove(r);
                            }
                        }
                    }
                }
            } else if is_store {
                // Backward: what produced each stored register?
                for r in regs {
                    for j in (0..i).rev() {
                        let d = &self.insns[j];
                        if d.dst.iter().any(|o| matches!(o, Opnd::Vgpr(x) if *x == r)) {
                            if let Some((k2, _, true, _)) = typed(self, j) {
                                links.push((key, k2));
                                break;
                            }
                            // A conversion's result type is its first type token.
                            let c = match d.mnemonic.strip_prefix("v_cvt_") {
                                Some(rest) => Some(if rest.starts_with('f') { 0 } else if rest.contains("i32") || rest.contains("i16") { 2 } else { 1 }),
                                None => class(&d.mnemonic),
                            };
                            if let Some(c) = c {
                                v[c as usize] += 1;
                            }
                            break;
                        }
                    }
                }
            }
        }
        let mut ty: BTreeMap<(bool, bool, u32), u8> = BTreeMap::new();
        for (k, v) in &votes {
            let ints = v[1] + v[2];
            if ints > v[0] {
                ty.insert(*k, if v[2] > 0 { 2 } else { 1 });
            }
        }
        for (slot, packed) in uniform_uint {
            let key = (true, false, slot);
            if packed && votes.get(&key) == Some(&[0; 3]) && !sampled.contains(&slot) {
                ty.insert(key, 1);
            }
        }
        for (to, from) in links {
            if votes.get(&to).map(|v| v.iter().all(|&x| x == 0)).unwrap_or(true) {
                if let Some(&t) = ty.get(&from) {
                    ty.insert(to, t);
                }
            }
        }
        // Typed loads through the heap, per instruction: a vertex program fetches its index
        // buffer this way. Every path must agree.
        self.heap_load_ty = (0..n)
            .filter(|&i| (self.ex.stage == Stage::Vertex || matches!(self.ex.stage, Stage::Pixel | Stage::Compute) && self.insns[i].enc == Enc::Mimg) && match self.res.get(&i).and_then(|v| v.first()) {
                Some(Res::HeapRaw(_)) => self.insns[i].mnemonic.starts_with("buffer_load_format_"),
                Some(Res::HeapTexture(..)) => matches!(self.insns[i].mnemonic.as_str(), "image_load" | "image_load_mip"),
                _ => false,
            })
            .filter_map(|i| {
                let Some(&Opnd::Vgpr(vd)) = self.insns[i].dst.first() else { return None };
                let mut seen = [false; 3];
                let at: HashMap<i64, usize> = self.insns.iter().enumerate().map(|(k, x)| (x.offset as i64, k)).collect();
                let channels = match self.insns[i].enc {
                    Enc::Mimg => image_data_registers(&self.insns[i]),
                    _ => self.insns[i].mnemonic.rsplit('_').next().map_or(4, str::len) as u8,
                };
                let mut work: Vec<(usize, u8)> = (0..channels).map(|k| (i + 1, vd.wrapping_add(k))).collect();
                let mut visited: BTreeSet<(usize, u8)> = work.iter().copied().collect();
                while let Some((j, reg)) = work.pop() {
                    let Some(u) = self.insns.get(j) else { continue };
                    let uses = u.src.iter().any(|o| *o == Opnd::Vgpr(reg));
                    let address = u.src.first() == Some(&Opnd::Vgpr(reg));
                    // The first typed use on a path decides it. Later ones can be reached
                    // through registers the compiler reused for other values.
                    if uses && u.mnemonic != "v_nop" {
                        let verdict = match class(&u.mnemonic) {
                            Some(c) => Some(usize::from(c)),
                            None if small_mask(u) || (address && matches!(u.enc, Enc::Mubuf | Enc::Mtbuf)) => Some(1),
                            None if follows(&u.mnemonic) => None,
                            None if u.mnemonic.ends_with("_u32") => Some(1),
                            // An export or an operation without a type proves nothing.
                            None => Some(0),
                        };
                        if let Some(c) = verdict {
                            seen[c] = true;
                            continue;
                        }
                    }
                    let mut next = vec![];
                    if u.enc == Enc::Sopp {
                        let target = at.get(&(u.offset as i64 + 4 + u.aux.imm * 4)).copied();
                        if Self::branch_cond(u.op).is_some() || u.op == 2 { next.extend(target); }
                        if u.op != 1 && u.op != 2 { next.push(j + 1); }
                    } else {
                        next.push(j + 1);
                    }
                    let mut regs = vec![];
                    let width = match u.enc {
                        Enc::Mimg => image_data_registers(u),
                        Enc::Mtbuf => (u.op as u8 & 3) + 1,
                        Enc::Mubuf => match u.mnemonic.rsplit('_').next() {
                            Some(x @ ("x" | "xy" | "xyz" | "xyzw")) => x.len() as u8,
                            Some("dwordx2") => 2,
                            Some("dwordx3") => 3,
                            Some("dwordx4") => 4,
                            _ => 1,
                        },
                        _ => 1,
                    };
                    let writes = u.mnemonic != "v_nop" && !u.mnemonic.contains("store")
                        && u.dst.iter().any(|o| matches!(o, Opnd::Vgpr(d) if (*d..d.saturating_add(width)).contains(&reg)));
                    if !writes { regs.push(reg); }
                    if uses && follows(&u.mnemonic) {
                        regs.extend(u.dst.iter().filter_map(|o| match o { Opnd::Vgpr(r) => Some(*r), _ => None }));
                    }
                    for k in next {
                        for &r in &regs {
                            if visited.insert((k, r)) { work.push((k, r)); }
                        }
                    }
                }
                Some((i, seen))
            })
            .collect();
        for ((is_img, is_uav, slot), t) in ty {
            if !is_img {
                self.tbuf_ty.insert((is_uav, slot), t);
            } else if is_uav {
                // Textures used with image atomics already have their own uint declaration.
                if !self.uav_uint.contains(&slot) {
                    self.img_ty.insert((true, slot), t);
                }
            } else if !sampled.contains(&slot) {
                self.img_ty.entry((false, slot)).or_insert(t);
            }
        }
    }

    /// Recognise the colour-export epilogue of a pixel shader.
    ///
    /// Every colour export in the title's pixel shaders has the same shape, whatever the
    /// HLSL output type:
    ///
    /// ```text
    /// s_nop ; s_nop ; v_cvt_pkrtz_f16_f32 vA, x, y ; v_cvt_pkrtz_f16_f32 vB, z, w ; exp mrtN vA, vB (compressed)
    /// ```
    ///
    /// including shaders whose output is `uint4` (the lighting-channel copy writes an R16_UINT
    /// target this way, where a float-to-half conversion of the integer would always give 0).
    /// The export format belongs to the render target, which the shader compiler does not
    /// know, so this tail can only be a default that the console's pipeline creation replaces
    /// per target format (the two leading no-ops are spare room for it). That replacement is
    /// inferred, not observed. What the shader itself computes is the four unpacked operands
    /// (x, y, z, w), so those are exported directly: as integers for an integer output
    /// element, as full floats otherwise (the render-target write then converts to the
    /// target's own format, as a per-format epilogue would).
    ///
    /// Records the pack instructions whose operands must be captured and, per export, which
    /// captures feed it. An export is left alone unless both packed operands come from pack
    /// instructions directly before it with nothing but no-ops in between and no branch
    /// landing inside the sequence.
    fn find_export_epilogs(&mut self) {
        let mut off2idx: HashMap<usize, usize> = HashMap::new();
        for (i, ins) in self.insns.iter().enumerate() {
            off2idx.insert(ins.offset, i);
        }
        let mut targets: BTreeSet<usize> = BTreeSet::new();
        for ins in &self.insns {
            if ins.enc == Enc::Sopp && Self::branch_cond(ins.op).is_some() {
                let t = ins.offset as i64 + 4 + ins.aux.imm * 4;
                if t >= 0 {
                    if let Some(&ti) = off2idx.get(&(t as usize)) {
                        targets.insert(ti);
                    }
                }
            }
        }
        let mut next_id = 0usize;
        for idx in 0..self.insns.len() {
            let ins = &self.insns[idx];
            if ins.enc != Enc::Exp || ins.aux.imm2 & 1 == 0 || ins.aux.imm > 7 {
                continue;
            }
            let (Some(&Opnd::Vgpr(r0)), Some(&Opnd::Vgpr(r1))) = (ins.src.first(), ins.src.get(1)) else { continue };
            let en = ins.aux.mask;
            let (need0, need1) = (en & 0x3 != 0, en & 0xc != 0);
            let (mut p0, mut p1) = (None, None);
            let mut first = idx;
            for j in (idx.saturating_sub(8)..idx).rev() {
                let u = &self.insns[j];
                if matches!(u.mnemonic.as_str(), "s_nop" | "s_waitcnt") {
                    continue;
                }
                if u.mnemonic != "v_cvt_pkrtz_f16_f32" {
                    break;
                }
                let Some(&Opnd::Vgpr(d)) = u.dst.first() else { break };
                if d == r0 && p0.is_none() {
                    p0 = Some(j);
                    first = j;
                }
                if d == r1 && p1.is_none() {
                    p1 = Some(j);
                    first = j;
                }
            }
            if (need0 && p0.is_none()) || (need1 && p1.is_none()) {
                continue;
            }
            if targets.range(first + 1..=idx).next().is_some() {
                continue;
            }
            let mut id_of = |em: &mut Self, p: Option<usize>, need: bool| -> usize {
                match p {
                    Some(j) if need => *em.epilog_pack.entry(j).or_insert_with(|| {
                        next_id += 1;
                        next_id - 1
                    }),
                    _ => usize::MAX,
                }
            };
            let a = id_of(self, p0, need0);
            let b = id_of(self, p1, need1);
            self.epilog_exp.insert(idx, (a, b));
        }
    }

    /// Shaders may read only one field of a padded instance or view record.
    /// Array indexing and byte-offset loads both need the complete native stride.
    fn resolve_structured_strides(&mut self) {
        let keys: Vec<_> = self.sbuf.keys().copied().collect();
        for (uav, slot) in keys {
            let kind = u32::from(uav);
            let space = self.binding_spaces.get(&(kind, slot)).copied().unwrap_or(0);
            let words = self.table_structured_stride(slot, space, uav).or_else(|| {
                if space != 0 || self.native_binding(uav, slot, space) != (slot, space) { return None; }
                fssm_bindings::stride_words(&self.ex.fssm, &self.ex.resources, self.ex.stage, slot, uav)
            });
            if let Some(words) = words {
                self.sbuf.insert((uav, slot), words);
                self.reflected_strides.insert((uav, slot));
            }
        }
    }

    /// The physical table slot can differ from the resource register in a mixed
    /// root. Require the FSSM offset, root range, and structured PSV binding to agree.
    fn table_structured_stride(&self, slot: u32, space: u32, uav: bool) -> Option<u32> {
        if let Some(mesh) = &self.mesh_bindings {
            return mesh.tables.values().find(|b| (b.kind, b.register, b.space) == (u32::from(uav), slot, space))?.stride_words;
        }
        let class = if uav { 0x303 } else { 0x300 };
        let slot = self.register_of(u32::from(uav), slot);
        let psv_type = if uav { 8 } else { 5 };
        // The used-resource rows and PSV keep the compiler's own numbering.
        let (native, native_space) = self.native_binding(uav, slot, space);
        if !self.ex.resources.iter().any(|r| (r.res_type, r.space, r.lower, r.upper) == (psv_type, native_space, native, native)) {
            return None;
        }
        let mut rows = tuxbox_dxbc::compute_table::records(&self.ex.fssm)?
            .filter(|&[c, _, r, _, loc]| c == class && r == native && self.table_locations().iter().any(|&(_, l)| l == loc));
        let [_, offset, _, stride, location] = rows.next()?;
        let &(sgpr, _) = self.table_locations().iter().find(|&&(_, l)| l == location)?;
        if rows.next().is_some() || offset % 8 != 0 || !(4..=2048).contains(&stride) || stride % 4 != 0 {
            return None;
        }
        let binding = self.table_binding_space(sgpr, offset.checked_mul(4)?, 32).ok()?;
        (binding == (Some(u32::from(uav)), slot, space)).then_some(stride / 4)
    }

    /// User SGPR and used-resource location of each descriptor table.
    fn table_locations(&self) -> &'static [(u8, u32)] {
        match self.ex.stage {
            Stage::Compute => &[(4, 0x404), (5, 0x10504)],
            Stage::Vertex | Stage::Pixel => &[(8, 0x804)],
            _ => &[],
        }
    }

    /// The register and space the shader's own metadata gives a resource bound at `slot`.
    /// An embedded root signature can number its ranges independently of the shader;
    /// the table position is what both sides share.
    fn native_binding(&self, uav: bool, slot: u32, space: u32) -> (u32, u32) {
        let class = if uav { 0x303 } else { 0x300 };
        let slot = self.register_of(u32::from(uav), slot);
        if self.mesh_bindings.is_some() || self.ex.root_sig.is_none() { return (slot, space); }
        let Some(rows) = tuxbox_dxbc::compute_table::records(&self.ex.fssm) else { return (slot, space) };
        let mut found = rows.filter(|&[c, _, _, _, _]| c == class).filter_map(|[_, offset, register, _, location]| {
            let &(sgpr, _) = self.table_locations().iter().find(|&&(_, l)| l == location)?;
            let bound = self.table_binding_space(sgpr, offset.checked_mul(4)?, 32).ok()?;
            (bound == (Some(u32::from(uav)), slot, space)).then_some(register)
        });
        let (Some(native), None) = (found.next(), found.next()) else { return (slot, space) };
        let types = if uav { 6..=8 } else { 3..=5 };
        let mut spaces = self.ex.resources.iter()
            .filter(|p| types.contains(&p.res_type) && p.lower <= native && native <= p.upper)
            .map(|p| p.space);
        match (spaces.next(), spaces.next()) {
            (Some(space), None) => (native, space),
            // The same register in two spaces: only an unchanged number is unambiguous.
            _ => (slot, space),
        }
    }

    /// Declared element size, in dwords, of a structured buffer.
    ///
    /// The stride is a property of the buffer view, not of the shader: on the console the
    /// hardware multiplies the element index by the descriptor's stride. D3D12 instead takes
    /// the stride from the HLSL element type (observed: a translation declaring a 12-byte
    /// element for the 16-byte primitive-data buffer read the wrong rows), so the declared
    /// element has to be the real one. Use a validated native stride when available. For
    /// shaders without one, the fallback uses observed access extents. What the PC build
    /// of the same game shows for 801 buffers bound at the same register: stride 16 for
    /// every buffer the Xbox code touches with extents of 8..28 bytes, 64 for extents of 64.
    /// Hence: extents of 8..28 bytes -> 16-byte elements (larger offsets address following
    /// elements), anything larger rounded up to a multiple of 16. A buffer only ever read or
    /// written one dword at offset 0 keeps a 4-byte element: ScatterCopyCS reads its index
    /// buffer that way and its view really has stride 4 (forcing 16 scrambled the per-object
    /// data it uploads); no 16-byte buffer in the comparison was touched like that.
    /// A heuristic (TECHNICAL_DEBT).
    fn sbuf_words(&self, key: (bool, u32)) -> u32 {
        let raw = self.sbuf.get(&key).copied().unwrap_or(1).max(1);
        if self.persistent_queue || self.micropoly_vote.is_some() || self.reflected_strides.contains(&key) {
            return raw;
        }
        // Unreal's instance-culling work item packs two uints per record. Both
        // the single-instance and general permutations index those words using
        // the descriptor's 8-byte stride. Rounding this SRV to a float4 skips
        // every other work item and leaves part of the visible-instance list unwritten.
        if raw == 2 && !key.0 && !self.sbuf_flat.contains(&key)
            && self.ex.entry.as_deref() == Some("InstanceCullBuildInstanceIdBufferCS")
        {
            return 2;
        }
        match raw {
            1 => 1,
            2..=7 => 4,
            _ => raw.div_ceil(4) * 4,
        }
    }

    /// Raw SRV behind a root descriptor whose GPU address arrives in user SGPR `sgpr`.
    fn root_raw_srv(&mut self, sgpr: u8) -> Option<Res> {
        let mut rows = tuxbox_dxbc::compute_table::records(&self.ex.fssm)?
            .filter(|&[class, _, _, _, loc]| class == 0x100 && (loc >> 8) & 0xff == u32::from(sgpr));
        let [_, _, register, _, _] = rows.next()?;
        if rows.next().is_some() {
            return None;
        }
        let mut raw = self.ex.resources.iter().filter(|p| p.res_type == 4 && p.lower == register && p.upper == register);
        let space = raw.next()?.space;
        if raw.next().is_some() {
            return None;
        }
        if self.binding_spaces.insert((0, register), space).is_some_and(|old| old != space) {
            return None;
        }
        self.raw_binding(register, false)
    }

    fn raw_binding(&mut self, slot: u32, is_uav: bool) -> Option<Res> {
        let ty = if is_uav { 7 } else { 4 };
        let space = self.binding_spaces.get(&(u32::from(is_uav), slot)).copied().unwrap_or(0);
        let (native, space) = self.native_binding(is_uav, slot, space);
        if self.ex.resources.iter().any(|p| p.res_type == ty && p.space == space && p.lower <= native && native <= p.upper) {
            self.raw.insert((is_uav, slot));
            Some(Res::Raw(slot, is_uav))
        } else {
            None
        }
    }

    /// Resource a MUBUF instruction addresses when its descriptor is `slot` of `table`
    /// (also records declaration requirements).
    fn mubuf_res(&mut self, ins: &Insn, offset: u32, table: u8) -> Result<Res, String> {
        let (kind, slot) = self.table_binding(table, offset, 32)?;
        let is_uav = match kind {
            Some(0) => false,
            Some(1) => true,
            Some(2) => {
                // Per-lane indexed read of a constant buffer (e.g. cbuffer arrays indexed by
                // a varying value): size the declaration to the maximum.
                self.cb_size.insert(slot, 4096 * 4);
                return Ok(Res::CbDyn(slot));
            }
            other => return Err(format!("mubuf at {:#x}: buffer in table of kind {other:?}", ins.offset)),
        };
        if let Some(raw) = self.raw_binding(slot, is_uav) {
            return Ok(raw);
        }
        if ins.mnemonic.contains("_atomic_") {
            if !is_uav {
                return Err(format!("mubuf at {:#x}: atomic on a non-UAV buffer", ins.offset));
            }
            // PSV0 tells typed (6) from structured (8) UAVs. Atomics need a uint element, so
            // a typed UAV used atomically is declared RWBuffer<uint>.
            let (native, space) = self.native_binding(true, slot, self.binding_spaces.get(&(1, slot)).copied().unwrap_or(0));
            let structured = self.ex.resources.iter().any(|p| p.res_type == 8 && p.space == space && p.lower <= native && native <= p.upper);
            if structured {
                let e = self.sbuf.entry((true, slot)).or_insert(0);
                *e = (*e).max(ins.aux.imm as u32 / 4 + 1);
                return Ok(Res::Buf(slot, true));
            }
            self.tbuf.insert((true, slot));
            self.tbuf_uint.insert(slot);
            return Ok(Res::TBuf(slot, true));
        }
        if ins.mnemonic.contains("_format_") {
            // Typed buffer (Buffer<T> / RWBuffer<T>), addressed by element index.
            self.tbuf.insert((is_uav, slot));
            return Ok(Res::TBuf(slot, is_uav));
        }
        let ndw = Self::mubuf_dwords(&ins.mnemonic).ok_or_else(|| format!("mubuf op {} unsupported", ins.mnemonic))?;
        let end = ins.aux.imm as u32 / 4 + ndw;
        let e = self.sbuf.entry((is_uav, slot)).or_insert(0);
        *e = (*e).max(end);
        Ok(Res::Buf(slot, is_uav))
    }

    /// Candidate descriptors `(table, offset)` of a symbol that is dword 0 of a descriptor.
    fn desc_candidates(&self, s: Option<&Sym>) -> Option<Vec<(u8, u32)>> {
        match s {
            Some(Sym::Desc { table, off, k: 0 }) => Some(vec![(*table, *off)]),
            Some(Sym::Multi { set, k: 0 }) => self.multi_sets.get(*set as usize).cloned(),
            _ => None,
        }
    }

    /// Join of two different descriptor symbols at a control-flow merge: the union of
    /// their candidates when both are the same dword of some descriptor, else nothing.
    fn join_desc(&mut self, a: Sym, b: Sym) -> Option<Sym> {
        let parts = |s: Sym, sets: &Vec<Vec<(u8, u32)>>| -> Option<(Vec<(u8, u32)>, u32)> {
            match s {
                Sym::Desc { table, off, k } => Some((vec![(table, off)], k)),
                Sym::Multi { set, k } => sets.get(set as usize).map(|v| (v.clone(), k)),
                _ => None,
            }
        };
        let (mut ca, ka) = parts(a, &self.multi_sets)?;
        let (cb, kb) = parts(b, &self.multi_sets)?;
        if ka != kb {
            return None;
        }
        ca.extend(cb);
        ca.sort();
        ca.dedup();
        if ca.len() > 16 {
            return None; // not a small "one of N" selection
        }
        let set = match self.multi_sets.iter().position(|v| *v == ca) {
            Some(i) => i,
            None => {
                self.multi_sets.push(ca);
                self.multi_sets.len() - 1
            }
        };
        Some(Sym::Multi { set: set as u16, k: ka })
    }

    // Some Xbox vertex fetch programs assemble a raw buffer descriptor from
    // a root GPU address, a byte stride of one and an unbounded record count.
    // Recognize the complete four-word construction and require a unique SRV.
    fn direct_raw_descriptor(&mut self, instructions: &[Insn], written: &BTreeSet<u16>) -> Option<(u16, u32)> {
        let [format, size, low, high] = instructions else { return None; };
        if [format, size, low].iter().any(|op| op.mnemonic != "s_mov_b32")
            || format.src != [Opnd::Literal(0x11014fac)]
            || size.src != [Opnd::ConstInt(-1)] { return None; }
        let [Opnd::Sgpr(base)] = low.dst[..] else { return None; };
        let [Opnd::Sgpr(root)] = low.src[..] else { return None; };
        if base > 124 || root > 126 || written.contains(&u16::from(root)) || written.contains(&(u16::from(root)+1))
            || format.dst != [Opnd::Sgpr(base+3)] || size.dst != [Opnd::Sgpr(base+2)]
            || high.dst != [Opnd::Sgpr(base+1)] || !raw_index::sets_byte_stride(high, root+1) { return None; }
        let [binding] = &self.ex.resources[..] else { return None; };
        if binding.res_type != 4 || binding.lower != binding.upper { return None; }
        let slot = binding.lower;
        self.binding_spaces.insert((0, slot), binding.space);
        self.raw.insert((false,slot));
        Some((u16::from(base),slot))
    }

    fn mesh_raw_descriptor(&mut self, sym: &HashMap<u16, Sym>, base: u16) -> Result<Option<Res>, String> {
        let Some(Sym::MeshRawAddress { register, word: 0 }) = sym.get(&base).copied() else {
            return Ok(None);
        };
        if sym.get(&(base + 1)) != Some(&Sym::MeshRawAddress { register, word: 2 })
            || sym.get(&(base + 2)) != Some(&Sym::InlineWord(u32::MAX))
            || sym.get(&(base + 3)) != Some(&Sym::InlineWord(0x11014fac))
        {
            return Err(format!("unverified mesh root SRV descriptor at s{base}"));
        }
        let (slot, space) = self.mesh_bindings.as_ref().and_then(|mesh| mesh.raw.get(&register)).copied()
            .ok_or("mesh root SRV missing from signature")?;
        if self.binding_spaces.insert((0, slot), space).is_some_and(|previous| previous != space) {
            return Err(format!("mesh SRV t{slot} is used in multiple spaces"));
        }
        self.raw.insert((false, slot));
        Ok(Some(Res::Raw(slot, false)))
    }

    /// Transfer function for one basic block (also records resources / reports errors).
    fn res_block(
        &mut self,
        insns: &[Insn],
        sym: &mut HashMap<u16, Sym>,
        written: &mut BTreeSet<u16>,
        range: std::ops::Range<usize>,
        track_heap_origin: bool,
    ) -> Result<(), String> {
        let dbg_reg = std::env::var("TB_DFDEBUG").ok().and_then(|v| v.parse::<u16>().ok());
        let mut dbg_prev: Option<Sym> = dbg_reg.and_then(|r| sym.get(&r).copied());
        let block_start = range.start;
        let inline_samplers = self.resource_heap_base.is_some()
            || (self.ex.root_sig.is_none() && self.ex.resources.iter().any(|r| r.res_type == 1))
            // Literal sampler words can name any static sampler with identical state.
            || self.ex.root_sig.as_ref().is_some_and(|r| !r.static_samplers.is_empty())
            || (self.ex.stage == Stage::Compute && self.ex.root_sig.as_ref().is_some_and(|r| !r.static_samplers.is_empty())
                && self.ex.resources.iter().filter(|r| r.res_type == 1).count() > 1);
        for idx in range {
            if let Some(r) = dbg_reg {
                let cur = sym.get(&r).copied();
                if cur != dbg_prev && idx > 0 {
                    eprintln!("  s{r}: {dbg_prev:?} -> {cur:?} by {:#x} {}", insns[idx - 1].offset, insns[idx - 1].mnemonic);
                }
                dbg_prev = cur;
            }
            let ins = &insns[idx];
            let heap_origin = track_heap_origin.then_some(idx);
            // Read reaching sources before invalidating every result word.
            // In-place copies/selects must see the old value, while multiword
            // image/buffer results must not leave stale offset proofs behind.
            let heap_value = heap_offsets::transfer(ins, sym, self.compute_heap, self.pixel_heap);
            // Judged before the instruction's own writes: a load may overwrite its SOFFSET.
            if self.pixel_heap {
                lane_reads::step(ins, sym, self.one_word_vector_mask());
            }
            // Dataflow key of a scalar location: SGPR number, or 1000+ for a lane-spill slot.
            let sg = |o: &Opnd| match o {
                Opnd::Sgpr(n) => Some(*n as u16),
                Opnd::Temp(n) if *n >= LANE_TEMP_BASE => Some(1000 + (*n - LANE_TEMP_BASE)),
                _ => None,
            };
            if self.mesh_bindings.is_some() && ins.mnemonic != "v_nop" {
                for (i, source) in ins.src.iter().enumerate() {
                    let Some(Sym::MeshRawAddress { word, .. }) = sg(source).and_then(|s| sym.get(&s)) else {
                        continue;
                    };
                    let allowed = ins.mnemonic == "s_mov_b32"
                        || (i == 2 && matches!(ins.mnemonic.as_str(), "v_mbcnt_lo_u32_b32" | "v_mbcnt_hi_u32_b32"))
                        // VOP3 uses a spare SRC2 field for these binary operations.
                        || (ins.enc == Enc::Vop3 && i == 2 && *source == Opnd::Sgpr(0)
                            && (ins.mnemonic.starts_with("v_cmp") || matches!(ins.mnemonic.as_str(),
                                "v_sub_f32" | "v_subrev_f32" | "v_add_f32" | "v_mul_f32" | "v_mul_legacy_f32" | "v_max_f32" | "v_min_f32"
                                | "v_mul_hi_u32" | "v_mul_lo_u32" | "v_mul_hi_i32" | "v_mul_u32_u24" | "v_ldexp_f32"
                                | "v_lshlrev_b32" | "v_lshrrev_b32" | "v_ashrrev_i32" | "v_and_b32" | "v_or_b32" | "v_xor_b32"
                                | "v_add_nc_u32" | "v_sub_nc_u32" | "v_subrev_nc_u32" | "v_max_u32" | "v_min_u32" | "v_max_i32" | "v_min_i32"
                                | "v_bfm_b32" | "v_bcnt_u32_b32" | "v_cvt_pkrtz_f16_f32" | "v_cvt_pknorm_i16_f32" | "v_cvt_pknorm_u16_f32")))
                        || (ins.mnemonic == "s_or_b32" && i == 0 && *word == 1
                            && ins.src.get(1) == Some(&Opnd::Literal(65536)))
                        || (ins.enc == Enc::Smem && ins.op >= 8 && i == 0 && *word == 0)
                        || (matches!(ins.enc, Enc::Mubuf | Enc::Mtbuf) && i == 1 && *word == 0);
                    if !allowed {
                        return Err(format!("mesh root address used as shader data at {:#x}", ins.offset));
                    }
                }
            }
            if idx >= block_start + 3 && ins.mnemonic == "s_or_b32" {
              if let Some((base, slot)) = self.direct_raw_descriptor(&insns[idx-3..=idx], written) {
                sym.insert(base, Sym::DirectRaw(slot));
                sym.remove(&(base + 1));
                written.insert(base + 1);
                continue;
              }
            }
            match ins.enc {
                Enc::Sop1 | Enc::Sop2
                    if self.mesh_bindings.is_some()
                        && matches!(ins.mnemonic.as_str(), "s_bitset1_b32" | "s_or_b32")
                        && ins.src.get(if ins.mnemonic == "s_or_b32" { 1 } else { 0 })
                            == Some(&if ins.mnemonic == "s_or_b32" { Opnd::Literal(65536) } else { Opnd::ConstInt(16) }) =>
                {
                    if let Some(d) = ins.dst.first().and_then(sg) {
                        let source = if ins.mnemonic == "s_or_b32" { ins.src.first().and_then(sg) } else { Some(d) };
                        let high = source.and_then(|s| sym.get(&s)).copied();
                        sym.remove(&d);
                        if let Some(Sym::MeshRawAddress { register, word: 1 }) = high {
                            sym.insert(d, Sym::MeshRawAddress { register, word: 2 });
                        }
                        written.insert(d);
                    }
                }
                Enc::Sop2 if matches!(ins.mnemonic.as_str(), "s_xor_b32" | "s_or_b32" | "s_and_b32")
                    && ins.src.len() == 2 && ins.src.iter().any(|o| matches!(o, Opnd::ConstInt(_) | Opnd::Literal(_)))
                    && ins.src.iter().any(|o| matches!(sg(o).and_then(|s| sym.get(&s)), Some(Sym::HeapOffset | Sym::HeapSamplerOffset))) =>
                {
                    if let Some(d) = ins.dst.first().and_then(sg) {
                        let known = |o: &Opnd| sg(o).and_then(|s| sym.get(&s).copied());
                        let literal = |o: &Opnd| match o {
                            Opnd::ConstInt(v) => Some(*v as u32),
                            Opnd::Literal(v) => Some(*v),
                            _ => None,
                        };
                        let result = [(0, 1), (1, 0)].into_iter().find_map(|(a,b)| {
                            let kind = known(&ins.src[a])?;
                            let mask = match kind { Sym::HeapOffset => 31, Sym::HeapSamplerOffset => 15, _ => return None };
                            let value = literal(&ins.src[b])?;
                            (ins.mnemonic == "s_and_b32" || value & mask == 0).then_some(kind)
                        });
                        // The ordinary scalar write invalidates all other facts.
                        sym.remove(&d);
                        written.insert(d);
                        if let Some(kind) = result { sym.insert(d, kind); }
                    }
                }
                Enc::Sop2 if self.resource_heap_base.is_some() && ins.mnemonic == "s_lshl_b32" && ins.src.get(1) == Some(&Opnd::ConstInt(5)) => {
                    if let Some(d) = ins.dst.first().and_then(sg) {
                        sym.insert(d, Sym::HeapOffset);
                        written.insert(d);
                    }
                }
                Enc::Sop1 if inline_samplers && ins.mnemonic == "s_brev_b32" && matches!(ins.src.first(), Some(Opnd::ConstInt(_) | Opnd::Literal(_))) => {
                    if let Some(d) = ins.dst.first().and_then(sg) {
                        let word = match ins.src[0] { Opnd::ConstInt(v) => v as u32, Opnd::Literal(v) => v, _ => unreachable!() };
                        sym.insert(d, Sym::InlineWord(word.reverse_bits()));
                        written.insert(d);
                    }
                }
                Enc::Sopk if inline_samplers && ins.mnemonic == "s_movk_i32" => {
                    if let Some(d) = ins.dst.first().and_then(sg) {
                        sym.insert(d, Sym::InlineWord(ins.aux.imm as i16 as i32 as u32));
                        written.insert(d);
                    }
                }
                Enc::Sop2 if ins.mnemonic == "s_lshl_b32" && ins.src.get(1) == Some(&Opnd::ConstInt(4)) && ins.src.first().and_then(sg).is_some() => {
                    if let (Some(d), Some(source)) = (ins.dst.first().and_then(sg), ins.src.first().and_then(sg)) {
                        let entry = match sym.get(&source) {
                            Some(Sym::Table(t)) => Some(*t),
                            None if !written.contains(&source) => u8::try_from(source).ok(),
                            _ => None,
                        };
                        if let Some(index) = entry.and_then(|n| sampler_heap::index(self.ex, n)) {
                            sym.insert(d, Sym::SamplerOffset(index));
                        } else if self.pixel_heap {
                            sym.insert(d, Sym::HeapSamplerOffset);
                        } else { sym.remove(&d); }
                        written.insert(d);
                    }
                }
                Enc::Sop1 if ins.mnemonic == "s_mov_b32" => {
                    if let Some(d) = ins.dst.first().and_then(sg) {
                        if inline_samplers || self.mesh_bindings.is_some() {
                            let literal = match ins.src.first() {
                                Some(Opnd::ConstInt(i)) => Some(*i as u32),
                                Some(Opnd::Literal(v)) => Some(*v),
                                _ => None,
                            };
                            if let Some(v) = literal {
                                sym.insert(d, Sym::InlineWord(v));
                                written.insert(d);
                                continue;
                            }
                        }
                        match ins.src.first().and_then(sg).and_then(|s| sym.get(&s).cloned()) {
                            Some(v) => {
                                sym.insert(d, v);
                            }
                            None => {
                                // A never-written user SGPR copied before use is still a table pointer.
                                let s = ins.src.first().and_then(sg);
                                match s {
                                    Some(s) if !written.contains(&s) => {
                                        sym.insert(d, Sym::Table(s as u8));
                                    }
                                    _ => {
                                        sym.remove(&d);
                                    }
                                }
                            }
                        }
                        written.insert(d);
                        if let Some(s) = ins.src.first().and_then(sg) {
                            if matches!(sym.get(&d), Some(Sym::Desc { k: 0, .. }) | Some(Sym::Multi { k: 0, .. })) {
                                self.dsel_moves.insert(idx, (d, s));
                            }
                        }
                    }
                }
                Enc::Sop1 if ins.mnemonic == "s_mov_b64" => {
                    // 64-bit moves copy descriptors two dwords at a time
                    // (`s_mov_b64 s44, s84` + `s_mov_b64 s46, s86` moves a 4-dword descriptor).
                    if let Some(d) = ins.dst.first().and_then(sg) {
                        let s = ins.src.first().and_then(sg);
                        for k in 0..2u16 {
                            match s.and_then(|s| sym.get(&(s + k)).cloned()) {
                                Some(v) => {
                                    if k == 0 && matches!(v, Sym::Desc { k: 0, .. } | Sym::Multi { k: 0, .. }) {
                                        self.dsel_moves.insert(idx, (d, s.unwrap_or(d)));
                                    }
                                    sym.insert(d + k, v);
                                }
                                None => match s {
                                    // A never-written user SGPR is a root table pointer (see
                                    // s_mov_b32); a 64-bit copy carries it in the low half.
                                    Some(s) if k == 0 && !written.contains(&s) => {
                                        sym.insert(d, Sym::Table(s as u8));
                                    }
                                    _ => {
                                        sym.remove(&(d + k));
                                    }
                                },
                            }
                            written.insert(d + k);
                        }
                    }
                }
                // s_setreg_* program a hardware mode register; the encoding's register
                // field is not an SGPR destination.
                Enc::Sopk if ins.mnemonic.starts_with("s_setreg") => {}
                // A code pointer taken through a VGPR: `v = index*4 + sCode` followed by
                // `v_readfirstlane sBase, v` and `s_load [sBase]` is a constant-table lookup
                // with a run-time index (ColorDeficiencyMain, cc45bda725a0f774). VGPRs are
                // tracked under keys 2000+ for exactly this.
                Enc::Vop1 | Enc::Vop2 | Enc::Vop3 if heap_value.is_some() => {
                    if let Some(Opnd::Vgpr(v)) = ins.dst.first() {
                        sym.insert(2000 + *v as u16, heap_value.unwrap());
                    }
                }
                Enc::Vop2 | Enc::Vop3
                    if matches!(ins.mnemonic.as_str(), "v_add_co_u32" | "v_add_nc_u32")
                        && ins.src.iter().any(|o| sg(o).map(|s| sym.get(&s) == Some(&Sym::Code)).unwrap_or(false)) =>
                {
                    if let Some(Opnd::Vgpr(v)) = ins.dst.first() {
                        sym.insert(2000 + *v as u16, Sym::Code);
                    }
                }
                Enc::Vop1 if ins.mnemonic == "v_readfirstlane_b32" => {
                    let code = matches!(ins.src.first(), Some(Opnd::Vgpr(v)) if sym.get(&(2000 + *v as u16)) == Some(&Sym::Code));
                    let heap = match ins.src.first() {
                        Some(Opnd::Vgpr(v)) => sym.get(&(2000 + *v as u16)).copied()
                            .filter(|s| matches!(s, Sym::HeapOffset | Sym::HeapSamplerOffset)),
                        _ => None,
                    };
                    if let Some(d) = ins.dst.first().and_then(sg) {
                        if let Some(heap) = heap {
                            // readfirstlane copies one lane's byte offset into an SGPR.
                            // Later uses index ResourceDescriptorHeap[sN]: the SGPR is
                            // uniform until something writes it, which rebinds the handle.
                            sym.insert(d, heap);
                        } else if code {
                            sym.insert(d, Sym::Code);
                        } else {
                            sym.remove(&d);
                        }
                        written.insert(d);
                    }
                }
                Enc::Sop1 if ins.mnemonic == "s_getpc_b64" => {
                    if let Some(d) = ins.dst.first().and_then(sg) {
                        sym.insert(d, Sym::Code);
                        sym.remove(&(d + 1));
                        written.insert(d);
                        written.insert(d + 1);
                    }
                }
                Enc::Sop2 if matches!(ins.mnemonic.as_str(), "s_add_u32" | "s_add_i32") => {
                    // Pointer arithmetic keeps a code pointer a code pointer.
                    let is_code = ins.src.iter().any(|o| sg(o).map(|s| sym.get(&s) == Some(&Sym::Code)).unwrap_or(false));
                    if let Some(d) = ins.dst.first().and_then(sg) {
                        if is_code {
                            sym.insert(d, Sym::Code);
                        } else {
                            sym.remove(&d);
                        }
                        written.insert(d);
                    }
                }
                Enc::Smem if ins.op <= 4 && ins.src.first().and_then(sg).map(|b| sym.get(&b) == Some(&Sym::Code)).unwrap_or(false) => {
                    // Read from an embedded constant table.
                    let n = 1u32 << ins.op;
                    let d = ins.dst.first().and_then(sg).ok_or("s_load: bad dst")?;
                    if self.static_smem_offset(ins).is_none()
                        && self.dynamic_code_offset(idx).is_none() {
                        return Err(format!("s_load at {:#x}: code-table read with dynamic soffset unsupported", ins.offset));
                    }
                    self.uses_code = true;
                    self.res.insert(idx, vec![Res::CodeTbl]);
                    for k in 0..n {
                        sym.remove(&(d + k as u16));
                        written.insert(d + k as u16);
                    }
                }
                Enc::Smem if ins.op <= 4 => {
                    let n = 1u32 << ins.op;
                    let b = ins.src.first().and_then(sg).ok_or("s_load: bad base")?;
                    let d = ins.dst.first().and_then(sg).ok_or("s_load: bad dst")?;
                    let table = match sym.get(&b) {
                        Some(Sym::Table(t)) => *t,
                        None if !written.contains(&b) => b as u8,
                        other => {
                            return Err(format!(
                                "s_load at {:#x}: base s{b} is not a root table pointer ({other:?})",
                                ins.offset
                            ))
                        }
                    };
                    if (ins.raw.get(1).copied().unwrap_or(0) >> 25) != 0x7d {
                        let soff = (ins.raw[1] >> 25) as u16;
                        // A pixel wave includes helper lanes, which host wave intrinsics skip:
                        // an index read from one lane is not the guest's there.
                        // In compute the heap pointer itself is verified (the one FSSM class-5
                        // user record), so any descriptor-sized load from it at a run-time
                        // SOFFSET is a heap fetch, however the byte offset was computed.
                        // Nothing tells a constant buffer from a raw buffer in the heap.
                        if self.pixel_heap && n == 4 && self.resource_heap_base == Some(table) {
                            return Err(format!("s_load at {:#x}: heap buffer in a pixel shader", ins.offset));
                        }
                        let heap_offset = sym.get(&soff) == Some(&Sym::HeapOffset)
                            || (self.compute_heap && !self.pixel_heap && soff < 106 && written.contains(&soff));
                        if self.resource_heap_base == Some(table) && matches!(n, 4 | 8)
                            && (ins.aux.imm == 0 || (ins.aux.imm >= 0 && ins.aux.imm % 32 == 0 && sym.get(&soff) == Some(&Sym::HeapOffset)))
                            && heap_offset {
                            for k in 0..n as u16 {
                                sym.insert(d + k, Sym::HeapWord(k as u8, heap_origin));
                                written.insert(d + k);
                            }
                            sym.insert(d, Sym::HeapDescriptor(n, heap_origin));
                            self.res.insert(idx, vec![Res::HeapLoad(soff)]);
                            continue;
                        }
                        // A sampler picked at run time from the directly indexed compute sampler
                        // heap (CS_RaytracingResolve: per-material index << 4, then a waterfall).
                        // Statically resolved sampler offsets keep their own path below.
                        if self.sampler_heap_base == Some(table) && n == 4 && ins.aux.imm == 0
                            && soff < 106 && written.contains(&soff)
                            && (!self.pixel_heap || sym.get(&soff) == Some(&Sym::HeapSamplerOffset))
                            && !matches!(sym.get(&soff), Some(Sym::SamplerOffset(_)))
                        {
                            for k in 0..4 {
                                sym.insert(d + k, Sym::HeapWord(k as u8, heap_origin));
                                written.insert(d + k);
                            }
                            sym.insert(d, Sym::HeapSamplerIndex(heap_origin));
                            self.res.insert(idx, vec![Res::HeapSamplerLoad(soff)]);
                            continue;
                        }
                        if let Some(Sym::SamplerOffset(index)) = sym.get(&soff).copied() {
                            // Four dwords per sampler, on the heap pointer following
                            // s8 (table) and its immediately following root constants.
                            if u32::from(table) == 9 + index.count && n == 4 && ins.aux.imm == 0 {
                                for k in 0..4 {
                                    sym.remove(&(d+k));
                                    written.insert(d+k);
                                }
                                sym.insert(d, Sym::HeapSampler(index));
                                continue;
                            }
                        }
                        // Preserve heap-specific lowering above. Only fold an ordinary
                        // descriptor-table offset when every reaching path proves its value.
                    }
                    let off = if let Some(off) = self.static_smem_offset(ins) {
                        off
                    } else if let Some((reg, imm, candidates)) = self.bounded_cbv_offsets(idx, table, b, d) {
                        let set = match self.multi_sets.iter().position(|v| *v == candidates) {
                            Some(set) => set,
                            None => {
                                self.multi_sets.push(candidates);
                                self.multi_sets.len() - 1
                            }
                        };
                        let set = u16::try_from(set).map_err(|_| "too many descriptor sets")?;
                        for k in 0..n {
                            sym.insert(d + k as u16, Sym::Multi { set, k });
                            written.insert(d + k as u16);
                        }
                        self.dsel_runtime.insert(idx, (d, table, reg, imm));
                        continue;
                    } else {
                        return Err(format!("s_load at {:#x}: dynamic soffset unsupported", ins.offset));
                    };
                    if self.mesh_bindings.as_ref().is_some_and(|mesh| {
                        self.sampler_heap_base != Some(table)
                            && (!matches!(n, 4 | 8) || !mesh.tables.contains_key(&(table, off)))
                    }) {
                        return Err(format!("unverified mesh descriptor load at {:#x}", ins.offset));
                    }
                    for k in 0..n {
                        sym.insert(d + k as u16, Sym::Desc { table, off, k });
                        self.dsel_loads.insert(idx, (d, ((table as u32) << 24) | off));
                        written.insert(d + k as u16);
                    }
                }
                Enc::Smem => {
                    let n = 1u32 << (ins.op - 8);
                    let b = ins.src.first().and_then(sg).ok_or("s_buffer_load: bad base")?;
                    let d = ins.dst.first().and_then(sreg_num).map(u16::from).ok_or("s_buffer_load: bad dst")?;
                    if let Some(raw) = self.mesh_raw_descriptor(sym, b)? {
                        self.res.insert(idx, vec![raw]);
                        for k in d..d+n as u16 { sym.remove(&k); written.insert(k); }
                        continue;
                    }
                    if heap_descriptor(sym, b, 4) {
                        self.res.insert(idx, vec![Res::HeapBuffer(b)]);
                        for k in d..d+n as u16 { sym.remove(&k); written.insert(k); }
                        continue;
                    }
                    if let Some(Sym::DirectRaw(slot)) = sym.get(&b) {
                        self.res.insert(idx, vec![Res::Raw(*slot, false)]);
                        for k in d..d+n as u16 { sym.remove(&k); written.insert(k); }
                        continue;
                    }
                    // A V# built inline from a root descriptor's GPU address (FSSM class 0x100
                    // row at that user SGPR, e.g. VS_LinearDepth's s[11:12]) reads the raw SRV
                    // that PSV declares at the row's register.
                    if let Some(Sym::Table(t)) = sym.get(&b).copied() {
                        if let Some(raw) = self.root_raw_srv(t) {
                            self.res.insert(idx, vec![raw]);
                            for k in d..d+n as u16 { sym.remove(&k); written.insert(k); }
                            continue;
                        }
                    }
                    // CBV descriptors sit on a 32-byte stride (cb1 is loaded from table offset
                    // 0x20 in MainOnePassPointLightPS).
                    let cands = self.desc_candidates(sym.get(&b)).ok_or_else(|| {
                        format!("s_buffer_load at {:#x}: base s{b} is not a buffer descriptor ({:?})", ins.offset, sym.get(&b))
                    })?;
                    // A dynamic SOFFSET register indexes the buffer at run time: size the
                    // cbuffer declaration to the D3D12 maximum (64 KiB = 4096 vec4).
                    let dynamic = (ins.raw.get(1).copied().unwrap_or(0) >> 25) != 0x7d;
                    let end = if dynamic { 4096 * 4 } else { ins.aux.imm as u32 / 4 + n };
                    let mut out: Vec<(u32, Res)> = Vec::new();
                    // A scalar load reads whatever buffer the descriptor names. From the CBV
                    // table that is a constant buffer; from the SRV / UAV table it is a
                    // (structured) buffer addressed by byte offset, e.g. the sky-light
                    // spherical-harmonics buffer read by ReflectionEnvironmentSkyLighting.
                    // Only the unambiguous case is handled; with several candidate
                    // descriptors the load is still treated as a constant buffer (debt).
                    for (table, off) in &cands {
                        let (kind, slot) = self.table_binding(*table, *off, 32)?;
                        let buffer_kind = kind.filter(|k| *k <= 1 && cands.len() == 1);
                        if let Some(k) = buffer_kind {
                            if let Some(raw) = self.raw_binding(slot, k == 1) {
                                out.push((((*table as u32) << 24) | off, raw));
                                continue;
                            }
                            let key = (k == 1, slot);
                            let e = self.sbuf.entry(key).or_insert(0);
                            *e = (*e).max(4);
                            self.sbuf_flat.insert(key);
                            out.push((((*table as u32) << 24) | off, Res::Buf(slot, k == 1)));
                            continue;
                        }
                        let e = self.cb_size.entry(slot).or_insert(0);
                        *e = (*e).max(end);
                        out.push((((*table as u32) << 24) | off, Res::Cb(slot)));
                    }
                    self.res.insert(idx, vec![out[0].1]);
                    if out.len() > 1 {
                        self.bsel.insert(idx, (b, out));
                    }
                    for k in 0..n {
                        sym.remove(&(d + k as u16));
                        written.insert(d + k as u16);
                    }
                }
                Enc::Mtbuf => {
                    let rs = ins.src.get(1).and_then(sg).ok_or("mtbuf: bad srsrc")?;
                    if self.compute_heap && heap_descriptor(sym, rs, 4) {
                        self.res.insert(idx, vec![Res::HeapRaw(rs)]);
                        continue;
                    }
                    let root_raw = match sym.get(&rs).copied() {
                        Some(Sym::Table(t)) if self.mesh_bindings.is_none() => self.root_raw_srv(t),
                        _ => None,
                    };
                    let raw = match root_raw {
                        Some(raw) => raw,
                        None => self.mesh_raw_descriptor(sym, rs)?
                            .ok_or("typed buffer load requires a verified linear root SRV")?,
                    };
                    self.res.insert(idx, vec![raw]);
                }
                // buffer_gl0_inv / buffer_gl1_inv (MUBUF 0x71 / 0x72) have no resource operands.
                Enc::Mubuf if ins.op == 0x71 || ins.op == 0x72 => {}
                Enc::Mubuf => {
                    let rs = ins.src.get(1).and_then(sg).ok_or("mubuf: bad srsrc")?;
                    // Never retain a provisional proof from another CFG visit.
                    self.raw_byte_index.remove(&ins.offset);
                    if let Some(raw) = self.mesh_raw_descriptor(sym, rs)? {
                        self.raw_byte_index.insert(ins.offset);
                        self.res.insert(idx, vec![raw]);
                        continue;
                    }
                    if let Some(Sym::DirectRaw(slot)) = sym.get(&rs) {
                        if raw_index::local_byte_descriptor(&insns[block_start..idx], rs) {
                            self.raw_byte_index.insert(ins.offset);
                        }
                        self.res.insert(idx, vec![Res::Raw(*slot, false)]);
                        continue;
                    }
                    if self.compute_heap && heap_descriptor(sym, rs, 4) {
                        self.res.insert(idx, vec![Res::HeapRaw(rs)]);
                        continue;
                    }
                    if let Some(Sym::Table(t)) = sym.get(&rs).copied() {
                        if let Some(raw) = self.root_raw_srv(t) {
                            self.res.insert(idx, vec![raw]);
                            continue;
                        }
                    }
                    let cands = self.desc_candidates(sym.get(&rs)).ok_or_else(|| {
                        format!("mubuf at {:#x}: s{rs} not a buffer descriptor ({:?})", ins.offset, sym.get(&rs))
                    })?;
                    let mut out: Vec<(u32, Res)> = Vec::new();
                    for (table, off) in &cands {
                        out.push((((*table as u32) << 24) | off, self.mubuf_res(ins, *off, *table)?));
                    }
                    self.res.insert(idx, vec![out[0].1]);
                    if out.len() > 1 {
                        self.bsel.insert(idx, (rs, out));
                    }
                }
                Enc::Mimg if matches!(ins.op, 230 | 231) => {
                    return Err(format!(
                        "raw BVH node intersection requires traversal translation at {:#x}",
                        ins.offset
                    ));
                }
                Enc::Mimg => {
                    let n = ins.src.len();
                    let rs = ins.src.get(n - 2).and_then(sg).ok_or("mimg: bad srsrc")?;
                    let ss = ins.src.get(n - 1).and_then(sg).ok_or("mimg: bad ssamp")?;
                    let dim = ins.aux.imm as u32;
                    let mut tex_res: Vec<(u32, Res)> = Vec::new();
                    if heap_descriptor(sym, rs, 8) {
                        let load = self.compute_heap && matches!(ins.mnemonic.as_str(), "image_load" | "image_load_mip")
                            && matches!(dim, 0 | 1 | 2 | 4 | 5);
                        // The heap does not name a view's component type. Sampling needs a
                        // float view; a texel load does not (see `heap_load_ty`).
                        let cube = self.compute_heap && dim == 3;
                        let size = self.compute_heap && dim == 1 && ins.mnemonic == "image_get_resinfo";
                        let lod = self.pixel_heap && dim == 1 && ins.mnemonic == "image_get_lod";
                        // A texel store passes its registers on unconverted, whatever the view.
                        let store = ins.mnemonic == "image_store" && dim == 1 && self.compute_heap
                            && matches!(self.ex.stage, Stage::Compute | Stage::Pixel);
                        if !load && !store && !lod && !size && (!(ins.mnemonic.starts_with("image_sample")
                            || matches!(ins.mnemonic.as_str(), "image_gather4_lz" | "image_gather4_lz_o")) || !(matches!(dim, 1 | 5) || cube)) {
                            return Err(format!("unsupported direct heap texture operation {} dim {dim}", ins.mnemonic));
                        }
                        tex_res.push((0, Res::HeapTexture(rs, dim)));
                    } else {
                        let tex_c = self.desc_candidates(sym.get(&rs)).ok_or_else(|| {
                            format!("mimg at {:#x}: s{rs} not an image descriptor ({:?})", ins.offset, sym.get(&rs))
                        })?;
                        for (ttable, off) in &tex_c {
                            let (kind, tslot) = self.table_binding(*ttable, *off, 32)?;
                            let is_uav = kind == Some(1);
                            if is_uav && ins.mnemonic.starts_with("image_atomic_") {
                                self.uav_uint.insert(tslot);
                            }
                            let map = if is_uav { &mut self.uav } else { &mut self.tex };
                            if let Some(prev) = map.insert(tslot, dim) {
                                if prev != dim {
                                    return Err(format!("image slot {tslot} used with two dimensions"));
                                }
                            }
                            tex_res.push((((*ttable as u32) << 24) | off, if is_uav { Res::Uav(tslot, dim) } else { Res::Tex(tslot, dim) }));
                        }
                    }
                    let mut samp_res: Vec<(u32, Res)> = Vec::new();
                    if ins.mnemonic.starts_with("image_sample") || ins.mnemonic.starts_with("image_gather") || ins.mnemonic == "image_get_lod" {
                        if let Some(candidates) = self.desc_candidates(sym.get(&ss)) {
                            for (table, offset) in candidates {
                                if Some(table) == self.sampler_heap_base {
                                    // Static index into the sampler descriptor heap.
                                    if offset % 16 != 0 {
                                        return Err("unaligned sampler heap offset".into());
                                    }
                                    self.heap_sampler_slots.insert(offset / 16);
                                    samp_res.push((((table as u32) << 24) | offset, Res::Samp(offset / 16)));
                                    continue;
                                }
                                let (_, slot) = self.table_binding(table, offset, 16)?;
                                self.table_samplers.insert(slot);
                                samp_res.push((((table as u32) << 24) | offset, Res::Samp(slot)));
                            }
                        } else if self.chosen_samplers.contains(&idx)
                            || matches!(sym.get(&ss), Some(Sym::HeapSamplerIndex(_))) && heap_words(sym, ss, 4)
                        {
                            self.uses_heap_sampler = true;
                            samp_res.push((0, Res::HeapSamplerDyn(ss)));
                        } else if let Some(Sym::HeapSampler(index)) = sym.get(&ss) {
                            self.uses_heap_sampler = true;
                            samp_res.push((0, Res::HeapSampler(*index)));
                        } else if let Some((slot, space)) = if let Some(&slot) = self.private_samplers.get(&ins.offset) {
                            Some((slot, 32765))
                        } else if inline_samplers {
                            let word = |n| match sym.get(&(ss + n)) {
                                Some(Sym::InlineWord(w)) => Some(*w),
                                _ => None,
                            };
                            let words = (|| Some([word(0)?, word(1)?, word(2)?, word(3)?]))().or_else(|| {
                                (self.ex.stage == Stage::Compute).then(|| sampler_heap::guarded_words(insns, idx, ss as u8, compute_group(self.ex))).flatten()
                            });
                            words.and_then(|w| sampler_heap::inline_sampler(self.ex, w, ins.op & 8 != 0))
                                .or_else(|| self.static_sampler())
                        } else { self.static_sampler() } {
                            if self.binding_spaces.insert((3, slot), space).is_some_and(|old| old != space) {
                                return Err(format!("static sampler s{slot} overlaps another space"));
                            }
                            self.table_samplers.insert(slot);
                            samp_res.push((0, Res::Samp(slot)));
                        } else {
                            let words: Vec<_> = (0..4).map(|n| match sym.get(&(ss + n)) { Some(Sym::InlineWord(w)) => format!("{w:#x}"), other => format!("{other:?}") }).collect();
                            return Err(format!("mimg at {:#x}: s{ss} not a sampler descriptor ({:?}) words [{}]", ins.offset, sym.get(&ss), words.join(", ")));
                        }
                        for &(_, sampler) in &samp_res {
                            let Res::Samp(sslot) = sampler else { continue };
                            // Depth-compare variants (opcode bit 3) need a comparison sampler.
                            if ins.op & 8 != 0 {
                                self.samp_cmp.insert(sslot);
                            } else {
                                self.samp.insert(sslot);
                            }
                        }
                    }
                    let mut v = vec![tex_res[0].1];
                    if let Some(s) = samp_res.first() {
                        v.push(s.1);
                    }
                    if tex_res.len() > 1 || samp_res.len() > 1 {
                        self.msel.insert(idx, MSel { tex_reg: rs, samp_reg: ss, tex: tex_res, samp: samp_res });
                    }
                    self.res.insert(idx, v);
                }
                Enc::Vop1 if ins.mnemonic == "v_nop" => {}

                _ => {
                    for d in &ins.dst {
                        if let Opnd::Vgpr(v) = d {
                            sym.remove(&(2000 + *v as u16));
                            if float64::pair_result(&ins.mnemonic) {
                                sym.remove(&(2001 + *v as u16));
                            }
                        }
                        if let Some(n) = sg(d) {
                            sym.remove(&n);
                            written.insert(n);
                            if wide_bit_result(&ins.mnemonic) {
                                sym.remove(&(n + 1));
                                written.insert(n + 1);
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// A two-word, non-returning IMAGE_ATOMIC_UMAX addresses one uint64 texel.
    /// Use that declaration only when every access to its typed UAV agrees.
    fn resolve_wide_image_atomics(&mut self) -> Result<(), String> {
        let mut wide = BTreeSet::new();
        let mut other = BTreeSet::new();
        for (&i, resources) in &self.res {
            let op = &self.insns[i];
            if op.enc != Enc::Mimg {
                if let Some((_, candidates)) = self.bsel.get(&i) {
                    for (_, resource) in candidates {
                        if let Res::Raw(slot, true) | Res::Buf(slot, true) | Res::TBuf(slot, true) = resource {
                            other.insert(*slot);
                        }
                    }
                }
                for resource in resources {
                    if let Res::Uav(slot, _) | Res::Raw(slot, true) | Res::Buf(slot, true)
                        | Res::TBuf(slot, true) = resource {
                        other.insert(*slot);
                    }
                }
                continue;
            }
            if let Some(selection) = self.msel.get(&i) {
                for (_, resource) in &selection.tex {
                    if let Res::Uav(slot, _) = resource {
                        other.insert(*slot);
                    }
                }
                continue;
            }
            let [Res::Uav(slot, 1)] = resources.as_slice() else {
                continue;
            };
            if wide_atomic_coordinates(op).is_ok()
                && self.ex.resources.iter().filter(|r| {
                    matches!(r.res_type, 6..=9)
                        && r.space == self.binding_spaces.get(&(1, *slot)).copied().unwrap_or(0)
                        && r.lower <= *slot && *slot <= r.upper
                }).count() == 1
                && self.ex.resources.iter().any(|r| {
                    (r.res_type, r.space, r.lower, r.upper) ==
                        (6, self.binding_spaces.get(&(1, *slot)).copied().unwrap_or(0), *slot, *slot)
                })
            {
                wide.insert(*slot);
            } else {
                other.insert(*slot);
            }
        }
        if !wide.is_disjoint(&other) || !self.wide_visibility_uavs.is_disjoint(&other) {
            return Err("mixed access widths on a raster image".into());
        }
        self.wide_visibility_uavs.extend(wide);
        Ok(())
    }

    /// Registers read before any write in program order — i.e. the shader's entry ABI
    /// (interpolation barycentrics, fragment position, user-data SGPRs, thread ids...).
    fn live_in(&self) -> Vec<String> {
        let mut written: BTreeSet<String> = BTreeSet::new();
        let mut live: BTreeSet<(u8, u32)> = BTreeSet::new();
        for ins in &self.insns {
            if matches!(ins.mnemonic.as_str(), "v_nop" | "s_setreg_imm32_b32") {
                continue;
            }
            // Accumulators and partial writes implicitly read the old destination.
            let keep_other_bits = ins.aux.sdwa.is_none_or(|sd| (sd >> 8) & 7 != 6 && (sd >> 11) & 3 == 2);
            let setreg = ins.mnemonic == "s_setreg_b32";
            let old_dst = ((setreg && modeled_hwreg_mask(ins.raw[0]) != 0)
                || matches!(ins.mnemonic.as_str(), "v_pk_fmac_f16" | "v_fma_f16" | "v_rcp_f16")
                || ((half_round_or_conversion(&ins.mnemonic) || ins.mnemonic == "v_cvt_f16_f32") && keep_other_bits)
                || int16::new_half(ins))
                .then(|| ins.dst.first()).flatten();
            for o in ins.src.iter().chain(old_dst) {
                if let Opnd::Sgpr(n) | Opnd::Vgpr(n) = o {
                    let nm = name(o).unwrap_or_default();
                    if !written.contains(&nm) {
                        live.insert((if matches!(o, Opnd::Vgpr(_)) { 1 } else { 0 }, *n as u32));
                    }
                }
            }
            if setreg {
                continue;
            }
            if let Some(high) = float64::pair_source_highs(ins) {
                if !written.contains(&format!("v{high}")) {
                    live.insert((1, high));
                }
            }
            let atomic = ins.mnemonic.starts_with("image_atomic_");
            if ins.mnemonic.starts_with("image_store") || atomic {
                if let Some(Opnd::Vgpr(base)) = ins.dst.first() {
                    for reg in u32::from(*base)..u32::from(*base) + u32::from(image_data_registers(ins)) {
                        if !written.contains(&format!("v{reg}")) {
                            live.insert((1, reg));
                        }
                    }
                }
                if !atomic || ins.raw[0] & (1 << 13) == 0 {
                    continue;
                }
            }
            // Multi-register destinations: SMEM, buffer and image loads, 64-bit scalar ops.
            let extra = match ins.enc {
                Enc::Smem => (1u32 << (ins.op & 7)) - 1,
                Enc::Mimg => u32::from(image_data_registers(ins)).saturating_sub(1),
                Enc::Mtbuf if ins.op < 4 => ins.op,
                Enc::Mubuf if ins.mnemonic.starts_with("buffer_load_dword") =>
                    Self::mubuf_dwords(&ins.mnemonic).unwrap_or(1) - 1,
                _ if wide_bit_result(&ins.mnemonic) => 1,
                _ if float64::pair_result(&ins.mnemonic) => 1,
                _ => 0,
            };
            for o in &ins.dst {
                if let Ok(nm) = name(o) {
                    written.insert(nm);
                }
                match o {
                    Opnd::Sgpr(n) => (1..=extra).for_each(|k| { written.insert(format!("s{}", *n as u32 + k)); }),
                    Opnd::Vgpr(n) => (1..=extra).for_each(|k| { written.insert(format!("v{}", *n as u32 + k)); }),
                    _ => {}
                }
            }
        }
        live.iter().map(|(k, n)| format!("{}{}", if *k == 1 { "v" } else { "s" }, n)).collect()
    }

    fn line(&mut self, s: &str) {
        self.note_sgpr_stores(s);
        self.body.push_str("    ");
        // These are per-invocation EXEC guards in the emitter templates.
        // Scalar branches and mask-producing instructions select their
        // whole-wave forms before reaching here.
        if self.resolve_wave32 && s.contains("exec_lo & 1u") {
            self.body.push_str(&s.replace("exec_lo & 1u", "exec_lo & tb_lane_bit"));
        } else {
            self.body.push_str(s);
        }
        self.body.push('\n');
    }

    /// A store to `sN` (including `sN &= ...`) drops every heap handle captured
    /// from that register. `sN ==` is a compare and keeps the handle.
    fn note_sgpr_stores(&mut self, stmt: &str) {
        if self.heap_bound.is_empty() {
            return;
        }
        let bytes = stmt.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let boundary = i == 0 || {
                let prev = bytes[i - 1];
                !prev.is_ascii_alphanumeric() && prev != b'_'
            };
            if boundary && bytes[i] == b's' {
                let mut j = i + 1;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if j > i + 1 {
                    let mut k = j;
                    while k < bytes.len() && bytes[k] == b' ' {
                        k += 1;
                    }
                    let compound = k + 1 < bytes.len()
                        && matches!(bytes[k], b'+' | b'-' | b'&' | b'|' | b'^' | b'*')
                        && bytes[k + 1] == b'=';
                    let plain = bytes.get(k) == Some(&b'=') && bytes.get(k + 1) != Some(&b'=');
                    if compound || plain {
                        if let Ok(reg) = stmt[i + 1..j].parse::<u16>() {
                            self.heap_bound.retain(|&(_, sgpr)| sgpr != reg);
                        }
                    }
                    i = j;
                    continue;
                }
            }
            i += 1;
        }
    }

    /// The next heap use must assign its local again. A back edge, or a join
    /// that may have skipped the last assignment, makes the captured handle stale.
    fn drop_heap_handles(&mut self) {
        self.heap_bound.clear();
    }

    fn end_guard(&mut self, open: &mut bool) {
        if *open {
            self.line("}");
            *open = false;
            self.drop_heap_handles();
        }
    }

    /// Assign `ResourceDescriptorHeap[sN]` into one local of this view.
    /// The same SGPR keeps that local until [`note_sgpr_stores`] or [`drop_heap_handles`].
    fn bind_heap(&mut self, view: HeapView, sgpr: u16) -> String {
        let local = view.local(sgpr);
        self.heap_locals.insert((view, sgpr));
        self.regs.insert(format!("s{sgpr}"));
        if self.heap_bound.insert((view, sgpr)) {
            let index = if self.pixel_heap { format!("NonUniformResourceIndex(s{sgpr})") } else { format!("s{sgpr}") };
            self.line(&format!("{local} = ResourceDescriptorHeap[{index}];"));
        }
        local
    }

    /// Function-scoped declarations for [`bind_heap`]. Each local is assigned
    /// before any method call. DXC accepts the declaration without an initializer.
    fn heap_handle_decls(&self) -> String {
        let mut out = String::new();
        for &(view, sgpr) in &self.heap_locals {
            let _ = writeln!(out, "    {} {};", view.hlsl(), view.local(sgpr));
        }
        out
    }

    fn fp32_extrema(&self, ins: &Insn, first: String) -> Result<String, String> {
        let maximum = ins.mnemonic == "v_max_f32";
        let mode = if self.regs.contains("tb_fp_mode") { "tb_fp_mode" } else { "0xc0u" };
        let (first, second) = dpp_fp32_sources(ins, first)?;
        Ok(format!("tb_quad_extrema({first}, {second}, {maximum}, {mode})"))
    }

    fn push_helpers(&self, h: &mut String) {
        h.push_str(CUBE_HELPERS);
        if self.insns.iter().any(clamped_product24) {
            h.push_str(MAD24_SAT);
        }
        h.push_str(if self.single_wave32 { HALF_RNE_STRAIGHT_LINE } else { HALF_RNE });
        h.push_str(HALF_HELPERS);
        let has_fp_mode = self.regs.contains("tb_fp_mode");
        if self.insns.iter().any(|ins| matches!(ins.mnemonic.as_str(), "v_cvt_u32_f32" | "v_cvt_i32_f32" | "v_cvt_pk_u8_f32")
            || (has_fp_mode && matches!(ins.mnemonic.as_str(), "v_cvt_f32_i32" | "v_cvt_f32_u32"))) {
            h.push_str(include_str!("float_to_integer.hlsl"));
        }
        if self.insns.iter().any(|ins| ins.mnemonic == "v_fract_f32") {
            h.push_str(include_str!("fract_f32.hlsl"));
        }
        if self.insns.iter().any(|ins| ins.mnemonic == "v_ldexp_f32") {
            h.push_str(include_str!("ldexp_f32.hlsl"));
        }
        if self.insns.iter().any(|i| i.op == OP_DPP_BINARY && matches!(i.mnemonic.as_str(), "v_min_f32" | "v_max_f32")) {
            h.push_str(r#"
uint tb_quad_extrema(uint a, uint b, bool maximum, uint mode) {
    if ((mode & 0x10u) == 0u) {
        if ((a & 0x7fffffffu) < 0x00800000u) a &= 0x80000000u;
        if ((b & 0x7fffffffu) < 0x00800000u) b &= 0x80000000u;
    }
    // Order -0 below +0. Keep operands as bits for the non-IEEE NaN selection rule.
    uint keyA = (a & 0x80000000u) != 0u ? ~a : a ^ 0x80000000u;
    uint keyB = (b & 0x80000000u) != 0u ? ~b : b ^ 0x80000000u;
    uint result = (maximum ? keyA > keyB : keyA < keyB) ? a : b;
    if ((a & 0x7fffffffu) > 0x7f800000u) result = b;
    else if ((b & 0x7fffffffu) > 0x7f800000u) result = a;
    if ((mode & 0x20u) == 0u && (result & 0x7fffffffu) < 0x00800000u) result &= 0x80000000u;
    return result;
}
"#);
        }
        if self.insns.iter().any(|i| i.mnemonic.starts_with("v_cmp") && (i.mnemonic.ends_with("_f64") || i.mnemonic.ends_with("_f32") || i.mnemonic.ends_with("_f16"))) {
            h.push_str(compare::FLOAT_HELPERS);
        }
        if self.insns.iter().any(|i| float64::handles(&i.mnemonic)) {
            h.push_str(float64::HELPERS);
        }
        if self.insns.iter().any(|i| matches!(i.mnemonic.as_str(), "s_lshr_b64" | "s_ashr_i64" | "s_bfe_u64" | "s_bfe_i64")) {
            h.push_str(r#"
uint2 tb_shr64(uint lo, uint hi, uint n, bool sign) {
    n &= 63u;
    uint fill = sign ? asuint(asint(hi) >> 31) : 0u;
    if (n == 0u) return uint2(lo, hi);
    uint k = n & 31u;
    uint upper = sign ? asuint(asint(hi) >> k) : hi >> k;
    return n >= 32u ? uint2(upper, fill) : uint2((lo >> k) | (hi << (32u-k)), upper);
}
uint2 tb_field64(uint lo, uint hi, uint control, bool sign) {
    uint offset = control & 63u, width = min((control >> 16) & 127u, 64u-offset);
    if (width == 0u) return uint2(0u,0u);
    uint2 v = tb_shr64(lo, hi, offset, false);
    uint bit = width - 1u;
    uint fill = sign && (((bit < 32u ? v.x : v.y) >> (bit & 31u)) & 1u) ? ~0u : 0u;
    uint2 mask = uint2(width >= 32u ? ~0u : (1u << width)-1u,
                       width == 64u ? ~0u : width <= 32u ? 0u : (1u << (width-32u))-1u);
    return (v & mask) | (fill & ~mask);
}
"#);
        }
        if self.insns.iter().any(|i| normalized_half_to_integer(&i.mnemonic)) {
            h.push_str(include_str!("normalized_half.hlsl"));
        }
        let uses_half_unary = self.insns.iter().any(|i| half_unary_op(&i.mnemonic).is_some());
        let uses_half_output = self.insns.iter().any(|i| {
            let modified = (i.enc == Enc::Vop3 && (i.aux.imm != 0 || i.aux.imm2 != 0))
                || i.aux.sdwa.is_some_and(|sd| sd & 0xe000 != 0);
            modified && (half_unary_op(&i.mnemonic).is_some()
                || integer_to_half(&i.mnemonic) || i.mnemonic == "v_cvt_f16_f32")
        });
        if self.regs.contains("tb_fp_mode") || uses_half_unary || uses_half_output {
            h.push_str(r#"
// V_CVT_F16_F32 uses the destination round mode. Work with magnitudes
// so tiny directed results survive the host's single-precision FTZ mode.
uint tb_mode_half(float value, uint mode) {
    uint bits = asuint(value), magnitude = bits & 0x7fffffffu;
    if (magnitude < 0x00800000u && (mode & 0x10u) == 0u)
        return (bits >> 16) & 0x8000u;
    // FP16_OVFL saturates finite overflow, preserving true infinities.
    if ((mode & 0x800000u) != 0u && magnitude > 0x477fe000u && magnitude < 0x7f800000u)
        return ((bits >> 16) & 0x8000u) | 0x7bffu;
    uint rounding = (mode >> 2) & 3u;
    uint h = tb_f16_cvt(value, rounding != 0u);
    bool away = rounding == ((bits >> 31) != 0u ? 2u : 1u);
    if (away && magnitude < 0x7f800000u && asuint(f16tof32(h & 0x7fffu)) < magnitude) h++;
    return (h & 0x7fffu) < 0x400u && (mode & 0x80u) == 0u ? h & 0x8000u : h;
}
"#);
        }
        if uses_half_unary {
            h.push_str(include_str!("half_unary.hlsl"));
        }
        if uses_half_output {
            h.push_str(r#"
uint tb_half_output(uint bits, uint mode, float scale, bool clampResult) {
    // Scaling preserves NaN payloads. CLAMP clears them only in DX10 mode.
    if ((bits & 0x7fffu) > 0x7c00u)
        return clampResult && (mode & 0x100u) != 0u ? 0u : bits;
    // On RDNA2, OMOD is disabled with output denormals or IEEE mode.
    if (scale != 1.0 && (mode & 0x280u) == 0u) {
        float x = (bits & 0x7fffu) == 0u ? 0.0 : f16tof32(bits);
        bits = tb_mode_half(x * scale, mode);
    }
    if (!clampResult) return bits;
    return (bits & 0x8000u) != 0u ? 0u : min(bits, 0x3c00u);
}
"#);
        }
        if self.insns.iter().any(|i| matches!(i.mnemonic.as_str(), "v_fma_f16" | "v_pk_fmac_f16" | "v_fmaak_f16" | "v_fmamk_f16")) {
            h.push_str(r#"
// A binary16 product is exact in binary32. Recover the residual of the addition
// before rounding to half: a rounded float sum alone can land on the wrong tie.
float tb_fma_half(float a, float b, float addend) {
    precise float p = a * b;
    precise float sum = p + addend;
    uint bits = asuint(sum);
    if ((bits & 0x7fffffffu) < 0x7f800000u && (bits & 1u) == 0u) {
        precise float residual = abs(p) >= abs(addend) ? addend - (sum - p) : p - (sum - addend);
        if (residual != 0.0) bits += ((residual > 0.0) == (sum > 0.0)) ? 1u : 0xffffffffu;
    }
    return asfloat(bits);
}
uint tb_fmac_half(uint a, uint b, uint c) {
    return tb_f16_rne(tb_fma_half(f16tof32(a & 65535u), f16tof32(b & 65535u), f16tof32(c & 65535u)));
}
"#);
        }
    }

    fn touch(&mut self, o: &Opnd) {
        if let Opnd::Sgpr(_) | Opnd::Vgpr(_) = o {
            if let Ok(n) = name(o) {
                self.regs.insert(n);
            }
        }
    }

    /// Guarded write to a VGPR (inactive lanes keep their value).
    fn wv(&mut self, dst: &Opnd, expr: &str) -> Result<(), String> {
        self.touch(dst);
        let d = name(dst)?;
        self.line(&format!("if (exec_lo & 1u) {d} = {expr};"));
        Ok(())
    }

    fn ws(&mut self, dst: &Opnd, expr: &str) -> Result<(), String> {
        self.touch(dst);
        let d = name(dst)?;
        self.line(&format!("{d} = {expr};"));
        Ok(())
    }

    /// Condition under which a SOPP branch opcode is taken (None = not a branch).
    fn branch_cond(op: u32) -> Option<&'static str> {
        Some(match op {
            2 => "true",
            4 => "scc == 0u",
            5 => "scc != 0u",
            6 => "(vcc_lo & 1u) == 0u",
            7 => "(vcc_lo & 1u) != 0u",
            8 => "(exec_lo & 1u) == 0u",
            9 => "(exec_lo & 1u) != 0u",
            _ => return None,
        })
    }

    /// Control flow.
    ///
    /// * Forward branches use skip flags: `jT` is set when a branch to instruction T is
    ///   taken, and every instruction before T that lies after such a branch is wrapped in
    ///   `if (!(jT || ...))`. This handles arbitrary forward-only CFGs (if/else, breaks).
    /// * A backward branch at B to header L becomes `do { ... } while (cond)` around
    ///   [L..=B]. Flags for labels inside the body are reset each iteration; flags for
    ///   labels past B (loop exits) terminate the loop and keep skipping until their label.
    ///   Loops must be properly nested, have one back-edge per header, and must not be
    ///   entered from outside except through the header — otherwise `Err`.
    fn emit_body(&mut self, is_ps: bool) -> Result<(), String> {
        let mut off2idx: HashMap<usize, usize> = self.insns.iter().enumerate().map(|(i, x)| (x.offset, i)).collect();
        // A branch to the address just past the last instruction leaves the program (same as
        // reaching the final s_endpgm).
        if let Some(l) = self.insns.last() {
            off2idx.insert(l.offset + l.dwords * 4, self.insns.len());
        }
        let prog_end = self.insns.last().map_or(0, |l| l.offset + l.dwords * 4);
        let mut target: HashMap<usize, usize> = HashMap::new(); // forward: branch idx -> target idx
        let mut back: HashMap<usize, usize> = HashMap::new(); // backward: branch idx B -> header idx L
        let n_insns = self.insns.len();
        for (i, ins) in self.insns.iter().enumerate() {
            if ins.enc == Enc::Sopp && ins.op == 1 && i + 1 < n_insns {
                target.insert(i, n_insns); // early s_endpgm -> end of program
                continue;
            }
            if ins.enc == Enc::Sopp && Self::branch_cond(ins.op).is_some() {
                let t = branch_pc(ins);
                let ti = if t >= 0 && t as usize >= prog_end {
                        self.insns.len() // at or beyond the end of the program (s_code_end padding): exit
                    } else {
                        *off2idx
                            .get(&(t as usize))
                            .ok_or_else(|| format!("branch at {:#x} targets mid-instruction {t:#x}", ins.offset))?
                    };
                if ti <= i {
                    back.insert(i, ti);
                } else {
                    target.insert(i, ti);
                }
            }
        }
        // Validate loop structure.
        // Several back-edges to one header = `continue`s. The last one (highest address)
        // closes the loop; each earlier one becomes a forward jump to that closing branch
        // which also forces another iteration (see the `|| j{B}` at the loop bottom).
        let mut heads: HashMap<usize, usize> = HashMap::new(); // L -> B
        for (&b, &l) in &back {
            let e = heads.entry(l).or_insert(b);
            *e = (*e).max(b);
        }
        let continues: Vec<(usize, usize)> =
            back.iter().filter(|(&b, &l)| heads[&l] != b).map(|(&b, &l)| (b, heads[&l])).collect();
        for (b, closing) in continues {
            back.remove(&b);
            target.insert(b, closing);
        }
        // Sorted: `heads` is a HashMap, and the order of `loops` decides the order of the
        // emitted flag declarations. Iteration order would make the HLSL (and with it the
        // compiled-shader cache key) differ between runs.
        let mut loops: Vec<(usize, usize)> = heads.iter().map(|(&l, &b)| (l, b)).collect();
        loops.sort_unstable();
        for &(l1, b1) in &loops {
            for &(l2, b2) in &loops {
                let disjoint = b1 < l2 || b2 < l1;
                let nested = (l1 <= l2 && b2 <= b1) || (l2 <= l1 && b1 <= b2);
                if !(disjoint || nested) {
                    // Interleaved loops cannot be expressed as nested do/while: fall back
                    // to the general block dispatcher.
                    if self.insns.iter().any(is_quad_exchange) && !self.pixel_wave {
                        return Err("DPP quad exchange with interleaved loops".into());
                    }
                    return self.emit_body_dispatch(is_ps, 0);
                }
            }
            // A forward branch from before the header into the body (the usual rotated
            // `while`: jump straight to the condition) is handled by the skip flags: the
            // flag stays set on the first iteration, so everything before the label is
            // skipped, and in-body flags are cleared at the bottom of each iteration.
        }
        let labels: BTreeSet<usize> = target.values().copied().collect();
        for l in &labels {
            self.line(&format!("bool j{l} = false;"));
        }
        if !loops.is_empty() {
            self.line("uint tb_guard = 0u;");
            for &(_, b) in &loops {
                self.line(&format!("bool tb_c{b} = false;"));
            }
        }

        let insns = self.insns.clone();
        let view_cull = self.view_cull;
        let probe_trace = insns.iter().any(|op| matches!(op.mnemonic.as_str(),"tb_radiance_query" | "tb_reflection_far_query"));
        if view_cull.is_some() { self.line("uint cull_view = 0u; bool cull_candidate = false; uint3 cull_record = 0;"); }
        if insns.iter().any(|op| op.mnemonic == "tb_view_occluded") {
            self.line("bool hidden = false; uint hidden_id = 0u, hidden_view = 0u;");
        }
        let group_size = compute_group(self.ex).into_iter().fold(1u32, u32::saturating_mul);
        let lds_waits = if self.is_cs && (2..=64).contains(&group_size) {
            lds_order::exchange_waits(&insns, &loops)
        } else {
            BTreeSet::new()
        };
        let restored_waits = if self.is_cs && group_size == 64 {
            lds_order::restored_write_waits(&insns, &loops)
        } else {
            BTreeSet::new()
        };
        let mut open = false;
        let mut cur: Vec<usize> = vec![];
        let mut prior_lds_write = None;
        for (i, ins) in insns.iter().enumerate() {
            // One guest wave exchanges halfwords through LDS in instruction order.
            // Host invocations must finish reading before another lane overwrites
            // the tile, and finish publishing writes before other lanes read.
            // Keep the rendezvous outside the translated guest branch guard.
            if self.single_wave32 && ins.enc == Enc::Ds {
                let write = ins.mnemonic.starts_with("ds_write");
                if prior_lds_write.replace(write).is_some_and(|was_write| was_write != write) {
                    if !back.is_empty() {
                        return Err("single-wave LDS exchange inside a loop".into());
                    }
                    self.end_guard(&mut open); cur.clear();
                    self.line("GroupMemoryBarrierWithGroupSync();");
                }
            }
            // Reconverge both host subgroups after each guest view, including
            // invocations whose guest EXEC mask branched over the queue append.
            if let Some(program) = view_cull.filter(|p| p.join == ins.offset) {
                self.end_guard(&mut open);
                cur.clear();
                program.append(self);
            }
            if let Some(&b) = heads.get(&i) {
                self.end_guard(&mut open);
                cur.clear();
                self.line("do {");
                // The back edge can store the index after a use. Bind again inside the loop.
                self.drop_heap_handles();
                self.loop_depth += 1;
                self.line(&format!("tb_c{b} = false;"));
            }
            let mut active: Vec<usize> =
                labels.iter().copied().filter(|&t| t > i && target.iter().any(|(&b, &bt)| bt == t && b < i)).collect();
            if self.page_waves || self.hoist_wave_exchanges {
                let exchange = if self.page_waves {
                    page_waves::rendezvous(i, ins)?
                } else {
                    let local = self.own_lane_reads.contains(&i)
                        || (matches!(ins.mnemonic.as_str(), "s_cbranch_execz" | "s_cbranch_vccz" | "s_cbranch_vccnz") && self.in_loop(i));
                    if local { None } else { wave_cull::exchange(i, ins, self.ex.stage == Stage::Mesh)? }
                };
                if let Some(code) = exchange {
                    self.end_guard(&mut open);
                    cur.clear();
                    self.line(&code);
                }
            }
            let mesh_condition = self.hoist_wave_exchanges && self.ex.stage == Stage::Mesh
                && reads_scc(ins) && !self.in_loop(i) && scc_from_mask(&insns, i);
            if mesh_condition {
                // Mask operations set SCC if any bit in the guest wave is
                // set. Vote outside the forward branch guards so that every
                // workgroup invocation reaches the barriers.
                self.end_guard(&mut open);
                cur.clear();
                self.line(&format!("uint2 mesh_scc{i} = tb_page_ballot(tb_wave_tid, scc != 0u);"));
            }
            // Exited guest waves no longer count towards S_BARRIER. Their host
            // threads still belong to the group and must participate in the HLSL
            // barrier. Only lift the barrier if every enclosing skip exits the
            // program and no loop could change how many times a wave reaches it.
            if self.is_cs && ins.mnemonic == "s_barrier" && loops.is_empty()
                && active.iter().all(|&t| t == insns.len() || (t + 1 == insns.len() && insns[t].mnemonic == "s_endpgm"))
            {
                active.clear();
            }
            if self.compute_swizzle && exchanges(ins) && !active.is_empty() {
                self.end_guard(&mut open);
                cur.clear();
                self.emit_compute_swizzle(i, ins, &active)?;
                self.hoisted_swizzle = Some(i);
            }
            if ins.mnemonic == "tb_first_exec_lane" && !self.exec_snapshots.sites.contains_key(&i) {
                self.end_guard(&mut open);
                cur.clear();
                let mut condition=String::from("(exec_lo & 1u) != 0u");
                for t in &active { write!(condition," && !j{t}").unwrap(); }
                self.line(&format!("uint election{i}base = tb_lane & ~63u;"));
                self.line(&format!("if ((tb_lane & 63u)==0) tb_swizzle[election{i}base]=0xffffffffu;"));
                self.line("GroupMemoryBarrierWithGroupSync();");
                self.line(&format!("if ({condition}) InterlockedMin(tb_swizzle[election{i}base],tb_lane & 63u);"));
                self.line("GroupMemoryBarrierWithGroupSync();");
                self.line(&format!("uint election{i}=tb_swizzle[election{i}base];"));
                self.line("GroupMemoryBarrierWithGroupSync();");
            }
            if ins.mnemonic == "tb_indexed_readlane" && !active.is_empty() {
                self.end_guard(&mut open);
                cur.clear();
                self.emit_indexed_read(i, ins)?;
                self.hoisted_swizzle = Some(i);
            }
            if self.micropoly_vote == Some(i) {
                // Retired triangles contribute zero and still participate in both
                // barriers. The count precedes either rasterization loop.
                self.end_guard(&mut open);
                micropoly::emit_vote(self, ins, &active)?;
                return Ok(());
            }
            if is_quad_exchange(ins) {
                if self.loop_depth != 0 && (!self.pixel_wave || is_row_move(ins)) {
                    return Err("DPP16 exchange inside backward control flow".into());
                }
                if open {
                    self.end_guard(&mut open);
                    cur.clear();
                }
                if self.pixel_wave && !is_row_move(ins) {
                    self.emit_pixel_quad_exchange(i, ins)?;
                } else {
                    let source = r(&ins.src[0])?;
                    if ins.op == OP_DPP_BINARY {
                        // All host lanes participate before the guest destination guards.
                        let perm = ins.aux.imm;
                        self.line(&format!("uint q{i}lane = WaveGetLaneIndex();"));
                        let dpp8 = group_extrema::is_dpp8(ins);
                        let lane = if dpp8 {
                            format!("(q{i}lane & ~7u) | (({perm}u >> (3u * (q{i}lane & 7u))) & 7u)")
                        } else if perm == 0x140 {
                            format!("q{i}lane ^ 15u")
                        } else if (0x121..=0x12f).contains(&perm) {
                            format!("(q{i}lane & ~15u) | ((q{i}lane - {}u) & 15u)", perm & 15)
                        } else { format!("(q{i}lane & ~3u) | (({perm}u >> (2u * (q{i}lane & 3u))) & 3u)") };
                        if dpp8 || ins.raw[1] & (1 << 18) != 0 {
                            self.line(&format!("uint q{i} = WaveReadLaneAt({source}, {lane});"));
                        } else if ins.raw[1] & (1 << 19) != 0 {
                            self.line(&format!("uint q{i} = WaveReadLaneAt((exec_lo & 1u) != 0u ? {source} : 0u, {lane});"));
                        } else {
                            self.line(&format!("uint2 q{i}pair = WaveReadLaneAt(uint2({source}, exec_lo & 1u), {lane});"));
                            self.line(&format!("uint q{i} = q{i}pair.x;"));
                            self.line(&format!("bool q{i}active = q{i}pair.y != 0u;"));
                        }
                    } else {
                        let selector = ins.aux.imm;
                        let lane = match ins.op {
                            OP_QUAD_MOVE => {
                                self.line(&format!("uint q{i}lane = WaveGetLaneIndex();"));
                                format!("(q{i}lane & ~3u) | (({selector}u >> (2u * (q{i}lane & 3u))) & 3u)")
                            }
                            OP_ROW_SHARE_MOVE => format!("(WaveGetLaneIndex() & ~15u) | {selector}u"),
                            OP_ROW_ROR_MOVE => format!("(WaveGetLaneIndex() & ~15u) | ((WaveGetLaneIndex() - {selector}u) & 15u)"),
                            OP_ROW_SHIFT_MOVE => {
                                let sign = if selector < 0 { "-" } else { "+" };
                                self.line(&format!("uint q{i}offset = (WaveGetLaneIndex() & 15u) {sign} {}u;", selector.abs()));
                                // Keep the wave read in range even for lanes that will discard its value.
                                format!("(WaveGetLaneIndex() & ~15u) | (q{i}offset & 15u)")
                            }
                            _ => format!("WaveGetLaneIndex() ^ {selector}u"),
                        };
                        let fetch_inactive = ins.raw[1] & (1 << 18) != 0;
                        let bound_ctrl = ins.raw[1] & (1 << 19) != 0;
                        let lane = if is_row_move(ins) || !fetch_inactive {
                            self.line(&format!("uint q{i}src = {lane};"));
                            format!("q{i}src")
                        } else { lane };
                        if fetch_inactive {
                            self.line(&format!("uint q{i} = WaveReadLaneAt({source}, {lane});"));
                        } else if bound_ctrl {
                            self.line(&format!("uint q{i} = WaveReadLaneAt((exec_lo & 1u) != 0u ? {source} : 0u, {lane});"));
                        } else {
                            // An inactive source suppresses the write to this lane's destination.
                            let dst = r(&ins.dst[0])?;
                            self.line(&format!("uint2 q{i}pair = WaveReadLaneAt(uint2({source}, exec_lo & 1u), {lane});"));
                            self.line(&format!("uint q{i} = q{i}pair.y != 0u ? q{i}pair.x : {dst};"));
                        }
                        if ins.op == OP_ROW_SHIFT_MOVE {
                            let fallback = if bound_ctrl { "0u".to_string() } else { r(&ins.dst[0])? };
                            self.line(&format!("if (q{i}offset >= 16u) q{i} = {fallback};"));
                        }
                    }
                }
            }
            if restored_waits.contains(&i) {
                self.end_guard(&mut open);
                cur.clear();
                self.line("GroupMemoryBarrierWithGroupSync();");
            }
            if active != cur || (open && heads.contains_key(&i)) {
                self.end_guard(&mut open);
                if !active.is_empty() {
                    let c: Vec<String> = active.iter().map(|t| format!("j{t}")).collect();
                    self.line(&format!("if (!({})) {{", c.join(" || ")));
                    open = true;
                }
                cur = active;
            }
            for o in ins.src.iter().chain(ins.dst.iter()) {
                self.touch(o);
            }
            if mesh_condition {
                self.line(&format!("scc = uint(any(mesh_scc{i}));"));
            }
            if lds_waits.contains(&i) && !restored_waits.contains(&i) {
                self.line("GroupMemoryBarrierWithGroupSync();");
            }
            if self.packed.as_ref().is_some_and(|p| p.device_sync_before.contains(&i)) {
                // Publish every coarse luminance texel before this group takes its ticket.
                self.line("AllMemoryBarrierWithGroupSync();");
            }
            self.emit_insn(i, ins, is_ps, target.get(&i).copied())
                .map_err(|e| format!("{e} [{} @ {:#x}]", ins.mnemonic, ins.offset))?;
            if open && target.get(&i).is_some_and(|t| cur.contains(t)) {
                // Another branch to an enclosing target just updated its flag.
                // The following instruction must reevaluate the guard.
                self.end_guard(&mut open);
                cur.clear();
            }
            if back.contains_key(&i) {
                // Evaluate the back-edge condition inside the guard (a skipped branch is not taken).
                let cond = wave_cull::branch_condition(self, ins, Self::branch_cond(ins.op).unwrap_or("false"));
                self.line(&format!("tb_c{i} = ({cond});"));
                if labels.contains(&i) {
                    // A `continue` jumped here: iterate again regardless of the condition.
                    self.line(&format!("tb_c{i} = tb_c{i} || j{i};"));
                }
                self.end_guard(&mut open);
                cur.clear();
                // Clear the flags of labels inside this loop body for the next iteration.
                let head = back[&i];
                for t in labels.iter().filter(|&&t| t > head && t <= i) {
                    self.line(&format!("j{t} = false;"));
                }
                let exits: Vec<String> = labels.iter().filter(|&&t| t > i).map(|t| format!("j{t}")).collect();
                let exit = if exits.is_empty() { String::new() } else { format!(" && !({})", exits.join(" || ")) };
                let guard = if self.wave_cull || self.light_grid || probe_trace
                    || self.wave_bounded_latches.contains(&i)
                    || self.packed.as_ref().is_some_and(|p| p.copy_backedges.contains(&i) || p.bounded_backedges.contains(&i))
                {
                    ""
                } else {
                    " && ++tb_guard < 65536u"
                };
                if view_cull.is_some_and(|p| p.back == ins.offset) {
                    // The guest iterates cb0[12] views. Retired lanes must
                    // participate in the barriers for each remaining view.
                    self.line("} while (++cull_view < cb0[3].x);");
                } else {
                    self.line(&format!("}} while (tb_c{i}{exit}{guard});"));
                }
                self.loop_depth = self.loop_depth.saturating_sub(1);
                self.drop_heap_handles();
            }
        }
        if open {
            self.end_guard(&mut open);
        }
        Ok(())
    }

    /// Follow guest basic blocks from `entry`. Each block sets the next block id
    /// (`tb_pc`). The id one past the last instruction terminates the program.
    /// (Implicit-derivative sampling cannot live inside this loop — FXC rejects it, and the
    /// shader then fails to compile rather than silently changing meaning.)
    fn emit_pixel_quad_exchange(&mut self, i: usize, ins: &Insn) -> Result<(), String> {
        let mut source = r(&ins.src[0])?;
        let mut ty = "uint";
        let ctrl = ins.raw[1];
        let check_exec = ctrl & (1 << 18) == 0;
        let preserve = check_exec && ctrl & (1 << 19) == 0;
        if check_exec {
            // Pixel entry initializes the per-invocation EXEC bit even on helper
            // lanes. Only trust it after an explicit full-wave reset; CMPX may
            // then disable individual lanes, including covered pixels.
            let prefix = &self.insns[..i];
            let mut preceding = prefix.iter().rev();
            let mut reset = preceding.next();
            if reset.is_some_and(|op| op.mnemonic.starts_with("v_cmpx_")) {
                reset = preceding.next();
            }
            let full_exec = reset.is_some_and(|op| op.mnemonic == "s_mov_b64"
                && op.dst == [Opnd::ExecLo] && op.src == [Opnd::ConstInt(-1)]);
            if !full_exec || self.dispatched || self.insns.iter().any(|op| op.enc == Enc::Sopp && Self::branch_cond(op.op).is_some()) {
                return Err(format!("pixel DPP16 FI=0 at {:#x} requires a proven source EXEC mask", ins.offset));
            }
            if preserve {
                ty = "uint2";
                source = format!("uint2({source}, exec_lo & 1u)");
            } else {
                source = format!("((exec_lo & 1u) != 0u ? {source} : 0u)");
            }
        }
        let perm = ins.aux.imm;
        // Quad operations exchange values with helper lanes. Lane bits are x, y.
        self.line(&format!("uint q{i}lane = WaveGetLaneIndex() & 3u, q{i}src = (({perm}u >> (2u * q{i}lane)) & 3u) ^ q{i}lane;"));
        self.line(&format!("{ty} q{i}x = QuadReadAcrossX({source}), q{i}y = QuadReadAcrossY({source}), q{i}d = QuadReadAcrossDiagonal({source});"));
        let selected = if preserve { format!("q{i}pair") } else { format!("q{i}") };
        self.line(&format!("{ty} {selected} = q{i}src == 0u ? {source} : q{i}src == 1u ? q{i}x : q{i}src == 2u ? q{i}y : q{i}d;"));
        if preserve {
            if ins.op == OP_QUAD_MOVE {
                let dst = r(&ins.dst[0])?;
                self.line(&format!("uint q{i} = q{i}pair.y != 0u ? q{i}pair.x : {dst};"));
            } else {
                self.line(&format!("uint q{i} = q{i}pair.x; bool q{i}active = q{i}pair.y != 0u;"));
            }
        }
        Ok(())
    }

    fn emit_body_dispatch(&mut self, is_ps: bool, entry: usize) -> Result<(), String> {
        // Per-invocation dispatch cannot guarantee whole-wave arrival at a vote.
        if !self.reduction_execz_guards.is_empty() {
            return Err("wave reduction EXECZ vote in a block-dispatched program".into());
        }
        if !self.wave_bounded_latches.is_empty() {
            return Err("bounded wave reduction in a block-dispatched program".into());
        }
        self.dispatched = true;
        let n = self.insns.len();
        let mut off2idx: HashMap<usize, usize> = self.insns.iter().enumerate().map(|(i, x)| (x.offset, i)).collect();
        if let Some(l) = self.insns.last() {
            off2idx.insert(l.offset + l.dwords * 4, n);
        }
        let prog_end = self.insns.last().map_or(0, |l| l.offset + l.dwords * 4);
        let mut leaders = BTreeSet::from([entry]);
        let mut tgt: HashMap<usize, usize> = HashMap::new();
        for (i, ins) in self.insns.iter().enumerate().skip(entry) {
            if ins.enc != Enc::Sopp {
                continue;
            }
            if Self::branch_cond(ins.op).is_some() {
                let t = branch_pc(ins);
                let ti = if t >= 0 && t as usize >= prog_end {
                        self.insns.len() // at or beyond the end of the program (s_code_end padding): exit
                    } else {
                        *off2idx
                            .get(&(t as usize))
                            .ok_or_else(|| format!("branch at {:#x} targets mid-instruction {t:#x}", ins.offset))?
                    };
                tgt.insert(i, ti);
                leaders.insert(ti);
                leaders.insert(i + 1);
            } else if ins.op == 1 {
                leaders.insert(i + 1);
            }
        }
        // A guest wave reaches S_BARRIER as a whole: lanes that left a loop early wait in
        // EXEC for the rest. Here each lane follows its own blocks, so the barrier starts
        // one that a lane enters only when its wave has caught up.
        let barriers: Vec<usize> = (entry..n)
            .filter(|&i| self.is_cs && self.insns[i].enc == Enc::Sopp && self.insns[i].op == 0x0a)
            .collect();
        leaders.extend(&barriers);
        let starts: Vec<usize> = leaders.into_iter().filter(|&s| s < n).collect();
        self.loop_depth = 0;
        if entry == 0 {
            self.line("uint tb_pc = 0u; uint tb_guard = 0u;");
            self.line(&format!("[loop] while (tb_pc != {n}u && tb_guard++ < 1000000u) {{"));
        } else {
            self.line(&format!("uint tb_pc = {entry}u; [loop] while (tb_pc != {n}u) {{"));
        }
        self.loop_depth += 1;
        // Flat `if (tb_sel == K)` tests on a snapshot of the block id (FXC mishandles a
        // `switch` whose selector is assigned inside its cases).
        if barriers.is_empty() {
            self.line("uint tb_sel = tb_pc; tb_pc = 0xffffffffu;");
        } else {
            let at = barriers.iter().map(|i| format!("tb_pc == {i}u")).collect::<Vec<_>>().join(" || ");
            self.line(&format!("bool tb_wait = {at};"));
            // When lanes stand at different barriers the earliest goes first.
            self.line("bool tb_busy = WaveActiveAnyTrue(!tb_wait); uint tb_low = WaveActiveMin(tb_pc);");
            self.line("tb_wait = tb_wait && (tb_busy || tb_pc != tb_low);");
            self.line("uint tb_sel = tb_wait ? 0xfffffffeu : tb_pc; if (!tb_wait) tb_pc = 0xffffffffu;");
        }
        let insns = self.insns.clone();
        for (b, &start) in starts.iter().enumerate() {
            let end = starts.get(b + 1).copied().unwrap_or(n);
            self.line(&format!("if (tb_sel == {start}u) {{"));
            self.drop_heap_handles();
            for i in start..end {
                let ins = &insns[i];
                for o in ins.src.iter().chain(ins.dst.iter()) {
                    self.touch(o);
                }
                if is_quad_exchange(ins) && self.pixel_wave && !is_row_move(ins) {
                    self.emit_pixel_quad_exchange(i, ins)?;
                }
                let is_ctl = ins.enc == Enc::Sopp && (Self::branch_cond(ins.op).is_some() || ins.op == 1);
                if !is_ctl || ins.mnemonic == "s_swappc_b64" {
                    self.emit_insn(i, ins, is_ps, None)
                        .map_err(|e| format!("{e} [{} @ {:#x}]", ins.mnemonic, ins.offset))?;
                }
            }
            let last = &insns[end - 1];
            let next = if last.enc == Enc::Sopp && last.op == 1 {
                format!("{n}u")
            } else if let (Some(c), Some(&t)) = (
                if last.enc == Enc::Sopp { Self::branch_cond(last.op) } else { None },
                tgt.get(&(end - 1)),
            ) {
                if self.wave_cull { wave_cull::prepare_insn(self, end - 1, last)?; }
                let c = wave_cull::branch_condition(self, last, c);
                if last.op == 2 { format!("{t}u") } else { format!("({c}) ? {t}u : {end}u") }
            } else {
                format!("{end}u")
            };
            self.line(&format!("tb_pc = {next}; }}"));
        }
        // No block matched (cannot happen for a well-formed program): stop.
        self.line(&format!("if (tb_pc == 0xffffffffu) tb_pc = {n}u;"));
        self.line("}");
        self.drop_heap_handles();
        self.loop_depth = self.loop_depth.saturating_sub(1);
        Ok(())
    }

    /// Float source with VOP3 abs/neg modifiers applied.
    fn fm(&self, ins: &Insn, i: usize) -> Result<String, String> {
        let mask = if ins.enc == Enc::Vop3 { ins.aux.mask } else { 0 };
        Ok(Self::float_source(f(&ins.src[i])?, mask, i))
    }

    fn select_source(ins: &Insn, mut bits: String, source: u8) -> String {
        if ins.enc == Enc::Vop3 {
            if ins.aux.mask & (1 << source) != 0 { bits = format!("({bits} & 0x7fffffffu)"); }
            if ins.aux.mask & (8 << source) != 0 { bits = format!("({bits} ^ 0x80000000u)"); }
        }
        bits
    }

    fn float_source(mut expr: String, mask: u8, i: usize) -> String {
        if mask & (1 << i) != 0 {
            expr = format!("abs({expr})");
        }
        if mask & (8 << i) != 0 {
            expr = format!("(-{expr})");
        }
        expr
    }

    fn float_result(ins: &Insn, mut e: String) -> String {
        if ins.enc == Enc::Vop3 {
            e = match ins.aux.imm2 {
                1 => format!("(({e}) * 2.0)"),
                2 => format!("(({e}) * 4.0)"),
                3 => format!("(({e}) * 0.5)"),
                _ => e,
            };
            if ins.aux.imm != 0 {
                e = format!("saturate({e})");
            }
        }
        e
    }

    fn fres(&self, ins: &Insn, e: String) -> String {
        format!("asuint({})", Self::float_result(ins, e))
    }

    fn pair_high(&mut self, operand: &Opnd) -> Result<String, String> {
        let high = match operand {
            Opnd::Vgpr(n) => Opnd::Vgpr(n.checked_add(1).ok_or("VGPR pair overflows")?),
            Opnd::Sgpr(0..=104) | Opnd::ExecLo | Opnd::VccLo | Opnd::ConstInt(_) | Opnd::Null => hi(operand),
            _ => return Err("unsupported 64-bit operand pair".into()),
        };
        self.touch(&high);
        self.numeric_input(&high)
    }

    fn numeric_input(&self, operand: &Opnd) -> Result<String, String> {
        match self.exec_snapshots.word(self.cur_idx, operand) {
            Some(word) => Ok(word),
            None => r(operand),
        }
    }

    fn int_compare(&self, m: &str, a: &Opnd, b: &Opnd) -> Result<String, String> {
        let mut parts = m.rsplit('_');
        let ty = parts.next().unwrap_or("");
        let cc = parts.next().unwrap_or("");
        let (x, y) = match ty {
            "i16" | "u16" => {
                let half = |o: &Opnd| -> Result<String, String> {
                    if matches!(o, Opnd::ConstF32(_)) {
                        return Err("inline float constant in a 16-bit integer compare".into());
                    }
                    let bits = self.numeric_input(o)?;
                    Ok(if ty == "i16" {
                        format!("(asint({bits} << 16) >> 16)")
                    } else {
                        format!("({bits} & 0xffffu)")
                    })
                };
                (half(a)?, half(b)?)
            }
            "i32" => (format!("asint({})", self.numeric_input(a)?), format!("asint({})", self.numeric_input(b)?)),
            "u32" => (self.numeric_input(a)?, self.numeric_input(b)?),
            other => return Err(format!("compare type {other} unsupported")),
        };
        Ok(match cc {
            "f" => "false".into(),
            "lt" => format!("({x} < {y})"),
            "eq" => format!("({x} == {y})"),
            "le" => format!("({x} <= {y})"),
            "gt" => format!("({x} > {y})"),
            "lg" | "ne" => format!("({x} != {y})"),
            "ge" => format!("({x} >= {y})"),
            "tru" | "t" => "true".into(),
            other => return Err(format!("compare cond {other} unsupported")),
        })
    }

    fn emit_insn(&mut self, idx: usize, ins: &Insn, is_ps: bool, br: Option<usize>) -> Result<(), String> {
        self.emit_insn_body(idx, ins, is_ps, br)?;
        if let Some(vote) = self.lockstep.as_ref().and_then(|votes| votes.get(&idx)).cloned() {
            let mask = r(&vote.mask)?;
            let not = if vote.none { "!" } else { "" };
            self.line(&format!("scc = uint({not}WaveActiveAnyTrue(({mask} & 1u) != 0u));"));
        }
        if self.vote_defs.contains(&idx) {
            let mask = r(&ins.dst[0])?;
            self.line(&format!("tb_votes{idx} = WaveActiveCountBits(({mask} & 1u) != 0u);"));
        }
        Ok(())
    }

    fn emit_insn_body(&mut self, idx: usize, ins: &Insn, is_ps: bool, br: Option<usize>) -> Result<(), String> {
        let m = ins.mnemonic.as_str();
        let s = &ins.src;
        let d = ins.dst.first().cloned();
        self.line(&format!("// {:#06x} {m}", ins.offset));
        self.cur_idx = idx;
        if let Some(&site) = self.exec_snapshots.sites.get(&idx) {
            if site.emit(self, idx, ins)? { return Ok(()); }
        }
        if m == "tb_reflection_far_query" {
            reflection_far::emit(self, ins);
            return Ok(());
        }
        if m == "tb_radiance_query" {
            radiance_probe::emit(self, ins);
            return Ok(());
        }
        if let Some(reg) = self.packed.as_ref().and_then(|p| p.ballot_nonempty.get(&idx)) {
            self.line(&format!("s{reg} &= exec_lo; scc = uint(WaveActiveAnyTrue((s{reg} & 1u) != 0u));"));
            return Ok(());
        }
        if ins.enc == Enc::Ds && m != "ds_swizzle_b32" && !self.hoist_wave_exchanges && wave_cull::lds_words(self).is_some() {
            // Publish writes and finish reads before the next guest LDS access.
            self.line("GroupMemoryBarrierWithGroupSync();");
        }
        if let Some(width) = self.fspx_wave {
            if fspx_wave::emit(self, idx, ins, width)? { return Ok(()); }
        }
        if self.wave_cull {
            wave_cull::prepare_insn(self, idx, ins)?;
            if wave_cull::emit(self, idx, ins, br)? { return Ok(()); }
        }
        if self.page_waves && page_waves::emit(self, idx, ins, br)? { return Ok(()); }
        match m {
            "tb_light_grid_reserve" => {
                self.line(light_grid::RESERVE);
                return Ok(());
            }
            "tb_first_exec_lane" => return self.ws(&d.ok_or("missing election result")?, &format!("election{idx}")),
            "tb_indexed_readlane" => {
                if self.hoisted_swizzle != Some(idx) { self.emit_indexed_read(idx, ins)?; }
                self.ws(&d.ok_or("missing lane-read destination")?, &format!("readlane{idx}"))?;
                return Ok(());
            }
            "tb_view_occluded" => {
                self.line("hidden = (exec_lo & 1u) != 0u; hidden_id = v0; hidden_view = cull_view;");
                return Ok(());
            }
            "tb_view_candidate" => {
                self.line("cull_candidate = (exec_lo & 1u) != 0u;");
                self.line(&format!("cull_record = uint3({}, {}, {});", r(&ins.src[0])?, r(&ins.src[1])?, r(&ins.src[2])?));
                return Ok(());
            }
            "s_ff1_i32_b64" | "s_ff1_i32_b32" | "tb_first_active_lane" if lone_lane::on() && ins.src == [Opnd::ExecLo] => {
                return self.ws(&d.ok_or("missing elected lane register")?, "((exec_lo & 1u) ? 0u : 0xffffffffu)");
            }
            "s_ff1_i32_b64" if ins.src == [Opnd::ExecLo] => {
                // WaveActiveMin returns the same lane ID to all participating invocations.
                let dst = d.ok_or("missing elected lane register")?;
                return self.ws(&dst, "WaveActiveMin((exec_lo & 1u) ? WaveGetLaneIndex() : 0xffffffffu)");
            }
            "tb_subsurface_group_sync" => {
                self.line("GroupMemoryBarrierWithGroupSync();");
                return Ok(());
            }
            "tb_first_active_lane" => {
                let dst = d.ok_or("missing elected lane register")?;
                return self.ws(&dst, "WaveActiveMin((exec_lo & 1u) ? WaveGetLaneIndex() : 0xffffffffu)");
            }
            "tb_capture_allocation_mask32" => {
                let dst = d.ok_or("missing mask register")?;
                return self.ws(&dst, "WaveActiveBallot((exec_lo & 1u) != 0u).x");
            }
            "tb_capture_allocation_mask" => {
                let dst = d.ok_or("missing mask register")?;
                let upper = hi(&dst);
                self.touch(&upper);
                self.line(&format!("uint4 alloc_mask{idx} = WaveActiveBallot((exec_lo & 1u) != 0u);"));
                self.ws(&dst, &format!("alloc_mask{idx}.x"))?;
                return self.ws(&upper, &format!("alloc_mask{idx}.y"));
            }
            "tb_allocation_prefix_lo" | "tb_allocation_prefix_hi" => {
                let high = m.ends_with("_hi");
                let lane = if high { "max(32u, WaveGetLaneIndex()) - 32u" } else { "WaveGetLaneIndex()" };
                let word = if high { "y" } else { "x" };
                let prefix = format!("countbits(WaveActiveBallot((exec_lo & 1u) != 0u).{word} & {})", low_mask(lane));
                return self.wv(&d.ok_or("missing prefix destination")?, &format!("({} + {prefix})", r(&s[1])?));
            }
            _ => {}
        }
        if wave_alloc::emit(self, ins)? { return Ok(()); }
        if hair_cluster::emit(self, ins) { return Ok(()); }
        if hair_cluster_export::emit(self, ins) { return Ok(()); }
        if m == "tb_hidden_instance" {
            self.line("hidden = (exec_lo & 1u) != 0u; hidden_id = v0; if (hidden) v42 = v28;");
            return Ok(());
        }
        if instance_cull::emit(self, ins)? { return Ok(()); }
        if ps09_classify::emit(self, ins) { return Ok(()); }
        if let Some(tile) = self.lds_tile {
            if tile.emit(self, idx)? { return Ok(()); }
        }
        if let Some(&id) = self.epilog_pack.get(&idx) {
            // Export epilogue: keep the unpacked operands (see `find_export_epilogs`).
            for (k, sfx) in ["a", "b"].iter().enumerate() {
                let o = *s.get(k).ok_or("pack without two operands")?;
                self.touch(&o);
                self.regs.insert(format!("tb_pk{id}{sfx}"));
                self.line(&format!("tb_pk{id}{sfx} = {};", r(&o)?));
            }
        }
        // Run-time descriptor selectors (only for shaders with a "one of N" image op).
        if let Some((reg, cands)) = self.bsel.remove(&idx) {
            // One guarded copy of the op per candidate buffer.
            self.regs.insert(format!("tb_dsel{reg}"));
            let mut first = true;
            for (id, res) in &cands {
                self.line(&format!("{}if (tb_dsel{reg} == {id:#x}u) {{", if first { "" } else { "else " }));
                first = false;
                self.res.insert(idx, vec![*res]);
                self.emit_insn(idx, ins, is_ps, br)?;
                self.line("}");
                self.drop_heap_handles();
            }
            self.bsel.insert(idx, (reg, cands));
            return Ok(());
        }
        if !self.msel.is_empty() || !self.bsel.is_empty() {
            if let Some(&(d, table, reg, imm)) = self.dsel_runtime.get(&idx) {
                let offset = Opnd::Sgpr(reg);
                self.touch(&offset);
                self.regs.insert(format!("tb_dsel{d}"));
                self.line(&format!("tb_dsel{d} = {:#x}u | ({} + {imm:#x}u);",
                    u32::from(table) << 24, r(&offset)?));
            }
            if let Some(&(d, id)) = self.dsel_loads.get(&idx) {
                self.regs.insert(format!("tb_dsel{d}"));
                self.line(&format!("tb_dsel{d} = {id:#x}u;"));
            }
            if let Some(&(d, s)) = self.dsel_moves.get(&idx) {
                self.regs.insert(format!("tb_dsel{d}"));
                self.regs.insert(format!("tb_dsel{s}"));
                self.line(&format!("tb_dsel{d} = tb_dsel{s};"));
            }
        }
        if ins.enc == Enc::Mimg {
            if let Some(sel) = self.msel.get(&idx).cloned() {
                // One guarded copy of the op per (texture, sampler) candidate pair.
                self.regs.insert(format!("tb_dsel{}", sel.tex_reg));
                self.regs.insert(format!("tb_dsel{}", sel.samp_reg));
                let samps: Vec<Option<(u32, Res)>> =
                    if sel.samp.is_empty() { vec![None] } else { sel.samp.iter().copied().map(Some).collect() };
                let mut first = true;
                for (tid, tres) in &sel.tex {
                    for s in &samps {
                        let mut cond = vec![];
                        if sel.tex.len() > 1 {
                            cond.push(format!("tb_dsel{} == {tid:#x}u", sel.tex_reg));
                        }
                        let mut v = vec![*tres];
                        if let Some((sid, sres)) = s {
                            if sel.samp.len() > 1 {
                                cond.push(format!("tb_dsel{} == {sid:#x}u", sel.samp_reg));
                            }
                            v.push(*sres);
                        }
                        self.line(&format!("{}if ({}) {{", if first { "" } else { "else " }, cond.join(" && ")));
                        first = false;
                        self.res.insert(idx, v);
                        self.emit_mimg(idx, ins)?;
                        self.line("}");
                        self.drop_heap_handles();
                    }
                }
                return Ok(());
            }
        }
        if scalar_bits::emit(self, idx, ins)? { return Ok(()); }
        match ins.enc {
            Enc::Sopp if matches!(ins.op, 0x24 | 0x25) => {
                // MODE[7:0] exposes the guest round and denorm settings to S_GETREG.
                // V_CVT_F16_F32 also consumes these settings.
                let shift = if ins.op == 0x24 { 0 } else { 4 };
                self.regs.insert("tb_fp_mode".into());
                self.line(&format!("tb_fp_mode = (tb_fp_mode & ~{:#x}u) | {:#x}u;", 15 << shift, (ins.raw[0] & 15) << shift));
                Ok(())
            }
            Enc::Sopp if ins.op == 0x10 => self.emit_sendmsg(ins),
            Enc::Sopp if ins.op == 0x0a && self.is_cs => {
                if self.packed.as_ref().is_some_and(|p| p.device_sync_barriers.contains(&idx)) {
                    self.line("AllMemoryBarrierWithGroupSync();");
                } else {
                    self.line("GroupMemoryBarrierWithGroupSync();");
                }
                Ok(())
            }
            Enc::Sopp => {
                if m == "s_swappc_b64" {
                    let link = d.ok_or("missing local PC link register")?;
                    self.ws(&link, &format!("{:#x}u", ins.offset + 4))?;
                    self.ws(&hi(&link), "0u")?;
                }
                let cond = match ins.op {
                    2 => Some("true".to_string()),
                    4 => Some("scc == 0u".into()),
                    5 => Some("scc != 0u".into()),
                    6 => Some("(vcc_lo & 1u) == 0u".into()),
                    7 => Some("(vcc_lo & 1u) != 0u".into()),
                    // The recorded guard reaches every invocation before the
                    // full-mask reduction, including guest-inactive lanes.
                    8 if self.reduction_execz_guards.contains(&idx) =>
                        Some("!WaveActiveAnyTrue((exec_lo & 1u) != 0u)".into()),
                    8 => Some("(exec_lo & 1u) == 0u".into()),
                    9 => Some("(exec_lo & 1u) != 0u".into()),
                    // A non-final s_endpgm ends the program on this path: jump to the end label.
                    1 if br.is_some() => Some("true".to_string()),
                    0 | 1 | 0x0a | 0x0c | 0x1f | 0x20 | 0x21 | 0x23 | 0x24 | 0x25 => None,
                    _ => return Err("unsupported SOPP".into()),
                };
                if let (Some(c), Some(t)) = (cond, br) {
                    let c = if self.resolve_wave32 || self.lockstep.is_some() {
                        wave_cull::branch_condition(self, ins, &c)
                    } else { c };
                    self.line(&format!("if ({c}) j{t} = true;"));
                }
                Ok(())
            }
            Enc::Vop2 | Enc::Vop1 | Enc::Vop3 | Enc::Vopc => self.emit_vec(ins),
            Enc::Sop2 => {
                let d = d.ok_or("no dst")?;
                let (a, b) = (r(&s[0])?, r(&s[1])?);
                if matches!(m, "s_lshr_b64" | "s_ashr_i64" | "s_bfe_u64" | "s_bfe_i64") {
                    self.touch(&hi(&s[0]));
                    let upper = r(&hi(&s[0]))?;
                    let t = self.tmp;
                    self.tmp += 1;
                    let signed = m.ends_with("i64");
                    let call = if m.starts_with("s_bfe") { "tb_field64" } else { "tb_shr64" };
                    self.line(&format!("uint2 t{t} = {call}({a}, {upper}, {b}, {signed}); scc = any(t{t} != 0u);"));
                    self.ws(&d, &format!("t{t}.x"))?;
                    return self.ws(&hi(&d), &format!("t{t}.y"));
                }
                if m == "s_bfm_b64" || m == "s_cselect_b64" || m == "s_lshl_b64" {
                    let t = self.tmp;
                    self.tmp += 1;
                    let (lo, hi_e) = if m == "s_lshl_b64" {
                        self.touch(&hi(&s[0]));
                        let ah = r(&hi(&s[0]))?;
                        (
                            format!("((({b} & 63u) >= 32u) ? 0u : ({a} << ({b} & 31u)))"),
                            format!(
                                "((({b} & 63u) == 0u) ? {ah} : ((({b} & 63u) >= 32u) ? ({a} << (({b} & 63u) - 32u)) : (({ah} << ({b} & 31u)) | ({a} >> (32u - ({b} & 31u))))))"
                            ),
                        )
                    } else if m == "s_bfm_b64" {
                        // Intersect [start, end) with each 32-bit half of the destination.
                        self.line(&format!("uint t{t}start = {b} & 63u, t{t}end = t{t}start + ({a} & 63u);"));
                        let word_mask = |base: u32| {
                            let start = low_mask(&format!("max(t{t}start, {base}u) - {base}u"));
                            let end = low_mask(&format!("max(t{t}end, {base}u) - {base}u"));
                            format!("({end} & ~{start})")
                        };
                        (word_mask(0), word_mask(32))
                    } else {
                        self.touch(&hi(&s[0]));
                        self.touch(&hi(&s[1]));
                        (
                            format!("((scc != 0u) ? {a} : {b})"),
                            format!("((scc != 0u) ? {} : {})", r(&hi(&s[0]))?, r(&hi(&s[1]))?),
                        )
                    };
                    self.line(&format!("uint t{t}l = {lo}; uint t{t}h = {hi_e};"));
                    if m == "s_lshl_b64" {
                        self.line(&format!("scc = ((t{t}l | t{t}h) != 0u) ? 1u : 0u;"));
                    }
                    self.ws(&d, &format!("t{t}l"))?;
                    let dh = hi(&d);
                    self.touch(&dh);
                    return self.ws(&dh, &format!("t{t}h"));
                }
                let is64 = m.ends_with("_b64");
                let bit = |op: &str, a: &str, b: &str| -> Option<String> {
                    Some(match op {
                        "and" => format!("({a} & {b})"),
                        "or" => format!("({a} | {b})"),
                        "xor" => format!("({a} ^ {b})"),
                        "andn2" => format!("({a} & ~{b})"),
                        "orn2" => format!("({a} | ~{b})"),
                        "nand" => format!("(~({a} & {b}))"),
                        "nor" => format!("(~({a} | {b}))"),
                        "xnor" => format!("(~({a} ^ {b}))"),
                        _ => return None,
                    })
                };
                let opname = m.trim_start_matches("s_").trim_end_matches("_b32").trim_end_matches("_b64");
                if let Some(lo) = bit(opname, &a, &b) {
                    if is64 {
                        self.touch(&hi(&s[0]));
                        self.touch(&hi(&s[1]));
                        let (ah, bh) = (r(&hi(&s[0]))?, r(&hi(&s[1]))?);
                        let hi_e = bit(opname, &ah, &bh).unwrap();
                        let t = self.tmp;
                        self.tmp += 1;
                        self.line(&format!("uint t{t}l = {lo}; uint t{t}h = {hi_e};"));
                        self.ws(&d, &format!("t{t}l"))?;
                        let dh = hi(&d);
                        self.touch(&dh);
                        self.ws(&dh, &format!("t{t}h"))?;
                        self.line(&format!("scc = ((t{t}l | t{t}h) != 0u) ? 1u : 0u;"));
                        if is_ps && m == "s_andn2_b64" && pixel_quad::guards_expansion(&self.insns, idx) {
                            self.line("#if __SHADER_TARGET_MAJOR >= 6");
                            self.line(&pixel_quad::vote(t, &format!("t{t}l")));
                            self.line(&format!("scc = q{t} | (uint)WaveActiveAnyTrue(q{t} != 0u);"));
                            self.line("#endif");
                        }
                    } else {
                        self.ws(&d, &lo)?;
                        self.line(&format!("scc = ({} != 0u) ? 1u : 0u;", name(&d)?));
                    }
                    return Ok(());
                }
                let e = match m {
                    "s_add_u32" | "s_add_i32" => format!("({a} + {b})"),
                    "s_sub_u32" | "s_sub_i32" => format!("({a} - {b})"),
                    "s_addc_u32" => format!("({a} + {b} + (scc & 1u))"),
                    "s_subb_u32" => format!("({a} - {b} - (scc & 1u))"),
                    "s_mul_i32" => format!("({a} * {b})"),
                    // The subtraction wraps before taking the absolute value, including INT_MIN.
                    "s_absdiff_i32" => format!("(asint({a} - {b}) < 0 ? ({b} - {a}) : ({a} - {b}))"),
                    "s_mul_hi_u32" => format!("tb_mulhi_u32({a}, {b})"),
                    "s_mul_hi_i32" => format!("tb_mulhi_i32({a}, {b})"),
                    "s_lshl_b32" => format!("({a} << ({b} & 31u))"),
                    "s_lshr_b32" => format!("({a} >> ({b} & 31u))"),
                    "s_ashr_i32" => format!("asuint(asint({a}) >> ({b} & 31u))"),
                    "s_min_u32" => format!("min({a}, {b})"),
                    "s_max_u32" => format!("max({a}, {b})"),
                    "s_min_i32" => format!("asuint(min(asint({a}), asint({b})))"),
                    "s_max_i32" => format!("asuint(max(asint({a}), asint({b})))"),
                    "s_cselect_b32" => format!("((scc != 0u) ? {a} : {b})"),
                    "s_bfe_u32" => format!("(({a} >> ({b} & 31u)) & {})", low_mask(&format!("({b} >> 16) & 127u"))),
                    "s_bfe_i32" => format!("tb_bfe_i32({a}, {b})"),
                    "s_bfm_b32" => format!("(((1u << ({a} & 31u)) - 1u) << ({b} & 31u))"),
                    "s_pack_ll_b32_b16" => format!("(({a} & 65535u) | ({b} << 16))"),
                    "s_pack_lh_b32_b16" => format!("(({a} & 65535u) | ({b} & 0xffff0000u))"),
                    "s_pack_hh_b32_b16" => format!("(({a} >> 16) | ({b} & 0xffff0000u))"),
                    "s_lshl1_add_u32" => format!("(({a} << 1) + {b})"),
                    "s_lshl2_add_u32" => format!("(({a} << 2) + {b})"),
                    "s_lshl3_add_u32" => format!("(({a} << 3) + {b})"),
                    "s_lshl4_add_u32" => format!("(({a} << 4) + {b})"),
                    _ => return Err("unsupported SOP2".into()),
                };
                let t = self.tmp;
                self.tmp += 1;
                let result = format!("t{t}");
                self.line(&format!("uint {result} = {e};"));
                // SCC may depend on both original sources even when SDST aliases one of them.
                let scc = match m {
                    "s_add_i32" => Some(signed_add_overflow(&a, &b, &result)),
                    "s_sub_i32" => Some(format!("((({a} ^ {b}) & ({a} ^ {result})) >> 31)")),
                    // The carry is of the full sum, so bits the shift moves past
                    // bit 31 set it as well as a wrapped addition.
                    "s_lshl1_add_u32" => Some(shift_add_carry(&a, &b, &result, 1)),
                    "s_lshl2_add_u32" => Some(shift_add_carry(&a, &b, &result, 2)),
                    "s_lshl3_add_u32" => Some(shift_add_carry(&a, &b, &result, 3)),
                    "s_lshl4_add_u32" => Some(shift_add_carry(&a, &b, &result, 4)),
                    "s_add_u32" => Some(format!("(({a} + {b}) < {a}) ? 1u : 0u")),
                    "s_addc_u32" => Some(format!("((({a} + {b}) < {a}) || (({a} + {b} + (scc & 1u)) < ({a} + {b}))) ? 1u : 0u")),
                    "s_subb_u32" => Some(format!("(({b} > {a}) || ((scc & 1u) > ({a} - {b}))) ? 1u : 0u")),
                    "s_sub_u32" => Some(format!("({b} > {a}) ? 1u : 0u")),
                    "s_min_i32" => Some(format!("(asint({a}) < asint({b})) ? 1u : 0u")),
                    "s_max_i32" => Some(format!("(asint({a}) > asint({b})) ? 1u : 0u")),
                    "s_min_u32" => Some(format!("({a} < {b}) ? 1u : 0u")),
                    "s_max_u32" => Some(format!("({a} > {b}) ? 1u : 0u")),
                    "s_lshl_b32" | "s_lshr_b32" | "s_ashr_i32" | "s_bfe_u32" | "s_bfe_i32" | "s_absdiff_i32" => Some(format!("({result} != 0u) ? 1u : 0u")),
                    _ => None,
                };
                if let Some(flag) = scc {
                    self.line(&format!("scc = {flag};"));
                }
                self.ws(&d, &result)
            }
            Enc::Sop1 => {
                let d = d.ok_or("no dst")?;
                let a = r(&s[0])?;
                match m {
                    "s_mov_b32" => self.ws(&d, &a),
                    GROUP_READ => {
                        if self.hoisted_swizzle != Some(idx) { self.emit_compute_swizzle(idx, ins, &[])?; }
                        self.ws(&d, &format!("swizzle{idx}"))
                    }
                    ray_resolve::READ_LANE => {
                        let lane = r(&s[1])?;
                        self.ws(&d, &format!("WaveReadLaneAt({a}, ({lane}) & 31u)"))
                    }
                    // D[S0[4:0]] = 0 / 1
                    "s_bitset0_b32" => {
                        self.touch(&d);
                        let cur = r(&d)?;
                        self.ws(&d, &format!("({cur} & ~(1u << (({a}) & 31u)))"))
                    }
                    "s_bitset1_b32" => {
                        self.touch(&d);
                        let cur = r(&d)?;
                        self.ws(&d, &format!("({cur} | (1u << (({a}) & 31u)))"))
                    }
                    "s_wqm_b32" | "s_wqm_b64" => {
                        // For an ordinary scalar mask, replicate each nonempty nibble.
                        // EXEC instead holds one guest bit per invocation.
                        let t = self.tmp;
                        self.tmp += 1;
                        let wide = m.ends_with("_b64");
                        let lane_mask = is_ps && pixel_quad::saved_exec(&self.insns, idx);
                        if s[0] == Opnd::ExecLo && lone_lane::on() {
                            self.line(&format!("uint t{t}l = exec_lo & 1u, t{t}h = 0u;"));
                        } else if s[0] == Opnd::ExecLo && !self.resolve_wave32 {
                            self.line("#if __SHADER_TARGET_MAJOR >= 6");
                            self.line(&format!("uint4 t{t}ballot = WaveActiveBallot((exec_lo & 1u) != 0u);"));
                            self.line(&format!("uint t{t}lane = WaveGetLaneIndex(); uint t{t}l = ((t{t}ballot[t{t}lane >> 5] >> (t{t}lane & 28u)) & 15u) != 0u;"));
                            self.line("#else");
                            self.line(&format!("uint4 t{t}ballot = uint4(exec_lo,0,0,0); uint t{t}l = exec_lo & 1u;"));
                            self.line("#endif");
                            self.line(&format!("uint t{t}h = 0u;"));
                        } else if lane_mask {
                            self.line("#if __SHADER_TARGET_MAJOR >= 6");
                            self.line(&pixel_quad::vote(t, &a));
                            self.line(&format!("uint t{t}l = q{t}, t{t}h = 0u;"));
                            self.line(&format!("scc = q{t} | (uint)WaveActiveAnyTrue(q{t} != 0u);"));
                            self.line("#else");
                            self.line(&format!("uint t{t}l = {a} & 1u, t{t}h = 0u; scc = t{t}l;"));
                            self.line("#endif");
                        } else {
                            let expand = |word: &str| format!("((({word} | ({word} >> 1) | ({word} >> 2) | ({word} >> 3)) & 0x11111111u) * 15u)");
                            self.line(&format!("uint t{t}l = {};", expand(&a)));
                            let upper = if wide {
                                self.touch(&hi(&s[0]));
                                expand(&r(&hi(&s[0]))?)
                            } else { "0u".into() };
                            self.line(&format!("uint t{t}h = {upper};"));
                        }
                        if s[0] == Opnd::ExecLo && !self.resolve_wave32 && !lone_lane::on() {
                            self.line(&format!("scc = any(t{t}ballot != 0u);"));
                        } else if !lane_mask {
                            self.line(&format!("scc = ((t{t}l | t{t}h) != 0u);"));
                        }
                        self.ws(&d, &format!("t{t}l"))?;
                        if wide { self.ws(&hi(&d), &format!("t{t}h"))?; }
                        Ok(())
                    }
                    "s_mov_b64" | "s_cmov_b64" | "s_not_b64" => {
                        self.touch(&hi(&s[0]));
                        let ah = r(&hi(&s[0]))?;
                        let t = self.tmp;
                        self.tmp += 1;
                        let (lo, upper) = match m {
                            "s_not_b64" => (format!("~{a}"), format!("~{ah}")),
                            "s_cmov_b64" => (format!("scc ? {a} : {}", r(&d)?), format!("scc ? {ah} : {}", r(&hi(&d))?)),
                            _ => (a, ah),
                        };
                        self.line(&format!("uint t{t}l = {lo}; uint t{t}h = {upper};"));
                        if m == "s_not_b64" { self.line(&format!("scc = ((t{t}l | t{t}h) != 0u);")); }
                        self.ws(&d, &format!("t{t}l"))?;
                        let dh = hi(&d);
                        self.touch(&dh);
                        self.ws(&dh, &format!("t{t}h"))
                    }
                    "s_not_b32" => {
                        self.ws(&d, &format!("(~{a})"))?;
                        self.line(&format!("scc = ({} != 0u) ? 1u : 0u;", name(&d)?));
                        Ok(())
                    }
                    "s_cmov_b32" => self.ws(&d, &format!("((scc != 0u) ? {a} : {})", name(&d)?)),
                    "s_brev_b32" => self.ws(&d, &format!("reversebits({a})")),
                    COUNT_LANES => {
                        let def = *self.vote_uses.get(&idx).ok_or("lane count without its predicate")?;
                        self.ws(&d, &format!("tb_votes{def}"))?;
                        self.line(&format!("scc = ({} != 0u) ? 1u : 0u;", name(&d)?));
                        Ok(())
                    }
                    "s_bcnt1_i32_b64" => {
                        self.touch(&hi(&s[0]));
                        // Population count of a 64-bit value — typically the number of active
                        // lanes (wave-level allocation). One invocation = one lane, so a lane
                        // mask counts as 0 or 1.
                        let ah = r(&hi(&s[0]))?;
                        self.ws(&d, &format!("(countbits({a}) + countbits({ah}))"))?;
                        self.line(&format!("scc = ({} != 0u) ? 1u : 0u;", name(&d)?));
                        Ok(())
                    }
                    "s_bcnt1_i32_b32" => {
                        self.ws(&d, &format!("countbits({a})"))?;
                        self.line(&format!("scc = ({} != 0u) ? 1u : 0u;", name(&d)?));
                        Ok(())
                    }
                    "s_flbit_i32_b32" => self.ws(&d, &format!("({a} == 0u ? ~0u : 31u - firstbithigh({a}))")),
                    "s_flbit_i32_b64" => {
                        self.touch(&hi(&s[0]));
                        let ah = r(&hi(&s[0]))?;
                        self.ws(&d, &flbit_b64_expr(&a, &ah))
                    }
                    "s_ff0_i32_b32" => self.ws(&d, &ff0_b32_expr(&a)),
                    "s_ff0_i32_b64" => {
                        self.touch(&hi(&s[0]));
                        let ah = r(&hi(&s[0]))?;
                        self.ws(&d, &ff0_b64_expr(&a, &ah))
                    }
                    "s_ff1_i32_b32" => self.ws(&d, &format!("firstbitlow({a})")),
                    // s_ff1_i32_b64 is firstbitlow over the 64-bit value: the low word
                    // decides when it is nonzero, otherwise the high word.
                    "s_ff1_i32_b64" => {
                        self.touch(&hi(&s[0]));
                        let ah = r(&hi(&s[0]))?;
                        self.ws(&d, &ff1_b64_expr(&a, &ah))
                    }
                    "s_sext_i32_i8" => self.ws(&d, &format!("asuint(asint({a} << 24) >> 24)")),
                    "s_sext_i32_i16" => self.ws(&d, &format!("asuint(asint({a} << 16) >> 16)")),
                    "s_abs_i32" => {
                        self.ws(&d, &format!("asuint(abs(asint({a})))"))?;
                        self.line(&format!("scc = ({} != 0u) ? 1u : 0u;", name(&d)?));
                        Ok(())
                    }
                    "s_getpc_b64" => {
                        // Address of the next instruction, relative to the program start.
                        self.ws(&d, &format!("{:#x}u", ins.offset as u32 + 4))?;
                        let dh = hi(&d);
                        self.touch(&dh);
                        self.ws(&dh, "0u")
                    }
                    "s_swappc_b64" if self.ex.stage == Stage::Vertex => {
                        // Vertex-fetch shader call: inputs are materialised from the D3D input layout.
                        self.line("// (fetch-shader call: vertex attributes come from the input layout)");
                        Ok(())
                    }
                    _ if m.contains("_saveexec_") || m.contains("_wrexec_") => {
                        let wide = m.ends_with("_b64");
                        let combine = |src: &str, exec: &str| -> Result<String, String> {
                            Ok(match m.split('_').nth(1) {
                                Some("and") => format!("{src} & {exec}"),
                                Some("or") => format!("{src} | {exec}"),
                                Some("xor") => format!("{src} ^ {exec}"),
                                Some("nand") => format!("~({src} & {exec})"),
                                Some("andn2") => format!("{src} & ~{exec}"),
                                Some("orn2") => format!("{src} | ~{exec}"),
                                Some("andn1") => format!("~{src} & {exec}"),
                                _ => return Err("unsupported saveexec".into()),
                            })
                        };
                        let t = self.tmp;
                        self.tmp += 1;
                        self.line(&format!("uint t{t}l = {};", combine(&a, "exec_lo")?));
                        if wide {
                            self.touch(&hi(&s[0]));
                            self.line(&format!("uint t{t}h = {};", combine(&r(&hi(&s[0]))?, "exec_hi")?));
                        }
                        let (lo, high) = if m.contains("_wrexec_") {
                            (format!("t{t}l"), format!("t{t}h"))
                        } else {
                            ("exec_lo".into(), "exec_hi".into())
                        };
                        self.ws(&d, &lo)?;
                        if wide {
                            self.ws(&hi(&d), &high)?;
                            self.line(&format!("exec_hi = t{t}h;"));
                        }
                        let mask = if wide { "(exec_lo | exec_hi)" } else { "exec_lo" };
                        self.line(&format!("exec_lo = t{t}l; scc = ({mask} != 0u);"));
                        Ok(())
                    }
                    // Return from a vertex shader chained in front of a geometry shader: the
                    // program ends here (it is followed directly by s_endpgm).
                    "s_setpc_b64" if self.ex.stage == Stage::Vertex && idx + 2 >= self.insns.len() => {
                        self.line("// (return to the chained stage)");
                        Ok(())
                    }
                    _ => Err("unsupported SOP1".into()),
                }
            }
            Enc::Sopk => {
                let d = d.ok_or("no dst")?;
                let imm = format!("{:#x}u", ins.aux.imm as i32 as u32);
                let mask = modeled_hwreg_mask(ins.raw[0]);
                match m {
                    "s_version" => Ok(()),
                    "s_movk_i32" => self.ws(&d, &imm),
                    "s_cmovk_i32" => self.ws(&d, &format!("((scc != 0u) ? {imm} : {})", name(&d)?)),
                    "s_setreg_imm32_b32" | "s_setreg_b32" | "s_getreg_b32"
                        if mask != 0 => {
                        let id = ins.raw[0] & 63;
                        let shift = (ins.raw[0] >> 6) & 31;
                        let width = ((ins.raw[0] >> 11) & 31) + 1;
                        if shift + width > 32 { return Err("hardware register field exceeds word".into()); }
                        // Reads must be known in full. Writes need only update the
                        // intersection with the modeled register.
                        if m == "s_getreg_b32" && mask != u32::MAX >> (32 - width) {
                            return Err("unmodeled MODE read field".into());
                        }
                        let reg = if id == 1 { "tb_fp_mode".to_string() } else { format!("tb_flat_scratch{}", id & 1) };
                        self.regs.insert(reg.clone());
                        if m == "s_getreg_b32" {
                            return self.ws(&d, &format!("(({reg} >> {shift}) & {mask:#x}u)"));
                        }
                        let value = if m == "s_setreg_imm32_b32" {
                            format!("{:#x}u", ins.raw.get(1).ok_or("missing hardware-register literal")?)
                        } else {
                            self.touch(&d);
                            r(&d)?
                        };
                        self.line(&format!("{reg} = ({reg} & ~{:#x}u) | (({value} & {mask:#x}u) << {shift});", mask << shift));
                        Ok(())
                    }
                    // Other hardware-register writes and counter waits do not change the modeled state.
                    "s_setreg_imm32_b32" | "s_setreg_b32" | "s_waitcnt_vscnt" | "s_waitcnt_vmcnt"
                    | "s_waitcnt_expcnt" | "s_waitcnt_lgkmcnt" => Ok(()),
                    "s_addk_i32" => {
                        let a = name(&d)?;
                        let sum = format!("t{}", self.tmp);
                        self.tmp += 1;
                        self.line(&format!("uint {sum} = {a} + {imm};"));
                        self.line(&format!("scc = {};", signed_add_overflow(&a, &imm, &sum)));
                        self.ws(&d, &sum)
                    }
                    "s_mulk_i32" => self.ws(&d, &format!("({} * {imm})", name(&d)?)),
                    _ if m.starts_with("s_cmpk_") => {
                        let fake = m.replace("s_cmpk_", "s_cmp_");
                        let c = self.int_compare(&fake, &d, &Opnd::ConstInt(ins.aux.imm))?;
                        self.line(&format!("scc = {c} ? 1u : 0u;"));
                        Ok(())
                    }
                    _ => Err("unsupported SOPK".into()),
                }
            }
            Enc::Sopc => {
                let c = match m {
                    "s_bitcmp0_b32" => format!("((({} >> ({} & 31u)) & 1u) == 0u)", r(&s[0])?, r(&s[1])?),
                    "s_bitcmp1_b32" => format!("((({} >> ({} & 31u)) & 1u) != 0u)", r(&s[0])?, r(&s[1])?),
                    "s_cmp_eq_u64" => format!(
                        "({} == {} && {} == {})",
                        r(&s[0])?,
                        r(&s[1])?,
                        r(&hi(&s[0]))?,
                        r(&hi(&s[1]))?
                    ),
                    "s_cmp_lg_u64" => format!(
                        "({} != {} || {} != {})",
                        r(&s[0])?,
                        r(&s[1])?,
                        r(&hi(&s[0]))?,
                        r(&hi(&s[1]))?
                    ),
                    _ => self.int_compare(m, &s[0], &s[1])?,
                };
                self.line(&format!("scc = {c} ? 1u : 0u;"));
                Ok(())
            }
            Enc::Smem if self.packed.is_some() => self.emit_packed_load(idx, ins),
            Enc::Smem => {
                if ins.op <= 4 {
                    if let Some(Res::HeapLoad(offset)) = self.res.get(&idx).and_then(|v| v.first()).copied() {
                        let value = ssrc_name(u32::from(offset))?;
                        self.regs.insert(value.clone());
                        let dst = d.ok_or("missing direct descriptor destination")?;
                        self.touch(&dst);
                        let byte_offset = if ins.aux.imm == 0 { value }
                            else { format!("({value} + {:#x}u)", ins.aux.imm as u32) };
                        self.ws(&dst, &format!("tb_heap_index({byte_offset} >> 5)"))?;
                        return Ok(());
                    }
                    if let Some(Res::HeapSamplerLoad(offset)) = self.res.get(&idx).and_then(|v| v.first()).copied() {
                        let value = ssrc_name(u32::from(offset))?;
                        self.regs.insert(value.clone());
                        let dst = d.ok_or("missing sampler descriptor destination")?;
                        self.touch(&dst);
                        // The guest sampler-heap pointer is the heap start (as for the static
                        // path), so the byte offset over 16 is the descriptor index.
                        self.ws(&dst, &format!("({value} >> 4)"))?;
                        return Ok(());
                    }
                    let chosen = !self.chosen_samplers.is_empty() && ins.op == 2
                        && matches!(ins.src.first(), Some(Opnd::Sgpr(base)) if self.sampler_heap_base == Some(*base));
                    if let Some(Opnd::Sgpr(dst)) = d.as_ref().filter(|_| chosen) {
                        for (k, value) in [ins.aux.imm as u32 / 16, 0, 0, 0].into_iter().enumerate() {
                            let dst = sreg_opnd(dst + k as u8);
                            self.touch(&dst);
                            self.ws(&dst, &format!("{value}u"))?;
                        }
                        return Ok(());
                    }
                    if let Some(Res::CodeTbl) = self.res.get(&idx).and_then(|v| v.first()) {
                        // Constant table embedded in the code section: byte address = pointer
                        // (relative to the start of the program) + immediate.
                        let n = 1u32 << ins.op;
                        let base = sreg_num(&d.ok_or("no dst")?).ok_or("bad dst")?;
                        let ndw = ((self.ex.isa_data.len().saturating_sub(self.ex.isa.len())) / 4).max(1);
                        let t = self.tmp;
                        self.tmp += 1;
                        if let Some(offset) = self.static_smem_offset(ins) {
                            self.line(&format!("uint t{t} = ({} + {offset:#x}u) >> 2;", r(&s[0])?));
                            for k in 0..n {
                                self.line(&format!("uint t{t}_{k} = tb_code[(t{t} + {k}u - {}u) % {ndw}u];", self.ex.isa.len() / 4));
                            }
                        } else {
                            let reg = self.dynamic_code_offset(idx).ok_or("unverified dynamic code-table offset")?;
                            let offset = Opnd::Sgpr(reg);
                            self.touch(&offset);
                            self.line(&format!("uint t{t} = ({} + {} + {:#x}u - {}u) >> 2;", r(&s[0])?, r(&offset)?, ins.aux.imm, self.ex.isa.len()));
                            for k in 0..n {
                                self.line(&format!("uint t{t}_{k} = tb_code[t{t} + {k}u];"));
                            }
                        }
                        for k in 0..n {
                            let dst = sreg_opnd(base + k as u8);
                            self.touch(&dst);
                            self.ws(&dst, &format!("t{t}_{k}"))?;
                        }
                        return Ok(());
                    }
                    self.line("// (descriptor fetch from root table — resolved statically)");
                    return Ok(());
                }
                let n = 1u32 << (ins.op - 8);
                let base = sreg_num(&d.ok_or("no dst")?).ok_or("bad dst")?;
                let soff = ins.raw.get(1).copied().unwrap_or(0) >> 25;
                // SOFFSET is not an operand of the decoded instruction. VCC and the other
                // special registers are always declared.
                if soff < 106 {
                    self.regs.insert(ssrc_name(soff)?);
                }
                let slot = match self.res.get(&idx).and_then(|v| v.first()).copied() {
                    Some(Res::HeapBuffer(src)) if self.compute_heap => {
                        // A compute heap descriptor names a raw SRV (geometry and instance
                        // data read by both scalar and vector loads), not a CBV.
                        let t = self.tmp;
                        self.tmp += 1;
                        let offset = if soff != 0x7d { format!(" + {}", ssrc_name(soff)?) } else { String::new() };
                        let buf = self.bind_heap(HeapView::Raw, src);
                        self.line(&format!("uint tb_o{t} = ({}u{offset}) & ~3u;", ins.aux.imm as u32));
                        for k in 0..n {
                            self.line(&format!("uint t{t}_{k} = {buf}.Load(tb_o{t} + {}u);", 4 * k));
                        }
                        for k in 0..n {
                            let dst = sreg_opnd(base + k as u8);
                            self.touch(&dst);
                            self.ws(&dst, &format!("t{t}_{k}"))?;
                        }
                        return Ok(());
                    }
                    Some(Res::HeapBuffer(src)) => {
                        let t = self.tmp;
                        self.tmp += 1;
                        let offset = if soff != 0x7d { format!(" + {}", ssrc_name(soff)?) } else { String::new() };
                        let buf = self.bind_heap(HeapView::Constants, src);
                        self.line(&format!("uint tb_o{t} = ({}u{offset}) >> 2;", ins.aux.imm as u32));
                        for k in 0..n {
                            self.line(&format!("uint t{t}_{k} = {buf}.data[(tb_o{t}+{k}u) >> 2][(tb_o{t}+{k}u)&3u];"));
                        }
                        for k in 0..n {
                            let dst = sreg_opnd(base + k as u8);
                            self.touch(&dst);
                            self.ws(&dst, &format!("t{t}_{k}"))?;
                        }
                        return Ok(());
                    }
                    Some(Res::Cb(s)) => s,
                    Some(res @ (Res::Buf(slot, is_uav) | Res::Raw(slot, is_uav))) => {
                        // Snapshot all words before replacing any overlapping destination SGPR.
                        let raw = matches!(res, Res::Raw(..));
                        let nm = if is_uav { "ubuf" } else { "sbuf" };
                        let dynamic = if soff != 0x7d { format!(" + {}", ssrc_name(soff)?) } else { String::new() };
                        let t = self.tmp;
                        self.tmp += 1;
                        self.line(&format!("uint t{t} = {:#x}u{dynamic};", ins.aux.imm as u32));
                        for k in 0..n {
                            let load = if raw {
                                format!("raw_{nm}{slot}.Load(t{t} + {}u)", 4 * k)
                            } else {
                                format!("tb_sbw_{nm}{slot}(t{t} + {}u)", 4 * k)
                            };
                            self.line(&format!("uint t{t}_{k} = {load};"));
                        }
                        for k in 0..n {
                            let dst = sreg_opnd(base + k as u8);
                            self.touch(&dst);
                            self.ws(&dst, &format!("t{t}_{k}"))?;
                        }
                        return Ok(());
                    }
                    _ => return Err("unresolved constant buffer".into()),
                };
                if soff != 0x7d {
                    // Dynamic byte offset = imm + SGPR[soffset].
                    let so = ssrc_name(soff)?;
                    let t = self.tmp;
                    self.tmp += 1;
                    self.line(&format!("uint t{t} = ({:#x}u + {so}) >> 2;", ins.aux.imm as u32));
                    // Read all words before writing (the offset register may be a destination).
                    for k in 0..n {
                        self.line(&format!("uint t{t}_{k} = cb{slot}[((t{t} + {k}u) >> 2) & 4095u][(t{t} + {k}u) & 3u];"));
                    }
                    for k in 0..n {
                        let dst = sreg_opnd(base + k as u8);
                        self.touch(&dst);
                        self.ws(&dst, &format!("t{t}_{k}"))?;
                    }
                    return Ok(());
                }
                let w0 = ins.aux.imm as u32 / 4;
                for k in 0..n {
                    let w = w0 + k;
                    let dst = sreg_opnd(base + k as u8);
                    self.touch(&dst);
                    self.ws(&dst, &format!("cb{slot}[{}].{}", w / 4, XYZW[(w % 4) as usize]))?;
                }
                Ok(())
            }
            Enc::Mimg => self.emit_mimg(idx, ins),
            Enc::Mtbuf => self.emit_mtbuf(idx, ins),
            Enc::Mubuf if ins.op == 0x71 || ins.op == 0x72 => {
                // D3D12 exposes UAV memory visibility, not individual cache levels.
                // A device fence has no thread rendezvous; guest S_BARRIER is separate.
                if self.is_cs {
                    self.line("DeviceMemoryBarrier();");
                }
                Ok(())
            }
            Enc::Mubuf if self.packed.is_some() => self.emit_packed_buffer(idx, ins),
            Enc::Mubuf => self.emit_mubuf(idx, ins),
            Enc::Flat if m == "flat_load_ushort" => {
                code_table::halfword(&self.insns, idx, self.ex.isa.len(), self.ex.isa_data.len())
                    .ok_or("halfword load does not address a proven PC-relative table")?;
                self.uses_code = true;
                let t = self.tmp;
                self.tmp += 1;
                let addr = r(&s[0])?;
                self.line(&format!("uint t{t} = {addr} - {}u;", self.ex.isa.len()));
                self.wv(&d.ok_or("bad halfword destination")?, &format!("(tb_code[t{t} >> 2] >> ((t{t} & 2u) * 8u)) & 0xffffu"))
            }
            Enc::Flat if m.starts_with("flat_load_dword") => {
                // Per-lane load through a 64-bit address. Shaders use this for lookups in
                // constant tables embedded in their own code section (address derived from
                // `s_getpc_b64`), so it is only accepted when the program takes its PC.
                if ins.aux.mask == 1 {
                    return Err("scratch-memory load unsupported".into());
                }
                if !self.insns.iter().any(|i| i.mnemonic == "s_getpc_b64") {
                    return Err("global/flat load in a shader that never takes its PC".into());
                }
                self.uses_code = true;
                let n: u8 = match m {
                    "flat_load_dword" => 1,
                    "flat_load_dwordx2" => 2,
                    "flat_load_dwordx3" => 3,
                    _ => 4,
                };
                let extra = match &s[2] {
                    Opnd::Null => String::new(),
                    o => format!(" + {}", r(o)?),
                };
                let ndw = ((self.ex.isa_data.len().saturating_sub(self.ex.isa.len())) / 4).max(1);
                let vd = match d {
                    Some(Opnd::Vgpr(v)) => v,
                    _ => return Err("bad vdst".into()),
                };
                let t = self.tmp;
                self.tmp += 1;
                self.line(&format!("uint t{t} = ({} + {:#x}u{extra}) >> 2;", r(&s[0])?, ins.aux.imm as i32 as u32));
                for k in 0..n {
                    self.line(&format!("uint t{t}_{k} = tb_code[(t{t} + {k}u - {}u) % {ndw}u];", self.ex.isa.len() / 4));
                }
                for k in 0..n {
                    self.wv(&Opnd::Vgpr(vd + k), &format!("t{t}_{k}"))?;
                }
                Ok(())
            }
            Enc::Ds
                if !self.is_gs
                    && !self.is_cs
                    && self.ex.stage == Stage::Vertex
                    && matches!(m, "ds_write_b32" | "ds_write2_b32" | "ds_write_b64" | "ds_write2_b64") =>
            {
                // A vertex shader chained in front of a geometry shader hands its outputs over
                // through LDS instead of parameter exports: the vertex record is the output
                // registers back to back, 4 dwords each (observed in a55400ccab602283: dwords
                // 0..3 = TEXCOORD6.xyzw, 4..6 = TEXCOORD8.xyz, matching the output signature).
                // The address register is this vertex's record base and is not needed.
                let off = ins.aux.imm as u32;
                let vg = |o: &Opnd, k: u8| match o {
                    Opnd::Vgpr(n) => Ok(Opnd::Vgpr(n + k)),
                    _ => Err("LDS output write with a non-VGPR source".to_string()),
                };
                // (byte offset, source register) per written dword.
                let writes: Vec<(u32, Opnd)> = match m {
                    "ds_write_b32" => vec![(off & 0xffff, s[1])],
                    "ds_write2_b32" => vec![((off & 0xff) * 4, s[1]), (((off >> 8) & 0xff) * 4, s[2])],
                    "ds_write_b64" => vec![(off & 0xffff, s[1]), ((off & 0xffff) + 4, vg(&s[1], 1)?)],
                    _ => vec![
                        ((off & 0xff) * 8, s[1]),
                        ((off & 0xff) * 8 + 4, vg(&s[1], 1)?),
                        (((off >> 8) & 0xff) * 8, s[2]),
                        (((off >> 8) & 0xff) * 8 + 4, vg(&s[2], 1)?),
                    ],
                };
                let osig = self.ex.output_sig.clone();
                for (byte, src) in writes {
                    let dw = (byte / 4) as usize;
                    // The record is indexed by output *register* (4 dwords each), system
                    // values included: in 008bccdcf9dcecf9 register 1 is SV_Position and is
                    // written at dwords 4..7.
                    let reg = (dw / 4) as u32;
                    let comp = dw % 4;
                    self.touch(&src);
                    let mut hit = false;
                    for (ei, e) in osig.iter().enumerate() {
                        if e.register == reg && (e.mask >> comp) & 1 != 0 {
                            let idx = (0..comp).filter(|c| (e.mask >> c) & 1 != 0).count();
                            let (ty, n) = sig_type(e);
                            let field = if n == 1 { format!("o.o{ei}") } else { format!("o.o{ei}.{}", XYZW[idx]) };
                            let val = if ty.starts_with("float") { f(&src)? } else if ty.starts_with("int") { format!("asint({})", r(&src)?) } else { r(&src)? };
                            self.line(&format!("if (exec_lo & 1u) {field} = {val};"));
                            hit = true;
                        }
                    }
                    if !hit {
                        self.line(&format!("// (LDS output dword {dw} has no output-signature component: ignored)"));
                    }
                }
                Ok(())
            }
            Enc::Ds if self.is_gs && matches!(m, "ds_read_b32" | "ds_read2_b32" | "ds_write_b32" | "ds_write2_b32") => {
                self.emit_gs_lds(ins)
            }
            Enc::Ds if self.is_cs && matches!(m, "ds_read_b64" | "ds_read2_b64" | "ds_read2st64_b64" | "ds_write_b128" | "ds_write_b64" | "ds_write2_b64") => {
                if ins.raw[0] & (1 << 17) != 0 { return Err("GDS is unsupported".into()); }
                self.uses_lds = true;
                let t = self.tmp;
                self.tmp += 1;
                // Snapshot the address before any destination word can overwrite its VGPR.
                self.line(&format!("uint t{t}addr = {};", r(&s[0])?));
                let offset = ins.aux.imm as u32;
                let read = m.starts_with("ds_read");
                let paired = matches!(m, "ds_read2_b64" | "ds_read2st64_b64" | "ds_write2_b64");
                let stride = if m == "ds_read2st64_b64" { 512 } else { 8 };
                let count = if matches!(m, "ds_read_b64" | "ds_write_b64") { 2 } else { 4 };
                for word in 0..count {
                    let (base, k) = if read {
                        (ins.dst[0], word)
                    } else if paired {
                        (s[1 + (word / 2) as usize], word % 2)
                    } else {
                        (s[1], word)
                    };
                    let Opnd::Vgpr(v) = base else { return Err("expected consecutive LDS VGPRs".into()) };
                    let reg = Opnd::Vgpr(v.checked_add(k).ok_or("LDS register span exceeds v255")?);
                    let byte = if paired {
                        ((offset >> (8 * (word / 2))) & 255) * stride + u32::from(word % 2) * 4
                    } else {
                        offset + u32::from(word) * 4
                    };
                    let location = format!("tb_lds[((t{t}addr + {byte}u) >> 2) & 8191u]");
                    if read {
                        self.wv(&reg, &location)?;
                    } else {
                        self.touch(&reg);
                        self.line(&format!("if (exec_lo & 1u) {location} = {};", r(&reg)?));
                    }
                }
                Ok(())
            }
            Enc::Ds
                if self.is_cs
                    && matches!(
                        m,
                        "ds_read_b32" | "ds_read2_b32" | "ds_read2st64_b32" | "ds_write_b32" | "ds_write2_b32" | "ds_write2st64_b32"
                    ) =>
            {
                // Compute: LDS is the thread group's shared memory → an HLSL `groupshared`
                // array addressed by byte address / 4 (32 KiB, the D3D groupshared limit).
                self.uses_lds = true;
                let off = ins.aux.imm as u32;
                // Two-address forms scale their 8-bit offsets by 4 bytes ("2") or by
                // 64 dwords = 256 bytes ("2st64"); express both in units of 4 bytes.
                let scale = if m.contains("st64") { 64 } else { 1 };
                let (off0, off1) = ((off & 0xff) * scale, ((off >> 8) & 0xff) * scale);
                let addr = r(&s[0])?;
                let at = |o: u32| format!("tb_lds[(({addr} + {o:#x}u) >> 2) & 8191u]");
                let vd = match d {
                    Some(Opnd::Vgpr(v)) => v,
                    _ => return Err("bad vdst".into()),
                };
                let t = self.tmp;
                self.tmp += 1;
                match m {
                    "ds_read_b32" => {
                        if self.wave_cull && matches!(self.ex.entry.as_deref(), Some("CullPerPageDrawCommandsCs" | "VirtualSmInvalidateInstancePagesCS" | "GenerateReflectionRaysCS")) {
                            self.line(&format!("uint t{t} = 0u; if (exec_lo & 1u) t{t} = {};", at(off & 0xffff)));
                        } else {
                            self.line(&format!("uint t{t} = {};", at(off & 0xffff)));
                        }
                        self.wv(&Opnd::Vgpr(vd), &format!("t{t}"))
                    }
                    "ds_read2_b32" | "ds_read2st64_b32" => {
                        if self.wave_cull && matches!(self.ex.entry.as_deref(), Some("CullPerPageDrawCommandsCs" | "VirtualSmInvalidateInstancePagesCS" | "GenerateReflectionRaysCS")) {
                            self.line(&format!("uint t{t}a = 0u, t{t}b = 0u; if (exec_lo & 1u) {{ t{t}a = {}; t{t}b = {}; }}", at(off0 * 4), at(off1 * 4)));
                        } else {
                            self.line(&format!("uint t{t}a = {}; uint t{t}b = {};", at(off0 * 4), at(off1 * 4)));
                        }
                        self.wv(&Opnd::Vgpr(vd), &format!("t{t}a"))?;
                        self.wv(&Opnd::Vgpr(vd + 1), &format!("t{t}b"))
                    }
                    "ds_write_b32" => {
                        self.line(&format!("if (exec_lo & 1u) {} = {};", at(off & 0xffff), r(&s[1])?));
                        Ok(())
                    }
                    _ => {
                        self.line(&format!(
                            "if (exec_lo & 1u) {{ {} = {}; {} = {}; }}",
                            at(off0 * 4),
                            r(&s[1])?,
                            at(off1 * 4),
                            r(&s[2])?
                        ));
                        Ok(())
                    }
                }
            }
            Enc::Ds if self.is_cs && matches!(m, "ds_write_b16" | "ds_write_b16_d16_hi" | "ds_write_b8" | "ds_read_u16" | "ds_read_i16" | "ds_read_u8" | "ds_read_i8" | "ds_read_u16_d16" | "ds_read_u16_d16_hi") => {
                // Sub-word accesses to group-shared memory: tb_lds is addressed in dwords, so
                // read-modify-write the containing dword.
                self.uses_lds = true;
                let off = ins.aux.imm as u32 & 0xffff;
                let bits = if m.contains("16") { 16u32 } else { 8 };
                let mask = (1u32 << bits) - 1;
                let t = self.tmp;
                self.tmp += 1;
                self.line(&format!("uint t{t}a = ({}) + {off:#x}u; uint t{t}s = (t{t}a & 3u) * 8u;", r(&s[0])?));
                let word = format!("tb_lds[(t{t}a >> 2) & 8191u]");
                if m.starts_with("ds_write") {
                    let mut data = r(&s[1])?;
                    if m == "ds_write_b16_d16_hi" {
                        if ins.raw[0] & (1 << 17) != 0 {
                            return Err("unsupported global data store halfword write".into());
                        }
                        data = format!("({data} >> 16)");
                    }
                    // Adjacent lanes can target different halves of the same word.
                    // A plain read-modify-write would lose one lane's update.
                    self.line(&format!("if (exec_lo & 1u) {{ uint t{t}old; InterlockedOr({word}, 0u, t{t}old);"));
                    self.line(&format!("uint t{t}seen; [allow_uav_condition] do {{ t{t}seen = t{t}old;"));
                    self.line(&format!("uint t{t}new = (t{t}seen & ~({mask:#x}u << t{t}s)) | ((({data}) & {mask:#x}u) << t{t}s);"));
                    self.line(&format!("InterlockedCompareExchange({word}, t{t}seen, t{t}new, t{t}old); }} while (t{t}old != t{t}seen); }}"));
                    return Ok(());
                }
                let v = format!("(({word} >> t{t}s) & {mask:#x}u)");
                let dst = d.ok_or("no dst")?;
                let e = match m {
                    "ds_read_u16_d16" => format!("(({} & 0xffff0000u) | {v})", r(&dst)?),
                    "ds_read_u16_d16_hi" => format!("(({} & 0xffffu) | ({v} << 16))", r(&dst)?),
                    _ if m.contains("_i") => format!("asuint(asint({v} << {}) >> {})", 32 - bits, 32 - bits),
                    _ => v,
                };
                return self.wv(&dst, &e);
            }
            Enc::Ds
                if self.is_cs
                    && (m == "ds_wrxchg_rtn_b32" || matches!(
                        m.replace("_rtn", "").as_str(),
                        "ds_add_u32" | "ds_min_u32" | "ds_max_u32" | "ds_and_b32" | "ds_or_b32" | "ds_xor_b32"
                    )) =>
            {
                // Atomic read-modify-write on group-shared memory (the `_rtn` forms also
                // return the old value) → the matching HLSL Interlocked* intrinsic.
                self.uses_lds = true;
                if m == "ds_wrxchg_rtn_b32" && ins.raw[0] & (1 << 17) != 0 {
                    return Err("global data store exchange unsupported".into());
                }
                if m == "ds_wrxchg_rtn_b32" && self.insns.iter().any(|op|
                    op.enc == Enc::Sopp && Self::branch_cond(op.op).is_some()) {
                    return Err("LDS exchange with unverified branch convergence".into());
                }
                let func = match m.replace("_rtn", "").as_str() {
                    "ds_add_u32" => "InterlockedAdd",
                    "ds_min_u32" => "InterlockedMin",
                    "ds_max_u32" => "InterlockedMax",
                    "ds_and_b32" => "InterlockedAnd",
                    "ds_or_b32" => "InterlockedOr",
                    "ds_wrxchg_b32" => "InterlockedExchange",
                    _ => "InterlockedXor",
                };
                let off = ins.aux.imm as u32 & 0xffff;
                let addr = r(&s[0])?;
                let t = self.tmp;
                self.tmp += 1;
                self.line(&format!(
                    "uint t{t} = 0u; if (exec_lo & 1u) {{ {func}(tb_lds[(({addr} + {off:#x}u) >> 2) & 8191u], {}, t{t}); }}",
                    r(&s[1])?
                ));
                if m.contains("_rtn") {
                    self.wv(&d.ok_or("no dst")?, &format!("t{t}"))?;
                }
                Ok(())
            }
            Enc::Ds if m == "ds_bpermute_b32" => {
                if self.required_wave_size != Some(32) || ins.raw[0] & 0x3ff00 != 0 {
                    return Err("backward permute requires a verified wave32 compute layout".into());
                }
                let t = self.tmp;
                self.tmp += 1;
                let (addr, data) = (r(&s[0])?, r(&s[1])?);
                self.line(&format!("uint t{t} = WaveReadLaneAt(((exec_lo & 1u) != 0u) ? {data} : 0u, (({addr} + {}u) >> 2) & 31u);", ins.aux.imm));
                self.wv(&d.ok_or("no permute destination")?, &format!("t{t}"))
            }
            Enc::Ds if m == "ds_swizzle_b32" => {
                if self.resolve_wave32 {
                    let off = ins.aux.imm as u32;
                    if ins.raw[0] & (1 << 17) != 0 || (off & 0x8000 != 0 && off & 0xff00 != 0x8000) {
                        return Err("unsupported resolve wave swizzle mode".into());
                    }
                    let lane = "WaveGetLaneIndex()";
                    let partner = if off & 0x8000 != 0 {
                        format!("({lane} & ~3u) | (({:#x}u >> (2u * ({lane} & 3u))) & 3u)", off & 255)
                    } else {
                        format!("(({lane} & {}u) | {}u) ^ {}u", off & 31, (off >> 5) & 31, (off >> 10) & 31)
                    };
                    let source = r(&s[0])?;
                    let t = self.tmp;
                    self.tmp += 1;
                    self.line(&format!("uint t{t} = WaveReadLaneAt((exec_lo & tb_lane_bit) != 0u ? {source} : 0u, {partner});"));
                    return self.wv(&d.ok_or("no swizzle destination")?, &format!("t{t}"));
                }
                if self.is_cs && self.packed.as_ref().is_some_and(|p| p.scan_width.is_some()) {
                    let off = ins.aux.imm as u32;
                    if ins.raw[0] & (1 << 17) != 0 || off & 0x8000 != 0 { return Err("unsupported compute swizzle mode".into()); }
                    let t = self.tmp;
                    self.tmp += 1;
                    // Bitmask mode rearranges lanes within each independent group of 32.
                    self.line(&format!("uint t{t}lane = WaveGetLaneIndex();"));
                    self.line(&format!("uint t{t}src = (t{t}lane & ~31u) | (((t{t}lane & {}u) | {}u) ^ {}u);",
                        off & 31, (off >> 5) & 31, (off >> 10) & 31));
                    self.line(&format!("uint t{t}value = WaveReadLaneAt({}, t{t}src);", r(&s[0])?));
                    return self.wv(&d.ok_or("no swizzle destination")?, &format!("t{t}value"));
                }
                if self.compute_swizzle {
                    if self.hoisted_swizzle != Some(idx) { self.emit_compute_swizzle(idx, ins, &[])?; }
                    return self.wv(&d.ok_or("no swizzle destination")?, &format!("swizzle{idx}"));
                }
                if !is_ps { return Err("ds_swizzle outside a pixel shader".into()); }
                let off = ins.aux.imm as u32;
                if off & 0x8000 == 0 { return Err("ds_swizzle without QDMode unsupported".into()); }
                self.uses_quad = true;
                let src = r(&s[0])?;
                let t = self.tmp;
                self.tmp += 1;
                let dst = d.ok_or("no dst")?;
                let pattern = off & 255;
                let lane = pattern & 3;
                self.line("#if __SHADER_TARGET_MAJOR >= 6");
                // Keep reads outside the emulated EXEC predicate so helper lanes participate.
                if pattern == lane * 0x55 {
                    self.line(&format!("uint t{t}quad = QuadReadLaneAt({src}, {lane});"));
                    self.wv(&dst, &format!("t{t}quad"))?;
                } else {
                    self.line(&format!("uint4 t{t}quad = uint4(QuadReadLaneAt({src}, 0), QuadReadLaneAt({src}, 1), QuadReadLaneAt({src}, 2), QuadReadLaneAt({src}, 3));"));
                    self.wv(&dst, &format!("t{t}quad[({pattern:#x}u >> (tb_quad * 2u)) & 3u]"))?;
                }
                self.line("#else");
                self.line(&format!("float t{t} = asfloat({src}); uint t{t}s = ({:#x}u >> (tb_quad * 2u)) & 3u;", off & 255));
                self.wv(&dst, &format!("asuint(t{t} + ((float)(t{t}s & 1u) - (float)(tb_quad & 1u)) * ddx_fine(t{t}) + ((float)(t{t}s >> 1) - (float)(tb_quad >> 1)) * ddy_fine(t{t}))"))?;
                self.line("#endif");
                Ok(())
            }
            Enc::Vinterp => {
                if !is_ps {
                    return Err("v_interp outside a pixel shader".into());
                }
                let attr = ins.aux.imm as usize;
                let chan = ins.aux.imm2 as usize;
                let Param::Full(reg) = *self.ps_attrs.get(attr).ok_or_else(|| format!("PS attr{attr} has no signature register"))? else {
                    return Err(format!("32-bit read of half-float PS attr{attr}"));
                };
                let val = if let Some((ei, k)) = sig_lookup(&self.ex.input_sig, reg, chan) {
                    let acc = format!("i.i{ei}.{}", XYZW[k]);
                    if matches!(self.ex.input_sig[ei].comp_type, 3 | 6) { format!("asuint({acc})") } else { format!("(uint)({acc})") }
                } else if chan == 3 {
                    "0x3f800000u".to_string()
                } else {
                    "0u".to_string()
                };
                if self.pulled_attrs.contains(&attr) {
                    if let Some((ei, k)) = sig_lookup(&self.ex.input_sig, reg, chan) {
                        let (_, n) = sig_type(&self.ex.input_sig[ei]);
                        let field = if n == 1 { format!("i.i{ei}") } else { format!("i.i{ei}.{}", XYZW[k]) };
                        let vertex = |index| {
                            let bits = format!("GetAttributeAtVertex({field}, {index})");
                            if matches!(self.ex.input_sig[ei].comp_type, 3 | 6) { bits } else { format!("asfloat({bits})") }
                        };
                        let p0 = vertex(0);
                        let delta = |v| format!("({} - {p0})", vertex(v));
                        let dst = d.ok_or("no interpolation destination")?;
                        let expr = match ins.op {
                            0 => format!("{p0} + {} * {}", f(&s[0])?, delta(1)),
                            1 => format!("{} + {} * {}", f(&dst)?, f(&s[0])?, delta(2)),
                            2 => match ins.raw[0] & 255 {
                                0 => delta(1), 1 => delta(2), 2 => p0, _ => return Err("invalid interpolation parameter".into()),
                            },
                            _ => return Err("unsupported interpolation opcode".into()),
                        };
                        self.line("#if __SHADER_TARGET_MAJOR >= 6");
                        self.wv(&dst, &format!("asuint({expr})"))?;
                        self.line("#else");
                        self.wv(&dst, &val)?;
                        self.line("#endif");
                        return Ok(());
                    }
                    // A missing channel is constant at all three vertices.
                    let dst = d.ok_or("no interpolation destination")?;
                    let constant = match ins.op {
                        1 => return Ok(()), // the second delta is zero
                        2 if matches!(ins.raw[0] & 255, 0 | 1) => "0u",
                        _ => &val,
                    };
                    return self.wv(&dst, constant);
                }
                // p1 computes a partial term in hardware; the full interpolated value is
                // already available per-pixel here, so both steps assign it.
                self.wv(&d.ok_or("no dst")?, &val)
            }
            Enc::Exp => self.emit_exp(ins, is_ps),
            _ => Err("unsupported encoding".into()),
        }
    }

    // READLANE ignores EXEC. Publish each lane's current VGPR even when
    // that lane skipped the guest branch, then guard only the scalar write.
    fn emit_indexed_read(&mut self, idx: usize, ins: &Insn) -> Result<(), String> {
        self.line(&format!("tb_swizzle[tb_lane] = {};", r(&ins.src[0])?));
        self.line("GroupMemoryBarrierWithGroupSync();");
        self.line(&format!("uint readlane{idx} = tb_swizzle[(tb_lane & ~63u) | ({} & 63u)];", r(&ins.src[1])?));
        self.line("GroupMemoryBarrierWithGroupSync();");
        Ok(())
    }

    fn emit_compute_swizzle(&mut self, idx: usize, ins: &Insn, active: &[usize]) -> Result<(), String> {
        if ins.mnemonic == GROUP_READ {
            // A lane read ignores EXEC.
            let guard: Vec<_> = active.iter().map(|label| format!("!j{label}")).collect();
            let value = r(&ins.src[0])?;
            let value = if guard.is_empty() { value } else { format!("({}) ? {value} : 0u", guard.join(" && ")) };
            self.line(&format!("tb_swizzle[tb_lane] = {value};"));
            self.line("GroupMemoryBarrierWithGroupSync();");
            self.line(&format!("uint swizzle{idx} = tb_swizzle[(tb_lane & ~63u) | {}];", r(&ins.src[1])?));
            self.line("GroupMemoryBarrierWithGroupSync();");
            return Ok(());
        }
        let mode = ins.aux.imm as u32;
        // RDNA2 ISA DS_SWIZZLE_B32: offset[15] selects QDMode, which permutes
        // within each quad by offset[7:0]. Other high offset bits and GDS stay refused.
        let quad = mode & 0x8000 != 0;
        if ins.raw[0] & 0x3_0000 != 0 || (quad && mode & 0xff00 != 0x8000) {
            return Err("unsupported compute DS swizzle encoding".into());
        }
        // Skipped and EXEC-masked lanes contribute zero. Rendezvous before
        // reapplying the guest branch guard to the destination write.
        let mut predicate = String::from("(exec_lo & 1u) != 0u");
        for label in active { write!(predicate, " && !j{label}").unwrap(); }
        let value = r(&ins.src[0])?;
        self.line(&format!("tb_swizzle[tb_lane] = ({predicate}) ? {value} : 0u;"));
        self.line("GroupMemoryBarrierWithGroupSync();");
        if quad {
            self.line(&format!("uint swizzle{idx} = tb_swizzle[(tb_lane & ~3u) | (({:#x}u >> (2u * (tb_lane & 3u))) & 3u)];",
                mode & 255));
        } else {
            self.line(&format!("uint swizzle{idx} = tb_swizzle[(tb_lane & ~31u) | (((tb_lane & {}u) | {}u) ^ {}u)];",
                mode & 31, (mode >> 5) & 31, (mode >> 10) & 31));
        }
        self.line("GroupMemoryBarrierWithGroupSync();");
        Ok(())
    }



    /// Decode the packed SRC0 attribute descriptor of a Vop3 f16-interpolation
    /// instruction: (attr, channel, high).
    fn f16_interp_descriptor(second: u32) -> (usize, usize, bool) {
        ((second & 0x3f) as usize, ((second >> 6) & 3) as usize, (second >> 8) & 1 != 0)
    }

    /// Emit a Vop3-encoded f16-interpolation instruction.
    ///
    /// SRC0 in the second word packs the attribute descriptor: attr (6 bits),
    /// channel (2 bits), and the `high` bit (1 bit). SRC1 is the barycentric
    /// weight; for p2_f16, SRC2 is the p1ll's f32 partial register. An attribute
    /// packed in a 32-bit word holds two 16-bit halves, so each vertex's raw
    /// word is decoded per vertex and the halves are interpolated (mirroring
    /// the pulled-attribute path), which is also what Mesa ACO does.
    fn emit_f16_interp(&mut self, ins: &Insn) -> Result<(), String> {
        if self.ex.stage != Stage::Pixel {
            return Err("v_interp outside a pixel shader".into());
        }
        if ins.enc != Enc::Vop3 || ins.raw.len() < 2 {
            return Err("invalid f16 interpolation encoding".into());
        }
        let m = ins.mnemonic.as_str();
        let is_p2 = m == "v_interp_p2_f16";
        let (attr, chan, high) = Self::f16_interp_descriptor(ins.raw[1]);
        let clamp = (ins.raw[0] >> 15) & 1 != 0;
        let param = self.ps_attrs.get(attr).ok_or_else(|| format!("PS attr{attr} has no signature register"))?;
        // A packed half-float component arrives as the float it stands for.
        let packed = param.component(chan, high);
        let (reg, chan) = match (param, packed) {
            (Param::Full(reg), _) => (*reg, chan),
            (_, Some(found)) => found,
            _ => return Err(format!("PS attr{attr} has no signature register")),
        };
        let (ei, k) = sig_lookup(&self.ex.input_sig, reg, chan)
            .ok_or_else(|| format!("PS attr{attr} has no signature register"))?;
        if matches!(self.ex.input_sig[ei].comp_type, 1 | 2) {
            return Err("v_interp f16 with a typed signature element unsupported".into());
        }
        let (_, n) = sig_type(&self.ex.input_sig[ei]);
        let field = if n == 1 { format!("i.i{ei}") } else { format!("i.i{ei}.{}", XYZW[k]) };
        let raw = |index: usize| -> String {
            format!("asuint(GetAttributeAtVertex({field}, {index}))")
        };
        let half = |index: usize| -> String {
            let h = raw(index);
            if high { format!("(({h} >> 16u) & 0xffffu)") } else { format!("({h} & 0xffffu)") }
        };
        let vertex = |index: usize| {
            if packed.is_some() { format!("GetAttributeAtVertex({field}, {index})") } else { format!("f16tof32({})", half(index)) }
        };
        let (p0, p1, p2v) = (vertex(0), vertex(1), vertex(2));
        if packed.is_some() && !self.pulled_attrs.contains(&attr) {
            let dst = ins.dst.first().cloned().ok_or("no dst")?;
            if !is_p2 {
                return self.wv(&dst, &format!("asuint({field})"));
            }
            let value = if clamp { format!("saturate({field})") } else { field.clone() };
            let keep = r(&dst)?;
            self.touch(&dst);
            return self.wv(&dst, &format!("(({keep} & 0xffff0000u) | tb_f16_rne({value}))"));
        }
        let dst = ins.dst.first().cloned().ok_or("no dst")?;
        let name_d = name(&dst)?;
        // SM5 fallback: the raw smooth read (best-effort; the per-vertex path is SM6+).
        let fallback = format!("asuint({field})");
        if !is_p2 {
            // p1ll/p1lv: the f32 partial P0 + I*(P1-P0), consumed by p2_f16.
            let i = f(&ins.src[1])?;
            let expr = format!("{p0} + {i} * ({p1} - {p0})");
            self.line("#if __SHADER_TARGET_MAJOR >= 6");
            self.wv(&dst, &format!("asuint({expr})"))?;
            self.line("#else");
            self.wv(&dst, &fallback)?;
            self.line("#endif");
            Ok(())
        } else {
            if ins.src.len() < 3 {
                return Err("missing p2_f16 partial register".into());
            }
            let j = f(&ins.src[1])?;
            let partial = r(&ins.src[2])?;
            let full = format!("(asfloat({partial}) + {j} * ({p2v} - {p0}))");
            let full = if clamp { format!("saturate({full})") } else { full };
            let bits = format!("tb_f16_rne({full})");
            self.line("#if __SHADER_TARGET_MAJOR >= 6");
            self.wv(&dst, &format!("(({name_d} & 0xffff0000u) | {bits})"))?;
            self.line("#else");
            self.wv(&dst, &fallback)?;
            self.line("#endif");
            Ok(())
        }
    }

    fn emit_vec(&mut self, ins: &Insn) -> Result<(), String> {
        let mut m = ins.mnemonic.clone();
        let s = &ins.src;
        let d = ins.dst.first().cloned().ok_or("no dst")?;
        if self.required_wave_size == Some(64) && self.packed.as_ref().is_some_and(|p| p.lane_masks.contains(&self.cur_idx)) {
            self.touch(&hi(&s[2]));
        }
        if m == "tb_reflection_prefix32" {
            return self.wv(&d, &format!("WavePrefixSum({})", r(&s[0])?));
        }
        if m == "v_nop" {
            return Ok(());
        }
        if matches!(m.as_str(), "v_interp_p1ll_f16" | "v_interp_p1lv_f16" | "v_interp_p2_f16") {
            return self.emit_f16_interp(ins);
        }
        if ins.enc == Enc::Vop2 && ins.op == wave_sum::DPP8_EXCHANGE {
            if !self.portable_wave64 { return Err("DPP8 exchange without convergent group".into()); }
            let a = r(&s[0])?;
            let b = r(&s[1])?;
            let selector = ins.aux.imm as u32;
            let t = self.tmp;
            self.tmp += 1;
            self.line(&format!("uint t{t} = tb_reduction_read(gtid.x, {a}, (gtid.x & ~7u) | (({selector:#x}u >> (3u * (gtid.x & 7u))) & 7u));"));
            let value = match m.as_str() {
                "v_add_nc_u32" => format!("t{t} + {b}"),
                "v_and_b32" => format!("t{t} & {b}"),
                "v_or_b32" => format!("t{t} | {b}"),
                "v_xor_b32" => format!("t{t} ^ {b}"),
                "v_min_u32" => format!("min(t{t}, {b})"),
                "v_max_u32" => format!("max(t{t}, {b})"),
                "v_min_i32" => format!("asuint(min(asint(t{t}), asint({b})))"),
                "v_max_i32" => format!("asuint(max(asint(t{t}), asint({b})))"),
                _ => return Err("unsupported DPP8 exchange operation".into()),
            };
            return self.wv(&d, &value);
        }
        if ins.enc == Enc::Vop2 && ins.op == wave_sum::DPP_REDUCE {
            // Read the partner before the EXEC-guarded write, including when VDST aliases VSRC.
            let t = self.tmp;
            self.tmp += 1;
            let acc = r(&s[0])?;
            if self.pixel_wave {
                // The other lanes hold the identity, so each step keeps this one's value.
                self.pixel_sum = m.starts_with("v_add");
                return self.wv(&d, &acc);
            }
            let id = if self.portable_wave64 { "gtid.x" } else { "WaveGetLaneIndex()" };
            let control = ins.raw[1] >> 8; // The reduction matcher verified this word.
            let lane = match (ins.aux.imm, control) {
                (0, _) => format!("{id} ^ 1u"),
                (1, 0x936012) => format!("({id} & ~3u) | (({id} ^ 2u) & 2u)"),
                (2, 0x000924) => format!("({id} & ~7u) | (({id} ^ 4u) & 4u)"),
                (1, 0xb3e21a) => format!("{id} ^ 2u"),
                (2, 0x688fac) => format!("{id} ^ 4u"),
                (1 | 2, _) => format!("({id} & ~7u) | (({control:#x}u >> (3u * ({id} & 7u))) & 7u)"),
                (3, _) => format!("{id} ^ 15u"),
                _ => return Err("invalid integer sum step".into()),
            };
            let partner = if self.portable_wave64 {
                format!("tb_reduction_read(gtid.x, {acc}, {lane})")
            } else { format!("WaveReadLaneAt({acc}, {lane})") };
            let combined = match m.as_str() {
                "v_add_nc_u32" => format!("{partner} + {acc}"),
                "v_and_b32" => format!("{partner} & {acc}"),
                "v_or_b32" => format!("{partner} | {acc}"),
                "v_xor_b32" => format!("{partner} ^ {acc}"),
                "v_min_u32" => format!("min({partner}, {acc})"),
                "v_max_u32" => format!("max({partner}, {acc})"),
                "v_min_i32" => format!("asuint(min(asint({partner}), asint({acc})))"),
                "v_max_i32" => format!("asuint(max(asint({partner}), asint({acc})))"),
                "v_add_f32" => format!("asuint(asfloat({partner}) + asfloat({acc}))"),
                "v_min_f32" => format!("asuint(min(asfloat({partner}), asfloat({acc})))"),
                "v_max_f32" => format!("asuint(max(asfloat({partner}), asfloat({acc})))"),
                _ => return Err("invalid DPP reduction operation".into()),
            };
            self.line(&format!("uint t{t} = {combined};"));
            return self.wv(&d, &format!("t{t}"));
        }
        if ins.enc == Enc::Vop3 && ins.op == wave_sum::READ_LANE {
            if self.pixel_wave {
                if self.pixel_sum && s[1] == Opnd::ConstInt(32) { return self.ws(&d, "0u"); }
                return self.ws(&d, &r(&s[0])?);
            }
            let lane = r(&s[1])?;
            let lane = if matches!(s[1], Opnd::Sgpr(_)) {
                let width = self.required_wave_size.ok_or("missing width for scan lane read")?;
                format!("({lane} & {}u)", width - 1)
            } else { lane };
            let value = r(&s[0])?;
            let read = if self.portable_wave64 {
                format!("tb_reduction_read(gtid.x, {value}, {lane})")
            } else { format!("WaveReadLaneAt({value}, {lane})") };
            return self.ws(&d, &read);
        }
        if ins.enc == Enc::Vop2 && ins.op == OP_LOCAL_EXTREMA {
            let value = self.fp32_extrema(ins, r(&s[0])?)?;
            return self.wv(&d, &value);
        }
        if is_quad_exchange(ins) {
            if self.dispatched && (!self.pixel_wave || is_row_move(ins)) {
                return Err("quad exchange in a block-dispatched program".into());
            }
            if ins.op != OP_DPP_BINARY {
                let value = format!("q{}", self.cur_idx);
                let control = ins.raw[1];
                if control >> 24 != 0xff {
                    if self.ex.stage != Stage::Compute {
                        return Err("masked DPP16 move requires a known compute wave width".into());
                    }
                    // Destination masks do not affect which lanes can supply the source.
                    let rows = control >> 28;
                    let banks = (control >> 24) & 15;
                    let row = format!("({rows:#x}u >> (WaveGetLaneIndex() >> 4u))");
                    let bank = format!("({banks:#x}u >> ((WaveGetLaneIndex() >> 2u) & 3u))");
                    return self.wv(&d, &format!("(({row} & {bank} & 1u) != 0u) ? {value} : {}", r(&d)?));
                }
                return self.wv(&d, &value);
            }
            let peer = format!("q{}", self.cur_idx);
            let value = match ins.mnemonic.as_str() {
                op @ ("v_add_f32" | "v_sub_f32" | "v_subrev_f32" | "v_mul_f32" | "v_mul_legacy_f32") => {
                    let (a, b) = dpp_fp32_sources(ins, peer.clone())?;
                    match op {
                        "v_add_f32" => format!("asuint(asfloat({a}) + asfloat({b}))"),
                        "v_sub_f32" => format!("asuint(asfloat({a}) - asfloat({b}))"),
                        "v_subrev_f32" => format!("asuint(asfloat({b}) - asfloat({a}))"),
                        "v_mul_f32" => format!("asuint(asfloat({a}) * asfloat({b}))"),
                        _ => format!("asuint(tb_mull(asfloat({a}), asfloat({b})))"),
                    }
                }
                "v_min_f32" | "v_max_f32" => self.fp32_extrema(ins, peer.clone())?,
                "v_cvt_pkrtz_f16_f32" => {
                    let (lo, hi) = dpp_fp32_sources(ins, peer.clone())?;
                    format!("(tb_f16_rtz(asfloat({lo})) | (tb_f16_rtz(asfloat({hi})) << 16))")
                }
                "v_or_b32" => format!("{peer} | {}", r(&s[1])?),
                "v_and_b32" => format!("{peer} & {}", r(&s[1])?),
                "v_xor_b32" => format!("{peer} ^ {}", r(&s[1])?),
                "v_xnor_b32" => format!("~({peer} ^ {})", r(&s[1])?),
                "v_cndmask_b32" => format!("(vcc_lo & 1u) != 0u ? {} : {peer}", r(&s[1])?),
                "v_add_nc_u32" => format!("{peer} + {}", r(&s[1])?),
                "v_sub_nc_u32" => format!("{peer} - {}", r(&s[1])?),
                "v_subrev_nc_u32" => format!("{} - {peer}", r(&s[1])?),
                "v_lshlrev_b32" => format!("{} << ({peer} & 31u)", r(&s[1])?),
                "v_lshrrev_b32" => format!("{} >> ({peer} & 31u)", r(&s[1])?),
                "v_ashrrev_i32" => format!("asuint(asint({}) >> ({peer} & 31u))", r(&s[1])?),
                "v_min_i32" => format!("asuint(min(asint({peer}), asint({})))", r(&s[1])?),
                "v_max_i32" => format!("asuint(max(asint({peer}), asint({})))", r(&s[1])?),
                "v_min_u32" => format!("min({peer}, {})", r(&s[1])?),
                "v_max_u32" => format!("max({peer}, {})", r(&s[1])?),
                op => mul24_expr(op, &peer, &r(&s[1])?).ok_or("unknown quad binary operation")?,
            };
            // The source exchange precedes both the destination mask and EXEC.
            let mut guards = Vec::new();
            if let Some(mask) = group_extrema::destination_mask(ins) {
                if self.ex.stage != Stage::Compute {
                    return Err("DPP16 destination masks require a logical compute lane index".into());
                }
                let rows = mask >> 4;
                let banks = mask & 15;
                if rows != 15 {
                    let row_bits = group_extrema::row_index_mask(self, rows, banks)?;
                    guards.push(format!("(({rows:#x}u >> ((tb_lane >> 4u) & {row_bits}u)) & 1u) != 0u"));
                }
                if banks != 15 { guards.push(format!("(({banks:#x}u >> ((tb_lane >> 2u) & 3u)) & 1u) != 0u")); }
            }
            if !group_extrema::is_dpp8(ins) && ins.raw[1] & (3 << 18) == 0 {
                guards.push(format!("{peer}active"));
            }
            if guards.is_empty() { return self.wv(&d, &value); }
            return self.wv(&d, &format!("{} ? ({value}) : {}", guards.join(" && "), r(&d)?));
        }
        if ins.op == OP_WAVE_REDUCE && ins.enc == Enc::Vop3 {
            // See `lower_wave_reductions`. Every invocation computes the same value; lanes
            // that were inactive contribute the identity.
            let f = if m == "tb_wave_min_f32" { "WaveActiveMin" } else { "WaveActiveMax" };
            let e = format!(
                "asuint({f}((({} & 1u) != 0u) ? asfloat({}) : asfloat({:#x}u)))",
                r(&s[1])?,
                r(&s[0])?,
                ins.aux.imm as u32
            );
            return self.ws(&d, &e);
        }
        // ---- half-float arithmetic: operands are f16 in the low 16 bits (inline float
        // constants denote the half-float of that value); the result is written as f16.
        // VOP3 forms carry per-source abs/neg modifiers and a result clamp/output modifier,
        // exactly as for f32 (see `fm` / `fres`).
        let h16 = |o: &Opnd| -> Result<String, String> {
            let mut e = match o {
                // Inline selector 248 has distinct binary16 and binary32 values.
                Opnd::ConstF32(value) => f(&Opnd::ConstF32(inline_half_value(*value)))?,
                _ => format!("f16tof32(({}) & 0xffffu)", r(o)?),
            };
            if ins.enc == Enc::Vop3 {
                if let Some(i) = s.iter().position(|x| std::ptr::eq(x, o)) {
                    if (ins.aux.mask >> i) & 1 != 0 {
                        e = format!("abs({e})");
                    }
                    if (ins.aux.mask >> (3 + i)) & 1 != 0 {
                        e = format!("(-{e})");
                    }
                }
            }
            Ok(e)
        };
        let hres = |e| format!("tb_f16_rne({})", Self::float_result(ins, e));
        let half_binary = matches!(m.as_str(), "v_add_f16" | "v_sub_f16" | "v_subrev_f16" | "v_mul_f16" | "v_max_f16" | "v_min_f16");
        if ins.aux.sdwa.is_none() && half_binary {
            let (a, b) = (h16(&s[0])?, h16(&s[1])?);
            let e = match m.as_str() {
                "v_add_f16" => format!("{a} + {b}"),
                "v_sub_f16" => format!("{a} - {b}"),
                "v_subrev_f16" => format!("{b} - {a}"),
                "v_mul_f16" => format!("{a} * {b}"),
                "v_max_f16" => format!("max({a}, {b})"),
                _ => format!("min({a}, {b})"),
            };
            return self.wv(&d, &hres(e));
        }
        if ins.aux.sdwa.is_none() && matches!(m.as_str(), "v_sin_f16" | "v_cos_f16") {
            if ins.enc == Enc::Vop3 && (ins.aux.mask != 0 || ins.aux.imm != 0 || ins.aux.imm2 != 0) {
                return Err("half trigonometric instruction modifiers unsupported".into());
            }
            let bits = if matches!(s[0], Opnd::ConstF32(_)) { hres(f(&s[0])?) } else { r(&s[0])? };
            let lo = format!("tb_trig_f16({bits}, {})", m == "v_cos_f16");
            return self.wv(&d, &format!("(({} & 0xffff0000u) | {lo})", r(&d)?));
        }
        let unary_op = half_unary_op(&m);
        let modified_half_bits = || -> Result<String, String> {
            let mut bits = half_operand_bits(&s[0])?;
            if ins.enc == Enc::Vop3 {
                if ins.aux.mask & 1 != 0 { bits = format!("(({bits}) & 0x7fffu)"); }
                if ins.aux.mask & 8 != 0 { bits = format!("(({bits}) ^ 0x8000u)"); }
            }
            Ok(bits)
        };
        let half_int_src = integer_to_half(&m);
        if ins.aux.sdwa.is_none() && (unary_op.is_some() || half_int_src) {
            let has_mode = self.regs.contains("tb_fp_mode");
            let mode = if has_mode { "tb_fp_mode" } else { "0xc0u" };
            let narrow = |value| if has_mode {
                format!("tb_mode_half({value}, {mode})")
            } else {
                format!("tb_f16_rne({value})")
            };
            let mut bits = if let Some(op) = unary_op {
                if let Opnd::ConstF32(value) = s[0] {
                    // Inline floats are finite normal halves. Only 1/(2*pi)
                    // has a distinct rounded value in the half-precision ROM.
                    let mut value = inline_half_value(value);
                    if ins.enc == Enc::Vop3 {
                        if ins.aux.mask & 1 != 0 { value = value.abs(); }
                        if ins.aux.mask & 8 != 0 { value = -value; }
                    }
                    let x = f(&Opnd::ConstF32(value))?;
                    format!("tb_finite_unary_f16({x}, {mode}, {op}u)")
                } else {
                    format!("tb_unary_f16({}, {mode}, {op}u)", modified_half_bits()?)
                }
            } else {
                let raw = half_operand_bits(&s[0])?;
                let integer = if m == "v_cvt_f16_i16" {
                    format!("(asint(({raw}) << 16) >> 16)")
                } else {
                    format!("(({raw}) & 0xffffu)")
                };
                narrow(format!("(float){integer}"))
            };
            if ins.enc == Enc::Vop3 && (ins.aux.imm2 != 0 || ins.aux.imm != 0) {
                let scale = ["1.0", "2.0", "4.0", "0.5"][ins.aux.imm2 as usize];
                let clamp = ins.aux.imm != 0;
                bits = format!("tb_half_output({bits}, {mode}, {scale}, {clamp})");
            }
            return self.wv(&d, &format!("(({} & 0xffff0000u) | {bits})", r(&d)?));
        }
        if ins.aux.sdwa.is_none() && matches!(m.as_str(), "v_rcp_f16" | "v_sqrt_f16" | "v_rsq_f16" | "v_exp_f16") {
            let a = h16(&s[0])?;
            let e = match m.as_str() {
                "v_rcp_f16" => format!("1.0 / {a}"),
                "v_sqrt_f16" => format!("sqrt({a})"),
                "v_rsq_f16" => format!("rsqrt({a})"),
                _ => format!("exp2({a})"),
            };
            // The compact reciprocal updates the low half. The FSPX shader first
            // writes the high reciprocal via SDWA, then fills the low half here.
            if m == "v_rcp_f16" && ins.enc == Enc::Vop1 && matches!(d, Opnd::Vgpr(_)) {
                return self.wv(&d, &format!("(({} & 0xffff0000u) | {})", r(&d)?, hres(e)));
            }
            return self.wv(&d, &hres(e));
        }
        let half_int_result = half_to_integer(&m);
        if normalized_half_to_integer(&m) && ins.aux.sdwa.is_none() {
            let bits = modified_half_bits()?;
            let mode = if self.regs.contains("tb_fp_mode") { "tb_fp_mode" } else { "0xc0u" };
            let signed = m == "v_cvt_norm_i16_f16";
            return self.wv(&d, &format!("(({} & 0xffff0000u) | tb_norm_f16({bits}, {mode}, {signed}))", r(&d)?));
        }
        if half_int_result && ins.aux.sdwa.is_none() {
            let x = h16(&s[0])?;
            // Truncate after saturating. CLAMP controls the inexact exception,
            // independently of the integer value produced by the conversion.
            let mut value = if m == "v_cvt_u16_f16" {
                format!("(uint)clamp({x}, 0.0, 65535.0)")
            } else {
                format!("(asuint((int)clamp({x}, -32768.0, 32767.0)) & 0xffffu)")
            };
            if !matches!(s[0], Opnd::ConstF32(_)) {
                // Test NaN on its stored bits before DXC can assume a finite float.
                value = format!("((({}) & 0x7fffu) > 0x7c00u ? 0u : {value})", r(&s[0])?);
            }
            return self.wv(&d, &format!("(({} & 0xffff0000u) | {value})", r(&d)?));
        }
        if matches!(m.as_str(), "v_fmaak_f16" | "v_fmamk_f16") {
            // The trailing literal dword carries the half-float constant K in its low bits.
            let k = format!("f16tof32({:#x}u)", ins.raw.last().copied().unwrap_or(0) & 0xffff);
            let (a, b) = (h16(&s[0])?, h16(&s[1])?);
            let e = if m == "v_fmaak_f16" { format!("tb_fma_half({a}, {b}, {k})") } else { format!("tb_fma_half({a}, {k}, {b})") };
            return self.wv(&d, &hres(e));
        }
        if m == "v_fmac_f16" {
            self.touch(&d);
            let e = format!("{} * {} + {}", h16(&s[0])?, h16(&s[1])?, h16(&d)?);
            return self.wv(&d, &hres(e));
        }
        if m == "v_fma_f16" {
            let sel = (ins.raw[0] >> 11) & 15;
            let src = |i: usize| -> Result<String, String> {
                if sel & (1 << i) == 0 { return h16(&s[i]); }
                // Inline FP constants have a binary16 payload and a zero upper half.
                let value = if matches!(s[i], Opnd::ConstF32(_)) {
                    "0.0".to_string()
                } else {
                    format!("f16tof32(({}) >> 16)", r(&s[i])?)
                };
                Ok(Self::float_source(value, ins.aux.mask, i))
            };
            let result = hres(format!("tb_fma_half({}, {}, {})", src(0)?, src(1)?, src(2)?));
            self.touch(&d);
            let dest = r(&d)?;
            return self.wv(&d, &if sel & 8 == 0 {
                format!("(({dest} & 0xffff0000u) | {result})")
            } else {
                format!("(({dest} & 0xffffu) | ({result} << 16))")
            });
        }
        if matches!(m.as_str(), "v_dot4_i32_i8" | "v_dot4c_i32_i8") {
            let compact = ins.enc == Enc::Vop2 && ins.aux.sdwa.is_none();
            let mut rhs = r(&s[1])?;
            let acc = if compact {
                &d
            } else if ins.op == 0x416 {
                // OP_SEL and OP_SEL_HI choose source 1's half for each dot
                // pair. These three combinations were observed in FSPX.
                rhs = match (ins.raw[0] & 0xff00, ins.raw[1] >> 27) {
                    (0x4000, 3) => rhs,
                    (0x5000, 1) => format!("(({rhs} >> 16u) | ({rhs} << 16u))"),
                    (0x4000, 1) => format!("(({rhs} & 65535u) * 0x10001u)"),
                    _ => return Err("unsupported signed byte dot product modifiers".into()),
                };
                &s[2]
            } else {
                return Err("unsupported signed byte dot product encoding".into());
            };
            let lhs = r(&s[0])?;
            let acc = r(acc)?;
            let value = if self.fspx_wave.is_some() {
                format!("asuint(dot4add_i8packed({lhs}, {rhs}, asint({acc})))")
            } else {
                format!("tb_dot4_i8({lhs}, {rhs}, {acc})")
            };
            return self.wv(&d, &value);
        }
        if int16::new_half(ins) && self.ex.stage != Stage::Compute {
            return Err("unverified ordinary 16-bit integer shader stage".into());
        }
        if let Some(result) = int16::expression(ins)? {
            return self.wv(&d, &result);
        }
        if self.lower_f64(ins)? {
            return Ok(());
        }
        if int32::handles(ins) && self.ex.stage != Stage::Compute {
            return Err("unverified ordinary 32-bit integer shader stage".into());
        }
        if let Some(result) = int32::expression(ins)? {
            return self.wv(&d, &result);
        }
        if m == "v_pack_b32_f16" {
            let (w0, w1) = (ins.raw[0], ins.raw[1]);
            if w0 & 0x8700 != 0 || w1 >> 27 != 0 {
                return Err("unsupported packed 16-bit modifiers".into());
            }
            let half = |operand: &Opnd, select| {
                if let Opnd::ConstF32(value) = operand {
                    if select != 0 { return Err("high-half selection on inline float".into()); }
                    return Ok(format!("f32tof16({value:?}f)"));
                }
                int16::half(operand, select)
            };
            let lo = half(&s[0], (w0 >> 11) & 1)?;
            let high = half(&s[1], (w0 >> 12) & 1)?;
            return self.wv(&d, &format!("({lo} | ({high} << 16))"));
        }
        if m == "v_pk_fmac_f16" {
            if ins.enc != Enc::Vop2 || ins.aux.sdwa.is_some() || matches!(s[0], Opnd::ConstF32(_)) {
                return Err("unsupported packed FMAC encoding or inline float".into());
            }
            let (a, b, acc) = (r(&s[0])?, r(&s[1])?, r(&d)?);
            return self.wv(&d, &format!("tb_fmac_half({a}, {b}, {acc}) | (tb_fmac_half({a} >> 16, {b} >> 16, {acc} >> 16) << 16)"));
        }
        if m == "v_cvt_pknorm_u16_f32" {
            if ins.raw[0] & 0xfc00 != 0 || ins.raw[1] & 0x98000000 != 0 {
                return Err("unsupported UNORM16 pack modifiers".into());
            }
            if self.ex.stage == Stage::Pixel {
                // The direct two-channel export is used for R16G16_UNORM. Until
                // additional export formats have provenance, reject a skipped or
                // indirect pack instead of reinterpreting the bits as FP16.
                let e = self.insns.get(self.cur_idx + 1).ok_or("UNORM16 pack without export")?;
                if e.enc != Enc::Exp || e.aux.imm != 0 || e.aux.mask != 3 || e.aux.imm2 & 1 == 0
                    || e.src.first() != Some(&d) || self.insns.iter().any(|i| i.enc == Enc::Sopp
                        && Self::branch_cond(i.op).is_some() && i.offset as i64 + 4 + 4*i.aux.imm == e.offset as i64)
                {
                    return Err("unresolved packed UNORM16 pixel export".into());
                }
            }
            let lo = self.fm(ins, 0)?;
            let hi = self.fm(ins, 1)?;
            return self.wv(&d, &format!("((uint)round(saturate({lo}) * 65535.0) | ((uint)round(saturate({hi}) * 65535.0) << 16))"));
        }
        if m == "v_cvt_pk_u8_f32" {
            if ins.aux.mask & !9 != 0 || ins.raw[0] & 0xf800 != 0 || ins.aux.imm2 != 0 {
                return Err("unsupported byte-pack modifiers".into());
            }
            let x = self.fm(ins, 0)?;
            let t = self.tmp;
            self.tmp += 1;
            self.line(&format!("uint t{t}shift = ({} & 3u) * 8u;", r(&s[1])?));
            let mode = if self.regs.contains("tb_fp_mode") { "tb_fp_mode" } else { "0u" };
            self.line(&format!("uint t{t}byte = tb_round_u8({x}, {mode});"));
            return self.wv(&d, &format!("({} & ~(255u << t{t}shift)) | (t{t}byte << t{t}shift)", r(&s[2])?));
        }
        if matches!(m.as_str(), "v_pk_fma_f16" | "v_pk_add_f16" | "v_pk_mul_f16" | "v_pk_min_f16" | "v_pk_max_f16") {
            // Packed half-float ops (VOP3P): two f16 lanes per register. For the low result,
            // OPSEL bit i (dword0[13:11]) picks the low/high half of source i and NEG bit i
            // (dword1[31:29]) negates it; for the high result OPSEL_HI (dword1[28:27],
            // dword0[14]) and NEG_HI (dword0[10:8]) do the same.
            let (w0, w1) = (ins.raw[0], ins.raw.get(1).copied().unwrap_or(0));
            let nsrc = if m == "v_pk_fma_f16" { 3 } else { 2 };
            let sel = [(w0 >> 11) & 7, ((w1 >> 27) & 3) | (((w0 >> 14) & 1) << 2)];
            let neg = [(w1 >> 29) & 7, (w0 >> 8) & 7];
            let mut halves = Vec::new();
            for h in 0..2 {
                let mut v = Vec::new();
                for i in 0..nsrc {
                    let x = match &s[i] {
                        o @ Opnd::ConstF32(_) => format!("tb_f16_rne({})", f(o)?),
                        o => r(o)?,
                    };
                    let mut e = if (sel[h] >> i) & 1 != 0 { format!("f16tof32(({x}) >> 16)") } else { format!("f16tof32(({x}) & 0xffffu)") };
                    if (neg[h] >> i) & 1 != 0 {
                        e = format!("(-{e})");
                    }
                    v.push(e);
                }
                halves.push(match m.as_str() {
                    "v_pk_fma_f16" => format!("{} * {} + {}", v[0], v[1], v[2]),
                    "v_pk_add_f16" => format!("{} + {}", v[0], v[1]),
                    "v_pk_mul_f16" => format!("{} * {}", v[0], v[1]),
                    "v_pk_min_f16" => format!("min({}, {})", v[0], v[1]),
                    _ => format!("max({}, {})", v[0], v[1]),
                });
            }
            if w0 & (1 << 15) != 0 {
                for value in &mut halves {
                    *value = format!("saturate({value})");
                }
            }
            let pack = if self.single_wave32 && matches!(m.as_str(), "v_pk_min_f16" | "v_pk_max_f16") {
                "tb_repack_half"
            } else {
                "tb_f16_rne"
            };
            return self.wv(&d, &format!("({pack}({}) | ({pack}({}) << 16))", halves[0], halves[1]));
        }
        if matches!(m.as_str(), "v_min3_f16" | "v_max3_f16" | "v_med3_f16") {
            let (a, b, c) = (h16(&s[0])?, h16(&s[1])?, h16(&s[2])?);
            let e = match m.as_str() {
                "v_min3_f16" => format!("min(min({a}, {b}), {c})"),
                "v_max3_f16" => format!("max(max({a}, {b}), {c})"),
                _ => format!("tb_med3({a}, {b}, {c})"),
            };
            return self.wv(&d, &hres(e));
        }
        if m == "v_bcnt_u32_b32" {
            return self.wv(&d, &format!("(countbits({}) + {})", r(&s[0])?, r(&s[1])?));
        }
        if m == "v_bfm_b32" {
            return self.wv(&d, &format!("(((1u << (({}) & 31u)) - 1u) << (({}) & 31u))", r(&s[0])?, r(&s[1])?));
        }
        if m == "v_perm_b32" {
            return self.wv(&d, &format!("tb_perm({}, {}, {})", r(&s[0])?, r(&s[1])?, r(&s[2])?));
        }
        if m == "v_bfe_i32" {
            // Sign-extended bit field: offset = S1[4:0], width = S2[4:0].
            // Width 0 gives 0; a field reaching past bit 31 (offset + width >= 32) is the
            // arithmetic shift by the offset (LLVM AMDGPU's folding of V_BFE_I32; the ISA
            // guide's pseudocode omits the sign extension).
            let (a, o, w) = (r(&s[0])?, r(&s[1])?, r(&s[2])?);
            let t = self.tmp;
            self.tmp += 1;
            self.line(&format!("uint t{t}o = ({o}) & 31u, t{t}w = ({w}) & 31u;"));
            return self.wv(&d, &format!(
                "(t{t}w == 0u ? 0u : ((t{t}o + t{t}w >= 32u) ? asuint(asint({a}) >> t{t}o) : asuint(asint(({a}) << (32u - t{t}o - t{t}w)) >> (32u - t{t}w))))"));
        }
        if matches!(m.as_str(), "v_movrels_b32" | "v_movreld_b32" | "v_movrelsd_b32") {
            // Register-relative moves: the source and/or destination VGPR number is offset by
            // M0 at run time (local arrays kept in registers). Lowered to a chain over every
            // VGPR the program uses.
            let (Opnd::Vgpr(dn), Opnd::Vgpr(sn)) = (d, s[0]) else {
                return Err("register-relative move with a non-VGPR operand".into());
            };
            if !self.vreg_array {
                return Err("register-relative move outside VGPR-array mode".into());
            }
            let t = self.tmp;
            self.tmp += 1;
            let src = if m == "v_movreld_b32" { format!("tb_v[{sn}]") } else { format!("tb_v[({sn}u + m0) & 255u]") };
            self.line(&format!("uint t{t} = {src};"));
            let dst = if m == "v_movrels_b32" { format!("tb_v[{dn}]") } else { format!("tb_v[({dn}u + m0) & 255u]") };
            self.line(&format!("if (exec_lo & 1u) {dst} = t{t};"));
            return Ok(());
        }
        if matches!(m.as_str(), "v_fma_mix_f32" | "v_fma_mixlo_f16" | "v_fma_mixhi_f16") {
            // VOP3P: D = S0 * S1 + S2 where each source is either a full float or a half
            // float taken from the low/high 16 bits of its register and widened:
            //   OPSEL_HI bit i (dword1[28:27], dword0[14] for i = 2) = 0 -> 32-bit float,
            //                                                         1 -> 16-bit float;
            //   OPSEL bit i (dword0[13:11]) picks the high half when the source is 16-bit;
            //   NEG bit i (dword1[31:29]) negates, NEG_HI bit i (dword0[10:8]) takes |x|.
            let (w0, w1) = (ins.raw[0], ins.raw.get(1).copied().unwrap_or(0));
            let opsel = (w0 >> 11) & 7;
            let opsel_hi = ((w1 >> 27) & 3) | (((w0 >> 14) & 1) << 2);
            let (neg, abs) = ((w1 >> 29) & 7, (w0 >> 8) & 7);
            let mut v = Vec::new();
            for i in 0..3 {
                let x = r(&s[i])?;
                let mut e = if (opsel_hi >> i) & 1 != 0 {
                    if (opsel >> i) & 1 != 0 { format!("f16tof32(({x}) >> 16)") } else { format!("f16tof32(({x}) & 0xffffu)") }
                } else {
                    format!("asfloat({x})")
                };
                if (abs >> i) & 1 != 0 {
                    e = format!("abs({e})");
                }
                if (neg >> i) & 1 != 0 {
                    e = format!("(-{e})");
                }
                v.push(e);
            }
            let e = format!("{} * {} + {}", v[0], v[1], v[2]);
            return match m.as_str() {
                "v_fma_mix_f32" => self.wv(&d, &format!("asuint({e})")),
                // The f16 result replaces one half of the destination; the other half is kept.
                "v_fma_mixlo_f16" => {
                    self.touch(&d);
                    self.wv(&d, &format!("(({}) & 0xffff0000u) | tb_f16_rne({e})", r(&d)?))
                }
                _ => {
                    self.touch(&d);
                    self.wv(&d, &format!("(({}) & 0xffffu) | (tb_f16_rne({e}) << 16)", r(&d)?))
                }
            };
        }
        if m == "v_mad_u64_u32" || m == "v_mad_i64_i32" {
            let signed = m == "v_mad_i64_i32";
            let Opnd::Vgpr(low) = d else { return Err("expected VGPR pair".into()); };
            let high = Opnd::Vgpr(low.checked_add(1).ok_or("VGPR pair overflows")?);
            // LLVM writes the carry to the null register when it is unused.
            let cout = match (ins.raw[0] >> 8) & 127 {
                n @ 0..=104 if !signed => Opnd::Sgpr(n as u8),
                106 if !signed => Opnd::VccLo,
                125 => Opnd::Null,
                _ => return Err("unsupported multiply-add carry destination".into()),
            };
            if signed && (self.ex.stage != Stage::Compute
                || !ins.src[..3].iter().all(|s| matches!(s, Opnd::Vgpr(_)))) {
                return Err("unverified signed 64-bit multiply-add source or stage".into());
            }
            if ins.raw[0] & 0x8000 != 0 || ins.raw[1] >> 27 != 0 {
                return Err("unsupported multiply-add modifiers".into());
            }
            let (a, b, c, ch) = (r(&s[0])?, r(&s[1])?, r(&s[2])?, self.pair_high(&s[2])?);
            let t = self.tmp;
            self.tmp += 1;
            // The low product word is the same for signed and unsigned inputs;
            // the signed high word comes from the signed 32x32 product.
            let mulhi = if signed { "tb_mulhi_i32" } else { "tb_mulhi_u32" };
            self.line(&format!("uint t{t}lo = {a} * {b} + {c}; uint t{t}sum = {mulhi}({a}, {b}) + {ch};"));
            self.line(&format!("uint t{t}hi = t{t}sum + uint(t{t}lo < {c}); uint t{t}co = uint(t{t}sum < {ch} || t{t}hi < t{t}sum);"));
            self.wv(&d, &format!("t{t}lo"))?;
            self.touch(&high);
            self.wv(&high, &format!("t{t}hi"))?;
            if cout == Opnd::Null {
                return Ok(());
            }
            if self.resolve_wave32 {
                return self.ws(&cout, &format!("WaveActiveBallot((exec_lo & tb_lane_bit) != 0u && t{t}co != 0u).x"));
            }
            self.ws(&cout, &format!("(exec_lo & 1u) * t{t}co"))?;
            if self.one_word_vector_mask() { return Ok(()); }
            return self.ws(&hi(&cout), "0u");
        }
        if matches!(
            m.as_str(),
            "v_add_co_u32" | "v_sub_co_u32" | "v_subrev_co_u32" | "v_add_co_ci_u32" | "v_sub_co_ci_u32" | "v_subrev_co_ci_u32"
        ) {
            // Add/subtract with carry. VOP3B: carry-out register in bits [14:8] of dword0 and
            // carry-in is src2; VOP2: both are VCC. Lane model: carry = this lane's bit.
            let vop3f = ins.enc == Enc::Vop3;
            let cout = if vop3f {
                match (ins.raw[0] >> 8) & 0x7f {
                    c @ 0..=105 => Opnd::Sgpr(c as u8),
                    106 => Opnd::VccLo,
                    125 if ins.raw[1] >> 27 == 0 => Opnd::Null,
                    c => return Err(format!("carry-out register code {c} unsupported")),
                }
            } else {
                Opnd::VccLo
            };
            let cin = if !m.contains("_ci_") {
                "0u".to_string()
            } else {
                let mask = if vop3f { s[2] } else { Opnd::VccLo };
                if self.persistent_queue {
                    self.touch(&hi(&mask));
                    format!("uint(tb_bit(uint2({}, {}), tb_lane))", r(&mask)?, r(&hi(&mask))?)
                } else if self.resolve_wave32 {
                    format!("uint(({} & tb_lane_bit) != 0u)", r(&mask)?)
                } else { r(&mask)? }
            };
            let (mut a, mut b) = (r(&s[0])?, r(&s[1])?);
            if m.starts_with("v_subrev") {
                core::mem::swap(&mut a, &mut b);
            }
            let t = self.tmp;
            self.tmp += 1;
            let add = m.starts_with("v_add");
            if add {
                self.line(&format!(
                    "uint t{t}c = ({cin}) & 1u; uint t{t}a = {a} + {b}; uint t{t} = t{t}a + t{t}c; uint t{t}o = ((t{t}a < {a}) || (t{t} < t{t}a)) ? 1u : 0u;"
                ));
            } else {
                self.line(&format!(
                    "uint t{t}c = ({cin}) & 1u; uint t{t}a = {a} - {b}; uint t{t} = t{t}a - t{t}c; uint t{t}o = (({b} > {a}) || (t{t}c > t{t}a)) ? 1u : 0u;"
                ));
            }
            if vop3f && ins.raw[0] & 0x8000 != 0 {
                // Clamp the vector result only. SDST reports the original carry or borrow.
                let limit = if add { "0xffffffffu" } else { "0u" };
                self.line(&format!("if (t{t}o != 0u) t{t} = {limit};"));
            }
            self.wv(&d, &format!("t{t}"))?;
            // A null SDST discards the outgoing flag after the incoming mask has been read.
            if cout == Opnd::Null { return Ok(()); }
            self.touch(&cout);
            if self.persistent_queue {
                self.line(&format!("uint2 carry{t} = tb_ballot((exec_lo & t{t}o) != 0u);"));
                self.ws(&cout, &format!("carry{t}.x"))?;
                return self.ws(&hi(&cout), &format!("carry{t}.y"));
            }
            if self.resolve_wave32 {
                return self.ws(&cout, &format!("WaveActiveBallot((exec_lo & tb_lane_bit) != 0u && t{t}o != 0u).x"));
            }
            self.ws(&cout, &format!("((exec_lo & 1u) != 0u) ? t{t}o : 0u"))?;
            if self.one_word_vector_mask() { return Ok(()); }
            let ch = hi(&cout);
            self.touch(&ch);
            self.ws(&ch, "0u")?;
            return Ok(());
        }
        if let Some(sd) = ins.aux.sdwa {
            // SDWA: operate on a selected byte/word of each source. Materialise the selected
            // values into temporaries, then translate the instruction as usual.
            let is_cmp = ins.enc == Enc::Vopc;
            let dst_sel = if is_cmp { 6 } else { (sd >> 8) & 7 };
            if dst_sel == 7 {
                return Err("SDWA destination select 7 is reserved".into());
            }
            let mut c = ins.clone();
            c.aux.sdwa = None;
            if clamped_product24(ins) {
                if sd & 0xc000 != 0 || (sd >> 11) & 3 == 3 {
                    return Err("unsupported SDWA destination control for 24-bit multiply".into());
                }
                c.aux.imm = 1; // Saturate the full result before packing the destination.
            }
            // Reuse VOP3 floating modifiers after source selection and before destination packing.
            let half_alu = half_binary || half_int_result || unary_op.is_some() || matches!(m.as_str(), "v_rcp_f16" | "v_exp_f16" | "v_cvt_f32_f16");
            let float_alu = half_alu || matches!(m.as_str(),
                "v_add_f32" | "v_sub_f32" | "v_subrev_f32" | "v_mul_f32" | "v_mul_legacy_f32"
                | "v_min_f32" | "v_max_f32" | "v_fract_f32" | "v_trunc_f32" | "v_ceil_f32"
                | "v_rndne_f32" | "v_floor_f32");
            // Integer conversions accept floating output modifiers but keep integer source rules.
            let int_to_float = matches!(m.as_str(),
                "v_cvt_f32_i32" | "v_cvt_f32_u32" | "v_cvt_off_f32_i4" | "v_cvt_f32_ubyte0"
                | "v_cvt_f32_ubyte1" | "v_cvt_f32_ubyte2" | "v_cvt_f32_ubyte3");
            let float_source_modifiers = float_alu || m == "v_cvt_f16_f32";
            if float_source_modifiers || int_to_float || half_int_src {
                c.enc = Enc::Vop3;
                c.aux.imm = i64::from((sd >> 13) & 1);
                c.aux.imm2 = i64::from((sd >> 14) & 3);
            }
            if sd & 0x3030_0000 != 0 {
                if !(float_source_modifiers || (is_cmp && (m.ends_with("_f16") || m.ends_with("_f32"))))
                    || (m.contains("_class_") && sd & 0x3000_0000 != 0)
                {
                    return Err("unsupported SDWA neg/abs for this operand type".into());
                }
                // SDWAB Table 89: convert the independent source fields into the
                // same ABS[1:0], NEG[4:3] mask consumed by the VOP3 comparator.
                for (i, shift) in [(0, 16), (1, 24)] {
                    c.aux.mask |= (((sd >> (shift + 5)) & 1) as u8) << i;
                    c.aux.mask |= (((sd >> (shift + 4)) & 1) as u8) << (i + 3);
                }
            }
            for (i, shift) in [(0usize, 16u32), (1usize, 24u32)] {
                if i >= c.src.len() {
                    continue;
                }
                let sel = (sd >> shift) & 7;
                let sext = (sd >> (shift + 3)) & 1 != 0;
                if sel == 6 {
                    continue;
                }
                let mut x = r(&c.src[i])?;
                let half_constant = half_int_src || ((is_cmp || half_alu) && m.ends_with("_f16")
                    && (i == 0 || !m.contains("_class_")));
                if half_constant && matches!(c.src[i], Opnd::ConstF32(_))
                {
                    // The inline constant ROM supplies binary16 before byte/word selection.
                    x = format!("tb_f16_rne(asfloat({x}))");
                }
                let (bits, lsb) = match sel {
                    0..=3 => (8u32, sel * 8),
                    4 | 5 => (16, (sel - 4) * 16),
                    _ => return Err(format!("SDWA select {sel} is reserved")),
                };
                let e = if sext {
                    format!("asuint(asint({x} << {}) >> {})", 32 - bits - lsb, 32 - bits)
                } else {
                    format!("(({x} >> {lsb}) & {:#x}u)", (1u32 << bits) - 1)
                };
                let t = self.tmp;
                self.tmp += 1;
                self.line(&format!("uint tb_s{t} = {e};"));
                c.src[i] = Opnd::Temp(t as u16);
            }
            if dst_sel == 6 && !half_round_or_conversion(&m) && m != "v_cvt_f16_f32" {
                return self.emit_vec(&c);
            }
            // Partial destination: compute into a temporary, then place the selected
            // byte/word into the register. DST_UNUSED (bits 12:11): 0 = pad with zeros,
            // 1 = sign-extend upwards, 2 = keep the other bits of the destination.
            let t = self.tmp;
            self.tmp += 1;
            self.line(&format!("uint tb_s{t} = 0u;"));
            c.dst = vec![Opnd::Temp(t as u16)];
            self.emit_vec(&c)?;
            if dst_sel == 6 {
                // SDWA DWORD writes the entire zero-extended 16-bit result.
                return self.wv(&d, &format!("tb_s{t}"));
            }
            let (bits, lsb) = if dst_sel <= 3 { (8u32, dst_sel * 8) } else { (16, (dst_sel - 4) * 16) };
            let mask = (1u32 << bits) - 1;
            let val = format!("(tb_s{t} & {mask:#x}u)");
            let e = match (sd >> 11) & 3 {
                0 => format!("({val} << {lsb})"),
                1 => format!("asuint(asint({val} << {}) >> {})", 32 - bits, 32 - bits - lsb),
                2 => {
                    self.touch(&d);
                    format!("((({}) & {:#x}u) | ({val} << {lsb}))", r(&d)?, !(mask << lsb))
                }
                _ => return Err("SDWA DST_UNUSED 3 is reserved".into()),
            };
            return self.wv(&d, &e);
        }
        // V_CMPX_*: the comparison result replaces EXEC (inactive lanes give 0). On GFX10 the
        // CMPX forms write only EXEC, not VCC/SDST (LLVM's GFX10 encodings have no SDST for
        // them; the ISA reference says "write exec"). Lane model: this lane's EXEC bit.
        if m.starts_with("v_cmp_") || m.starts_with("v_cmpx_") {
            let mask = ins.aux.mask;
            let c = if m.ends_with("_i64") || m.ends_with("_u64") {
                self.compare_int64(ins, mask)?
            } else if m.ends_with("_f64") || m.ends_with("_f32") || m.ends_with("_f16") {
                self.compare_float(ins, mask)?
            } else {
                self.int_compare(&m, &s[0], &s[1])?
            };
            if self.resolve_wave32 {
                let vote = format!("WaveActiveBallot((exec_lo & tb_lane_bit) != 0u && {c}).x");
                return self.ws(if m.starts_with("v_cmpx_") { &Opnd::ExecLo } else { &d }, &vote);
            }
            if m.starts_with("v_cmpx_") {
                self.line(&format!("exec_lo = (((exec_lo & 1u) != 0u) && {c}) ? 1u : 0u; exec_hi = 0u;"));
                return Ok(());
            }
            if self.packed.as_ref().is_some_and(|p| p.ballots.contains(&self.cur_idx)) {
                let t = self.tmp;
                self.tmp += 1;
                self.line(&format!("uint4 t{t}ballot = WaveActiveBallot(((exec_lo & 1u) != 0u) && {c});"));
                self.ws(&d, &format!("t{t}ballot.x"))?;
                return self.ws(&hi(&d), &format!("t{t}ballot.y"));
            }
            self.ws(&d, &format!("(((exec_lo & 1u) != 0u) && {c}) ? 1u : 0u"))?;
            if self.one_word_vector_mask() { return Ok(()); } // The next SGPR can hold an independent mask.
            let dh = hi(&d);
            if !matches!(dh, Opnd::Null | Opnd::ConstInt(_)) {
                self.touch(&dh);
                self.ws(&dh, "0u")?;
            }
            return Ok(());
        }
        if matches!(m.as_str(), "v_lshlrev_b64" | "v_lshrrev_b64" | "v_ashrrev_i64") {
            // 64-bit vector shifts: value and result are register pairs (lo, hi).
            let pair_hi = |o: &Opnd| match o {
                Opnd::Vgpr(n) => Opnd::Vgpr(n + 1),
                other => hi(other),
            };
            let n = format!("({} & 63u)", r(&s[0])?);
            self.touch(&pair_hi(&s[1]));
            let (lo, hi_e) = (r(&s[1])?, r(&pair_hi(&s[1]))?);
            let t = self.tmp;
            self.tmp += 1;
            let (rl, rh) = match m.as_str() {
                "v_lshlrev_b64" => (
                    format!("(({n} >= 32u) ? 0u : ({lo} << ({n} & 31u)))"),
                    format!("(({n} == 0u) ? {hi_e} : (({n} >= 32u) ? ({lo} << ({n} - 32u)) : (({hi_e} << ({n} & 31u)) | ({lo} >> (32u - ({n} & 31u))))))"),
                ),
                "v_lshrrev_b64" => (
                    format!("(({n} == 0u) ? {lo} : (({n} >= 32u) ? ({hi_e} >> ({n} - 32u)) : (({lo} >> ({n} & 31u)) | ({hi_e} << (32u - ({n} & 31u))))))"),
                    format!("(({n} >= 32u) ? 0u : ({hi_e} >> ({n} & 31u)))"),
                ),
                _ => (
                    format!("(({n} == 0u) ? {lo} : (({n} >= 32u) ? asuint(asint({hi_e}) >> ({n} - 32u)) : (({lo} >> ({n} & 31u)) | ({hi_e} << (32u - ({n} & 31u))))))"),
                    format!("(({n} >= 32u) ? asuint(asint({hi_e}) >> 31) : asuint(asint({hi_e}) >> ({n} & 31u)))"),
                ),
            };
            self.line(&format!("uint t{t}l = {rl}; uint t{t}h = {rh};"));
            self.wv(&d, &format!("t{t}l"))?;
            let dh = pair_hi(&d);
            self.touch(&dh);
            return self.wv(&dh, &format!("t{t}h"));
        }
        if m == "v_swap_b32" {
            let (x, y) = (name(&d)?, name(&s[0])?);
            let t = self.tmp;
            self.tmp += 1;
            self.line(&format!("if (exec_lo & 1u) {{ uint t{t} = {x}; {x} = {y}; {y} = t{t}; }}"));
            return Ok(());
        }
        if m == "v_permlanex16_b32" {
            let flags = (ins.raw[0] >> 11) & 0xf;
            if flags != 0 || ins.aux.mask != 0 || ins.aux.imm != 0 || ins.aux.imm2 != 0 {
                return Err("permlanex16 modifiers unsupported".into());
            }
            let (value, lo, hi) = (r(&s[0])?, r(&s[1])?, r(&s[2])?);
            if self.pixel_wave { return self.wv(&d, if self.pixel_sum { "0u" } else { &value }); }
            if self.portable_wave64 {
                // portable_single_wave permits only the recognized cross-row
                // exchange inside a full-EXEC reduction with selectors zero.
                if s[1..] != [Opnd::ConstInt(0), Opnd::ConstInt(0)] {
                    return Err("unverified portable wave64 cross-row selectors".into());
                }
                let t = self.tmp;
                self.tmp += 1;
                self.line(&format!("uint t{t} = tb_reduction_read(gtid.x, {value}, (gtid.x & ~15u) ^ 16u);"));
                return self.wv(&d, &format!("t{t}"));
            }
            let t = self.tmp;
            self.tmp += 1;
            self.line(&format!("uint t{t}lane = WaveGetLaneIndex(); uint t{t}sel = (t{t}lane & 8u) ? {hi} : {lo};"));
            self.line(&format!("uint t{t}src = ((t{t}lane & ~15u) ^ 16u) | ((t{t}sel >> (4u * (t{t}lane & 7u))) & 15u);"));
            self.line(&format!("uint t{t}value = WaveReadLaneAt({value}, t{t}src);"));
            // FI=0, BOUND_CTRL=0: an inactive source preserves the destination.
            self.line(&format!("uint4 t{t}active = WaveActiveBallot((exec_lo & 1u) != 0u);"));
            self.line(&format!("if (t{t}active[t{t}src >> 5] & (1u << (t{t}src & 31u))) {{"));
            self.wv(&d, &format!("t{t}value"))?;
            self.line("}");
            return Ok(());
        }
        if m == "v_readfirstlane_b32" {
            let value = r(&s[0])?;
            if self.resolve_wave32 {
                return self.ws(&d, &format!("WaveReadLaneAt({value}, exec_lo == 0u ? 0u : uint(firstbitlow(exec_lo)))"));
            }
            if self.ex.stage == Stage::Mesh || (self.is_cs && compute_group(self.ex) == [1, 1, 1]) {
                return self.ws(&d, &value);
            }
            // A pixel wave holds helper lanes, which host wave intrinsics skip. Give
            // each pixel a wave of its own: it elects itself, so a waterfall over
            // heap indices runs once with that pixel's index.
            if self.pixel_heap || lone_lane::on() {
                return self.ws(&d, &value);
            }
            // Scalar writes reach every invocation. Select the first lane whose guest
            // EXEC bit is set, even when the executing host wave also has masked lanes.
            let t = self.tmp;
            self.tmp += 1;
            self.line("#if __SHADER_TARGET_MAJOR >= 6");
            self.line(&format!("uint t{t} = WaveActiveMin((exec_lo & 1u) ? WaveGetLaneIndex() : 0xffffffffu);"));
            self.ws(&d, &format!("WaveReadLaneAt({value}, t{t} == 0xffffffffu ? 0u : t{t})"))?;
            self.line("#else");
            self.ws(&d, &value)?;
            self.line("#endif");
            return Ok(());
        }
        let vop3 = ins.enc == Enc::Vop3;
        let k = format!("asfloat({:#x}u)", ins.raw.last().copied().unwrap_or(0));
        let dn = name(&d)?;
        let fa = || self.fm(ins, 0);
        let fb = || self.fm(ins, 1);
        let fc = || self.fm(ins, 2);
        let a = self.numeric_input(&s[0])?;
        let b = s.get(1).map(|operand| self.numeric_input(operand)).transpose()?.unwrap_or_default();
        let c = s.get(2).map(|o| r(o)).transpose().unwrap_or(None).unwrap_or_default();
        if m == "v_ldexp_f32" {
            let mode = if self.regs.contains("tb_fp_mode") { "tb_fp_mode" } else { "0u" };
            let bits = Self::select_source(ins, a, 0);
            let scale = [0, 1, 2, -1][ins.aux.imm2 as usize];
            let clamp = ins.aux.imm != 0;
            return self.wv(&d, &format!("tb_ldexp_f32({bits}, asint({b}), {mode}, {scale}, {clamp})"));
        }
        if clamped_product24(ins) {
            if vop3 && (ins.raw[0] & 0x7800 != 0 || ins.aux.mask != 0 || ins.aux.imm2 != 0) {
                return Err("unsupported modifiers for clamped 24-bit multiply".into());
            }
            let acc = if m.starts_with("v_mad_") { c.as_str() } else { "0u" };
            let signed = m.ends_with("_i24");
            return self.wv(&d, &format!("tb_mad24_sat({a}, {b}, {acc}, {signed})"));
        }
        // `v_mul_legacy_f32` / `v_mad_legacy_f32`: DX9 multiply rules — the product is 0 when
        // either operand is 0, even against infinity or NaN (`tb_mull`). A plain multiply
        // only matched under fast-math; with exact arithmetic it produced NaN positions.
        let legacy = m.ends_with("_legacy_f32");
        if m.ends_with("_legacy_f32") {
            m = m.replace("_legacy", "");
        }
        let fexpr: Option<String> = match m.as_str() {
            "v_add_f32" => Some(format!("{} + {}", fa()?, fb()?)),
            "v_sub_f32" => Some(format!("{} - {}", fa()?, fb()?)),
            "v_subrev_f32" => Some(format!("{} - {}", fb()?, fa()?)),
            // Lighting multiply saturates to the most negative finite value when S2
            // is unordered/nonpositive or S1 is NaN, -INF or -FLT_MAX.
            "v_mullit_f32" => Some(format!("((asuint({c}) == 0u || asuint({c}) > 0x7f800000u || (asuint({b}) & 0x7fffffffu) > 0x7f800000u || asuint({b}) >= 0xff7fffffu) ? asfloat(0xff7fffffu) : tb_mull({a}, {b}))",
                a=fa()?, b=fb()?, c=fc()?)),
            "v_mul_f32" if legacy => Some(format!("tb_mull({}, {})", fa()?, fb()?)),
            "v_mul_f32" => Some(format!("{} * {}", fa()?, fb()?)),
            "v_min_f32" => Some(format!("min({}, {})", fa()?, fb()?)),
            "v_max_f32" => Some(format!("max({}, {})", fa()?, fb()?)),
            "v_mac_f32" | "v_fmac_f32" => Some(format!("{} * {} + asfloat({dn})", fa()?, fb()?)),
            "v_madmk_f32" | "v_fmamk_f32" => Some(format!("{} * {k} + {}", fa()?, fb()?)),
            "v_madak_f32" | "v_fmaak_f32" => Some(format!("{} * {} + {k}", fa()?, fb()?)),
            "v_mad_f32" if legacy => Some(format!("tb_mull({}, {}) + {}", fa()?, fb()?, fc()?)),
            "v_mad_f32" | "v_fma_f32" => Some(format!("{} * {} + {}", fa()?, fb()?, fc()?)),
            "v_min3_f32" => Some(format!("min(min({}, {}), {})", fa()?, fb()?, fc()?)),
            "v_max3_f32" => Some(format!("max(max({}, {}), {})", fa()?, fb()?, fc()?)),
            // ISA: any NaN operand -> V_MIN3_F32 of the three; otherwise the median.
            "v_med3_f32" => Some(format!("tb_med3({}, {}, {})", fa()?, fb()?, fc()?)),
            "v_cvt_f32_i32" | "v_cvt_f32_u32" if self.regs.contains("tb_fp_mode") => {
                let signed = m == "v_cvt_f32_i32";
                Some(format!("tb_integer_to_f32({a}, {signed}, tb_fp_mode)"))
            }
            "v_cvt_f32_i32" => Some(format!("(float)asint({a})")),
            "v_cvt_f32_u32" => Some(format!("(float)({a})")),
            "v_cvt_off_f32_i4" => Some(format!("((float)(asint({a} << 28) >> 28) * 0.0625f)")),
            "v_cvt_f32_ubyte0" => Some(format!("(float)({a} & 0xffu)")),
            "v_cvt_f32_ubyte1" => Some(format!("(float)(({a} >> 8) & 0xffu)")),
            "v_cvt_f32_ubyte2" => Some(format!("(float)(({a} >> 16) & 0xffu)")),
            "v_cvt_f32_ubyte3" => Some(format!("(float)(({a} >> 24) & 0xffu)")),
            "v_cvt_f32_f16" => Some(h16(&s[0])?),
            "v_fract_f32" => {
                let mode = if self.regs.contains("tb_fp_mode") { "tb_fp_mode" } else { "0u" };
                Some(format!("tb_fract_f32({}, {mode})", fa()?))
            }
            "v_trunc_f32" => Some(format!("trunc({})", fa()?)),
            "v_ceil_f32" => Some(format!("ceil({})", fa()?)),
            "v_rndne_f32" => Some(format!("round({})", fa()?)),
            "v_floor_f32" => Some(format!("floor({})", fa()?)),
            "v_exp_f32" => Some(format!("exp2({})", fa()?)),
            "v_log_f32" => Some(format!("log2({})", fa()?)),
            "v_rcp_f32" | "v_rcp_iflag_f32" => Some(format!("1.0 / {}", fa()?)),
            "v_rsq_f32" => Some(format!("rsqrt({})", fa()?)),
            "v_sqrt_f32" => Some(format!("sqrt({})", fa()?)),
            "v_sin_f32" => Some(format!("sin({} * 6.28318530718)", fa()?)),
            "v_cubema_f32" => Some(format!("tb_cubema({}, {}, {})", fa()?, fb()?, fc()?)),
            "v_cubesc_f32" => Some(format!("tb_cubesc({}, {}, {})", fa()?, fb()?, fc()?)),
            "v_cubetc_f32" => Some(format!("tb_cubetc({}, {}, {})", fa()?, fb()?, fc()?)),
            "v_cubeid_f32" => Some(format!("tb_cubeid({}, {}, {})", fa()?, fb()?, fc()?)),
            "v_cos_f32" => Some(format!("cos({} * 6.28318530718)", fa()?)),
            _ => None,
        };
        if let Some(e) = fexpr {
            let e = self.fres(ins, e);
            return self.wv(&d, &e);
        }
        let mask = if matches!(self.exec_snapshots.sites.get(&self.cur_idx), Some(exec_snapshot::Site::LaneMask(..))) {
            format!("select_mask{}", self.cur_idx)
        } else if self.packed.as_ref().is_some_and(|p| p.lane_masks.contains(&self.cur_idx)) {
            let upper = if self.required_wave_size == Some(64) {
                r(&hi(&s[2]))?
            } else { "0u".into() };
            format!("(((WaveGetLaneIndex() < 32u) ? {c} : {upper}) >> (WaveGetLaneIndex() & 31u))")
        } else if vop3 { c.clone() } else { "vcc_lo".to_string() };
        let e = match m.as_str() {
            "v_mov_b32" => a,
            "v_cndmask_b32" => {
                // E64 permits float sign modifiers on both data sources. Apply them
                // to the bits so signed zero and NaN payloads survive the selection.
                let bit = if self.resolve_wave32 { "tb_lane_bit" } else { "1u" };
                format!("(({mask} & {bit}) != 0u) ? {} : {}", Self::select_source(ins, b, 1), Self::select_source(ins, a, 0))
            }
            "v_and_b32" => format!("({a} & {b})"),
            "v_or_b32" => format!("({a} | {b})"),
            "v_xor_b32" => format!("({a} ^ {b})"),
            "v_xnor_b32" => format!("(~({a} ^ {b}))"),
            "v_not_b32" => format!("(~{a})"),
            "v_bfrev_b32" => format!("reversebits({a})"),
            // Find-first-bit: -1 when no bit is set (HLSL intrinsics agree for zero input).
            "v_ffbl_b32" => format!("firstbitlow({a})"),
            "v_ffbh_u32" => format!("(({a}) == 0u ? 0xffffffffu : (31u - firstbithigh({a})))"),
            "v_lshrrev_b32" => format!("({b} >> ({a} & 31u))"),
            "v_lshlrev_b32" => format!("({b} << ({a} & 31u))"),
            "v_ashrrev_i32" => format!("asuint(asint({b}) >> ({a} & 31u))"),
            // Unsigned VOP3 clamp limits the operand before the operation can wrap.
            "v_add_nc_u32" if vop3 && ins.aux.imm != 0 => format!("(min({a}, ~{b}) + {b})"),
            "v_sub_nc_u32" if vop3 && ins.aux.imm != 0 => format!("(max({a}, {b}) - {b})"),
            "v_subrev_nc_u32" if vop3 && ins.aux.imm != 0 => format!("(max({b}, {a}) - {a})"),
            "v_add_nc_u32" => format!("({a} + {b})"),
            "v_sub_nc_u32" => format!("({a} - {b})"),
            "v_subrev_nc_u32" => format!("({b} - {a})"),
            "v_min_u32" => format!("min({a}, {b})"),
            "v_max_u32" => format!("max({a}, {b})"),
            "v_min_i32" => format!("asuint(min(asint({a}), asint({b})))"),
            "v_max_i32" => format!("asuint(max(asint({a}), asint({b})))"),
            "v_mul_lo_u32" => format!("({a} * {b})"),
            // Count the mask bits below this lane in the selected 32-bit word.
            "v_mbcnt_lo_u32_b32" | "v_mbcnt_hi_u32_b32" => {
                if lone_lane::on() { return self.wv(&d, &b); }
                if self.single_wave32 && m == "v_mbcnt_lo_u32_b32" && s[0] == Opnd::ConstInt(-1) {
                    return self.wv(&d, &format!("({b} + tb_lane)"));
                }
                if self.indexed_lane {
                    let lane = if m == "v_mbcnt_lo_u32_b32" { "tb_lane & 63u" } else { "max(tb_lane & 63u, 32u) - 32u" };
                    return self.wv(&d, &format!("({b} + countbits({a} & {}))", low_mask(lane)));
                }
                if self.ex.stage == Stage::Mesh {
                    // These forms count the low/high 32 lanes preceding this logical wave64 lane.
                    if s[0] != Opnd::ConstInt(-1) { return Err("mesh mbcnt with a dynamic mask".into()); }
                    let count = if m == "v_mbcnt_lo_u32_b32" { "min(lane & 63u, 32u)" } else { "max(lane & 63u, 32u) - 32u" };
                    return self.wv(&d, &format!("{} + ({count})", r(&s[1])?));
                }
                let lane = if m == "v_mbcnt_lo_u32_b32" { "WaveGetLaneIndex()" } else { "max(32u, WaveGetLaneIndex()) - 32u" };
                let t = self.tmp;
                self.tmp += 1;
                self.line("#if __SHADER_TARGET_MAJOR >= 6");
                let a = if self.packed.as_ref().is_some_and(|p| p.scan_width == Some(64))
                    && matches!(s[0], Opnd::VccLo | Opnd::VccHi) {
                    format!("WaveActiveBallot((vcc_lo & 1u) != 0u).{}", if s[0] == Opnd::VccLo { "x" } else { "y" })
                } else if matches!(s[0], Opnd::ExecLo | Opnd::ExecHi) && !self.resolve_wave32 {
                    // Each invocation holds only its own EXEC bit; the guest reads the wave mask.
                    format!("WaveActiveBallot((exec_lo & 1u) != 0u).{}", if s[0] == Opnd::ExecLo { "x" } else { "y" })
                } else { a };
                self.line(&format!("uint t{t} = countbits({a} & {});", low_mask(lane)));
                self.line("#else");
                self.line(&format!("uint t{t} = 0u;"));
                self.line("#endif");
                format!("({b} + t{t})")
            }
            "v_mul_hi_u32" => format!("tb_mulhi_u32({a}, {b})"),
            // Leading bits equal to the sign bit, from the MSB; -1 when all bits are equal.
            "v_ffbh_i32" => format!("(((asint({a}) < 0) ? ~({a}) : ({a})) == 0u ? 0xffffffffu : (31u - firstbithigh((asint({a}) < 0) ? ~({a}) : ({a}))))"),
            // {S0:S1} >> (8 * S2[1:0]), low 32 bits.
            "v_alignbyte_b32" => format!(
                "((({c}) & 3u) == 0u ? {b} : (({b} >> ((({c}) & 3u) * 8u)) | ({a} << (32u - (({c}) & 3u) * 8u))))"
            ),
            "v_mul_hi_i32" => format!("tb_mulhi_i32({a}, {b})"),
            "v_mad_u32_u24" => format!("(({a} & 0xffffffu) * ({b} & 0xffffffu) + {c})"),
            "v_mad_i32_i24" => format!("asuint((asint({a} << 8) >> 8) * (asint({b} << 8) >> 8) + asint({c}))"),
            "v_bfe_u32" => format!("(({a} >> ({b} & 31u)) & ((1u << ({c} & 31u)) - 1u))"),
            "v_bfi_b32" => format!("(({a} & {b}) | (~{a} & {c}))"),
            "v_lerp_u8" => {
                let t = self.tmp;
                self.tmp += 1;
                self.line(&format!("uint diff{t} = {a} ^ {b};"));
                // Mask each byte before shifting so bits cannot spill into its neighbour.
                // Only an odd sum rounds up when the corresponding low bit of src2 is set.
                let floor_avg = format!("(({a} & {b}) + ((diff{t} & 0xfefefefeu) >> 1))");
                format!("({floor_avg} + (diff{t} & {c} & 0x01010101u))")
            }
            "v_lshl_add_u32" => format!("(({a} << ({b} & 31u)) + {c})"),
            "v_add_lshl_u32" => format!("(({a} + {b}) << ({c} & 31u))"),
            "v_add3_u32" => format!("({a} + {b} + {c})"),
            "v_lshl_or_b32" => format!("(({a} << ({b} & 31u)) | {c})"),
            "v_and_or_b32" => format!("(({a} & {b}) | {c})"),
            "v_or3_b32" => format!("({a} | {b} | {c})"),
            "v_xor3_b32" => format!("({a} ^ {b} ^ {c})"),
            "v_xad_u32" => format!("(({a} ^ {b}) + {c})"),
            "v_min3_u32" => format!("min(min({a}, {b}), {c})"),
            "v_max3_u32" => format!("max(max({a}, {b}), {c})"),
            "v_med3_u32" => format!("max(min({a}, {b}), min(max({a}, {b}), {c}))"),
            "v_min3_i32" => format!("asuint(min(min(asint({a}), asint({b})), asint({c})))"),
            "v_max3_i32" => format!("asuint(max(max(asint({a}), asint({b})), asint({c})))"),
            "v_med3_i32" => format!(
                "asuint(max(min(asint({a}), asint({b})), min(max(asint({a}), asint({b})), asint({c}))))"
            ),
            // {S0:S1} >> S2[4:0], low 32 bits (S0 is the high word).
            "v_alignbit_b32" => format!(
                "((({c}) & 31u) == 0u ? {b} : (({b} >> (({c}) & 31u)) | ({a} << (32u - (({c}) & 31u)))))"
            ),
            "v_cvt_u32_f32" => format!("tb_cvt_u32_f32({})", fa()?),
            "v_cvt_i32_f32" => format!("tb_cvt_i32_f32({})", fa()?),
            "v_cvt_flr_i32_f32" => format!("asuint((int)floor({}))", fa()?),
            "v_cvt_rpi_i32_f32" => {
                let x = fa()?;
                let t = self.tmp;
                self.tmp += 1;
                // Adding 0.5 first can round an already integral f32 to the next integer.
                // Start with nearest-even and raise only the ties that rounded down.
                self.line(&format!("float t{t} = round({x});"));
                format!("(asuint((int)t{t}) + (uint)(({x} - t{t}) == 0.5))")
            }
            "v_cvt_f16_f32" => {
                let source = Self::select_source(ins, a, 0);
                let modified = vop3 && (ins.aux.imm != 0 || ins.aux.imm2 != 0);
                let mode = if self.regs.contains("tb_fp_mode") { "tb_fp_mode" } else { "0xc0u" };
                let mut bits = if modified || self.regs.contains("tb_fp_mode") {
                    format!("tb_mode_half(asfloat({source}), {mode})")
                } else {
                    format!("tb_f16_rne(asfloat({source}))")
                };
                if modified {
                    let scale = ["1.0", "2.0", "4.0", "0.5"][ins.aux.imm2 as usize];
                    bits = format!("tb_half_output({bits}, {mode}, {scale}, {})", ins.aux.imm != 0);
                }
                format!("(({dn} & 0xffff0000u) | {bits})")
            }
            // Round toward zero (ISA: "ignore the current rounding mode"): finite values never
            // become infinity (|x| >= 65504 gives 65504).
            "v_cvt_pkrtz_f16_f32" => format!("(tb_f16_rtz({}) | (tb_f16_rtz({}) << 16))", fa()?, fb()?),
            // Pack two signed 16-bit ints from the source into two unsigned 8-bit bytes in
            // the low 16 bits of the destination (high 16 bits cleared). ISA: V_SAT_PK_U8_I16.
            "v_sat_pk_u8_i16" => format!(
                "((uint)clamp(asint({a} << 16) >> 16, 0, 255) | ((uint)clamp(asint({a}) >> 16, 0, 255) << 8))"
            ),
            op => mul24_expr(op, &a, &b).ok_or("unsupported vector op")?,
        };
        self.wv(&d, &e)
    }

    /// Number of dwords moved by a MUBUF dword load/store mnemonic.
    fn mubuf_dwords(m: &str) -> Option<u32> {
        Some(match m.rsplit('_').next()? {
            "dword" | "byte" | "short" | "ubyte" | "sbyte" | "ushort" | "sshort" => 1,
            "dwordx2" => 2,
            "dwordx3" => 3,
            "dwordx4" => 4,
            _ => return None,
        })
    }

    /// The stride of an indexed load through the heap, when another path through the
    /// program loads the same field of the same element at `index << n`.
    fn sibling_stride(&self, ins: &Insn) -> Option<u32> {
        let Some(&Opnd::Vgpr(index)) = ins.src.first() else { return None };
        let here = self.insns.iter().position(|i| i.offset == ins.offset)?;
        let dwords = |m: &str| Self::mubuf_dwords(m).unwrap_or(1) as i64 * 4;
        for (j, shift) in self.insns.iter().enumerate() {
            let k = match (shift.mnemonic.as_str(), shift.src.as_slice()) {
                ("v_lshl_add_u32", [Opnd::Vgpr(v), Opnd::ConstInt(k), ..]) if *v == index => *k,
                ("v_lshlrev_b32", [Opnd::ConstInt(k), Opnd::Vgpr(v), ..]) if *v == index => *k,
                _ => continue,
            };
            if !(2..=10).contains(&k) { continue; }
            let Some(&Opnd::Vgpr(address)) = shift.dst.first() else { continue };
            // The index holds one value throughout.
            let (from, to) = (here.min(j), here.max(j));
            if self.insns[from..to].iter().any(|i| i.dst.contains(&Opnd::Vgpr(index))) { continue; }
            let same = self.insns[j + 1..].iter().take(16)
                .take_while(|i| !i.dst.contains(&Opnd::Vgpr(address)) || i.enc == Enc::Mubuf)
                .any(|i| i.enc == Enc::Mubuf && i.mnemonic.starts_with("buffer_load_dword") && i.aux.mask & 3 == 1
                    && i.src.first() == Some(&Opnd::Vgpr(address))
                    && (i.aux.imm..i.aux.imm + dwords(&i.mnemonic)).contains(&ins.aux.imm));
            if same && ins.aux.imm + dwords(&ins.mnemonic) <= 1 << k { return Some(1 << k); }
        }
        None
    }

    /// Resolve a raw buffer's byte address before any overlapping VGPR is overwritten.
    ///
    /// OFFEN and SOFFSET are byte offsets. A raw descriptor's stride is one byte,
    /// the same stride a shader writes when it builds that descriptor and the size
    /// recorded for a raw binding, so IDXEN is a byte offset too. With both enables
    /// the first address VGPR is the index and the next is the byte offset.
    fn raw_address(&mut self, ins: &Insn) -> Result<String, String> {
        let (indexed, offset) = (ins.aux.mask & 2 != 0, ins.aux.mask & 1 != 0);
        let vector = if indexed {
            let stride = matches!(self.res.get(&self.cur_idx).and_then(|v| v.first()), Some(Res::HeapRaw(_)))
                .then(|| self.sibling_stride(ins)).flatten();
            if (stride.is_none() && !self.raw_byte_index.contains(&ins.offset))
                || !ins.mnemonic.starts_with("buffer_load_dword")
            {
                return Err("raw buffer IDXEN needs the console descriptor stride".into());
            }
            let mut index = r(&ins.src[0])?;
            if let Some(stride) = stride { index = format!("({index} * {stride}u)"); }
            if offset {
                let Opnd::Vgpr(v) = ins.src[0] else { return Err("invalid indexed raw address".into()) };
                let byte_offset = Opnd::Vgpr(v.checked_add(1).ok_or("raw address register overflow")?);
                self.touch(&byte_offset);
                format!("({index} + {})", r(&byte_offset)?)
            } else { index }
        } else if offset { r(&ins.src[0])? } else { "0u".to_string() };
        let address = format!("raw_addr{}", self.tmp);
        self.tmp += 1;
        self.line(&format!("uint {address} = {vector} + {} + {}u;", r(&ins.src[2])?, ins.aux.imm));
        Ok(address)
    }

    fn emit_mtbuf(&mut self, idx: usize, ins: &Insn) -> Result<(), String> {
        let buffer = match self.res.get(&idx).and_then(|r| r.first()).copied() {
            Some(Res::Raw(slot, false)) => format!("raw_sbuf{slot}"),
            Some(Res::HeapRaw(src)) => self.bind_heap(HeapView::Raw, src),
            _ => return Err("typed buffer load has no verified linear root SRV".into()),
        };
        // Only formatted, full-width loads with a byte offset and no TFE.
        let [first, second] = ins.raw[..] else {
            return Err("invalid MTBUF encoding".into());
        };
        if ins.op >= 4
            || ins.aux.mask != 1
            || first & 0xc000 != 0
            || second & 0x01e0_0000 != 0
            || !self.zero_buffer_offset(ins)
        {
            return Err("unsupported typed buffer load addressing or modifiers".into());
        }
        let [Opnd::Vgpr(base)] = ins.dst[..] else {
            return Err("invalid MTBUF destination".into());
        };
        let address = self.raw_address(ins)?;
        // The supported formats consume one naturally aligned dword, except
        // 16_16_16_16_UINT, whose element is 8 bytes. Read them before updating
        // VGPRs in case VADDR overlaps the result.
        let packed = format!("t{}", self.tmp);
        let packed1 = format!("t{}", self.tmp + 1);
        self.tmp += 2;
        if ins.aux.imm2 == 69 {
            self.line(&format!(
                "uint {packed} = 0u; uint {packed1} = 0u; if (exec_lo & 1u) {{ {packed} = {buffer}.Load({address}); {packed1} = {buffer}.Load({address} + 4u); }}"
            ));
        } else {
            self.line(&format!(
                "uint {packed} = 0u; if (exec_lo & 1u) {packed} = {buffer}.Load({address});"
            ));
        }
        for channel in 0..=ins.op {
            let value = match ins.aux.imm2 {
                // BUFFER_FORMAT_16_16_SNORM.
                24 if channel < 2 => format!(
                    "asuint(max(-1.0f, float(asint({packed} << {}u) >> 16) / 32767.0f))",
                    16 - channel * 16
                ),
                24 => "0u".to_string(),
                // BUFFER_FORMAT_2_10_10_10_UNORM.
                50 => {
                    let max = if channel == 3 { 3 } else { 1023 };
                    format!("asuint(float(({packed} >> {}u) & {max}u) / {max}.0f)", channel * 10)
                }
                // BUFFER_FORMAT_8_8_8_8_UNORM / UINT.
                56 => format!("asuint(float(({packed} >> {}u) & 255u) / 255.0f)", channel * 8),
                60 => format!("(({packed} >> {}u) & 255u)", channel * 8),
                // BUFFER_FORMAT_32_UINT / 32_FLOAT: X only, then (0, 0, 1).
                20 | 22 if channel == 0 => packed.clone(),
                20 | 22 => (if channel == 3 { if ins.aux.imm2 == 22 { "asuint(1.0f)" } else { "1u" } } else { "0u" }).to_string(),
                // BUFFER_FORMAT_16_FLOAT.
                13 if channel == 0 => format!("asuint(f16tof32({packed}))"),
                13 => (if channel == 3 { "asuint(1.0f)" } else { "0u" }).to_string(),
                // BUFFER_FORMAT_16_16_16_16_UINT: four 16-bit unsigned channels.
                69 => tbuf_16_16_16_16_uint_channel(channel, &packed, &packed1),
                // BUFFER_FORMAT_16_16_UNORM / UINT / FLOAT.
                23 | 27 | 29 if channel >= 2 => (if channel == 3 { if ins.aux.imm2 == 27 { "1u" } else { "asuint(1.0f)" } } else { "0u" }).to_string(),
                23 => format!("asuint(float(({packed} >> {}u) & 65535u) / 65535.0f)", channel * 16),
                27 => format!("(({packed} >> {}u) & 65535u)", channel * 16),
                29 => format!("asuint(f16tof32({packed} >> {}u))", channel * 16),
                _ => return Err(format!("unsupported typed buffer format {}", ins.aux.imm2)),
            };
            let dst = base
                .checked_add(channel as u8)
                .ok_or("MTBUF destination overflows")?;
            self.wv(&Opnd::Vgpr(dst), &value)?;
        }
        Ok(())
    }

    fn raw_transfer(&mut self, ins: &Insn, slot: u32, is_uav: bool) -> Result<(), String> {
        let buffer = if is_uav { format!("raw_ubuf{slot}") } else { format!("raw_sbuf{slot}") };
        self.raw_transfer_on(ins, buffer, is_uav)
    }

    /// Raw transfer through a heap index held in `src` (compute heap). Stores use the
    /// writable view of the same descriptor.
    fn heap_raw_transfer(&mut self, ins: &Insn, src: u16) -> Result<(), String> {
        let store = ins.mnemonic.starts_with("buffer_store_");
        let view = if store { HeapView::RawRw } else { HeapView::Raw };
        let buf = self.bind_heap(view, src);
        self.raw_transfer_on(ins, buf, store)
    }

    /// `buffer_load_format_*` with IDXEN through a heap descriptor: a typed view,
    /// whose desktop descriptor supplies the element format and stride.
    fn heap_typed_load(&mut self, idx: usize, ins: &Insn, src: u16) -> Result<(), String> {
        if ins.aux.mask & 1 != 0 || ins.aux.imm != 0 || !self.zero_buffer_offset(ins) {
            return Err("typed heap buffer load with a byte offset unsupported".into());
        }
        let channels = match ins.mnemonic.rsplit('_').next() {
            Some("x") => 1u8, Some("xy") => 2, Some("xyz") => 3, Some("xyzw") => 4,
            _ => return Err(format!("unsupported typed heap load {}", ins.mnemonic)),
        };
        let Opnd::Vgpr(vd) = ins.dst[0] else { return Err("bad vdata".into()) };
        let index = r(&ins.src[0])?;
        let t = self.tmp;
        self.tmp += 1;
        if self.heap_load_ty.get(&idx) == Some(&[false, true, false]) {
            let buf = self.bind_heap(HeapView::TypedUint, src);
            self.line(&format!("uint4 t{t} = 0; if (exec_lo & 1u) t{t} = {buf}.Load({index});"));
            for k in 0..channels {
                self.wv(&Opnd::Vgpr(vd + k), &format!("t{t}.{}", XYZW[k as usize]))?;
            }
            return Ok(());
        }
        if self.heap_load_ty.get(&idx).is_some_and(|seen| seen[1] || seen[2]) {
            return Err(format!("typed heap load at {:#x}: element type is ambiguous", ins.offset));
        }
        let buf = self.bind_heap(HeapView::Typed, src);
        self.line(&format!("float4 t{t} = 0; if (exec_lo & 1u) t{t} = {buf}.Load({index});"));
        for k in 0..channels {
            self.wv(&Opnd::Vgpr(vd + k), &format!("asuint(t{t}.{})", XYZW[k as usize]))?;
        }
        Ok(())
    }

    /// `buffer_store_format_*` with IDXEN through a heap descriptor: the typed UAV's
    /// desktop descriptor supplies the element format, so store the float channels.
    fn heap_typed_store(&mut self, ins: &Insn, src: u16) -> Result<(), String> {
        if ins.aux.mask & 1 != 0 || ins.aux.imm != 0 || !self.zero_buffer_offset(ins) {
            return Err("typed heap buffer store with a byte offset unsupported".into());
        }
        let channels = match ins.mnemonic.rsplit('_').next() {
            Some("x") => 1u8, Some("xy") => 2, Some("xyz") => 3, Some("xyzw") => 4,
            _ => return Err(format!("unsupported typed heap store {}", ins.mnemonic)),
        };
        let Opnd::Vgpr(vd) = ins.dst[0] else { return Err("bad vdata".into()) };
        let index = r(&ins.src[0])?;
        let buf = self.bind_heap(HeapView::TypedRw, src);
        let mut value = Vec::new();
        for k in 0..4u8 {
            value.push(if k < channels {
                let o = Opnd::Vgpr(vd + k);
                self.touch(&o);
                format!("asfloat({})", r(&o)?)
            } else if k == 3 { "1.0f".into() } else { "0.0f".into() });
        }
        self.line(&format!("if (exec_lo & 1u) {buf}[{index}] = float4({});", value.join(", ")));
        Ok(())
    }

    fn raw_transfer_on(&mut self, ins: &Insn, buffer: String, is_uav: bool) -> Result<(), String> {
        let load = ins.mnemonic.starts_with("buffer_load_");
        if !load && (!is_uav || !ins.mnemonic.starts_with("buffer_store_")) {
            return Err("unsupported write to raw buffer".into());
        }
        let Opnd::Vgpr(base) = ins.dst[0] else { return Err("raw buffer needs VGPR data".into()); };
        let width = match ins.mnemonic.rsplit('_').next() {
            Some("dword" | "x") => 1, Some("dwordx2") => 2, Some("dwordx3") => 3, Some("dwordx4") => 4,
            _ => return Err(format!("unsupported raw buffer transfer {}", ins.mnemonic)),
        };
        let address = self.raw_address(ins)?;
        let t = self.tmp;
        self.tmp += 1;
        for k in 0..width {
            let offset = format!("{address} + {}u", 4 * k);
            let reg = Opnd::Vgpr(base.checked_add(k).ok_or("raw buffer register overflow")?);
            if load {
                if ins.aux.mask & 2 != 0 {
                    self.line(&format!("uint t{t}_{k} = 0u; if (exec_lo & 1u) t{t}_{k} = {buffer}.Load({offset});"));
                } else {
                    self.line(&format!("uint t{t}_{k} = {buffer}.Load({offset});"));
                }
            } else {
                self.touch(&reg);
                self.line(&format!("if (exec_lo & 1u) {buffer}.Store({offset}, {});", r(&reg)?));
            }
        }
        if load {
            for k in 0..width {
                self.wv(&Opnd::Vgpr(base + k), &format!("t{t}_{k}"))?;
            }
        }
        Ok(())
    }

    /// A conservative first subset: straight-line compute, explicit finite
    /// CBV root table, unchanged base pair, bounded aligned selector, and
    /// only immediate scalar-buffer consumers of the descriptor words.
    fn bounded_cbv_offsets(&self, idx: usize, table: u8, base: u16, dst: u16)
        -> Option<(u8, u32, Vec<(u8,u32)>)>
    {
        use crate::rdna2::clobbers_scalar;
        let ins = self.insns.get(idx)?;
        if self.ex.stage != Stage::Compute || self.packed.is_some() || self.mesh_bindings.is_some()
            || self.compute_heap || table != 4 || base != 4 || ins.op != 2
            || dst < 16 || dst > 102 || dst % 4 != 0
            || self.insns.len() > 32768 || self.insns.iter().any(|op|
                !op.is_complete() || op.enc == Enc::Unknown || op.mnemonic.contains('<')
                || op.mnemonic.starts_with("tb_") || op.mnemonic.starts_with("s_movrel")
                || op.mnemonic.starts_with("s_subvector_loop")
                || op.mnemonic.starts_with("s_cbranch") || matches!(op.mnemonic.as_str(),
                    "s_branch" | "s_setpc_b64" | "s_swappc_b64" | "s_call_b64" |
                    "s_rfe_b64" | "s_trap"))
        { return None; }
        if self.insns[..idx].iter().any(|op| clobbers_scalar(op, 4) || clobbers_scalar(op, 5)) {
            return None;
        }
        let reg = (ins.raw.get(1)? >> 25) as u8;
        if reg > 105 || (dst..dst+4).contains(&u16::from(reg)) { return None; }
        let imm = u32::try_from(ins.aux.imm).ok()?;
        if imm % 32 != 0 { return None; }
        let mask = code_offset::possible_bits(&self.insns, &branch_targets(&self.insns), idx, reg, 8)?;
        if mask == 0 || mask & 31 != 0 || mask.count_ones() > 4 || imm.checked_add(mask)? > 0x00ff_ffff {
            return None;
        }
        let [range] = self.root_table(table)? else { return None };
        if range.range_type != 2 || range.num == 0 || range.num == u32::MAX
            || range.offset_in_table != 0 && range.offset_in_table != u32::MAX {
            return None;
        }
        let mut candidates = Vec::new();
        let mut bits = mask;
        loop {
            let off = imm.checked_add(bits)?;
            let index = off / 32;
            if index >= range.num { return None; }
            let slot = range.base_register.checked_add(index)?;
            if self.ex.resources.iter().filter(|p| p.res_type == 2 && p.space == range.space &&
                p.lower <= slot && slot <= p.upper).count() != 1 { return None; }
            if self.table_binding_space(table, off, 32).ok()? != (Some(2), slot, range.space) {
                return None;
            }
            candidates.push((table, off));
            if bits == 0 { break; }
            bits = (bits - 1) & mask;
        }
        candidates.sort_unstable();
        // The selector stands in for the descriptor; it is not the descriptor
        // data. Until general escape analysis exists, reject any clobber,
        // potential wide source alias or other descriptor use.
        let mut loads = 0usize;
        for op in &self.insns[idx+1..] {
            if (dst..dst+4).any(|reg| clobbers_scalar(op, reg as u8)) { return None; }
            let consumer = op.enc == Enc::Smem && (8..=12).contains(&op.op) &&
                op.src.first() == Some(&Opnd::Sgpr(dst as u8)) &&
                op.raw.get(1).is_some_and(|word| word >> 25 == 0x7d);
            if consumer { loads += 1; }
            if op.enc == Enc::Smem && !consumer && op.raw.get(1).is_some_and(|word|
                (u32::from(dst)..u32::from(dst+4)).contains(&(word >> 25))) {
                return None;
            }
            for (i, src) in op.src.iter().enumerate() {
                if consumer && i == 0 { continue; }
                if matches!(src, Opnd::Sgpr(reg) if (dst.saturating_sub(15)..dst+4).contains(&u16::from(*reg))) {
                    return None;
                }
            }
        }
        if loads == 0 { return None; }
        Some((reg, imm, candidates))
    }

    fn dynamic_code_offset(&self, idx: usize) -> Option<u8> {
        if self.ex.stage != Stage::Compute || self.packed.is_some() || self.mesh_bindings.is_some() {
            return None;
        }
        code_offset::prove(&self.insns, idx, self.ex.isa.len(), self.ex.isa_data.len())
    }

    fn static_smem_offset(&self, ins: &Insn) -> Option<u32> {
        let imm = u32::try_from(ins.aux.imm).ok()?;
        if ins.raw.get(1).copied()? >> 25 == 0x7d { return Some(imm); }
        if self.proven_zero_smem_offset(ins) { return Some(imm); }
        let reg = ins.raw.get(1).copied()? >> 25;
        if reg > 105 { return None; }
        let offset = *self.smem_constants
            .get_or_init(|| mubuf_offsets::proven_smem_constants(&disassemble(&self.ex.isa)))
            .get(&(ins.offset, reg as u8))?;
        let combined = imm.checked_add(offset)?;
        // Static table selectors encode the table in the upper byte.
        // Do not turn unaligned or out-of-range addresses into descriptors.
        (combined <= 0x00ff_ffff && combined % 4 == 0).then_some(combined)
    }

    fn proven_zero_smem_offset(&self, ins: &Insn) -> bool {
        let reg = ins.raw.get(1).copied().unwrap_or(u32::MAX) >> 25;
        reg <= 105 && self.zero_buffer_offsets
            .get_or_init(|| mubuf_offsets::proven_zero(&disassemble(&self.ex.isa)))
            .contains(&(ins.offset, reg as u8))
    }

    fn zero_buffer_offset(&self, ins: &Insn) -> bool {
        match ins.src.get(2) {
            Some(Opnd::ConstInt(0)) => true,
            Some(Opnd::Sgpr(reg)) => self.zero_buffer_offsets
                .get_or_init(|| mubuf_offsets::proven_zero(&disassemble(&self.ex.isa)))
                .contains(&(ins.offset, *reg)),
            _ => false,
        }
    }

    /// MUBUF on a structured buffer: `vaddr` is the element index (IDXEN), the 12-bit
    /// immediate the byte offset inside the element. Dynamic offsets (OFFEN / SOFFSET
    /// register) are not handled yet and fail explicitly.
    fn emit_mubuf(&mut self, idx: usize, ins: &Insn) -> Result<(), String> {
        // LDS (dword0[16]: the data goes to LDS instead of VGPRs) and TFE (dword1[23]: a
        // status dword after the data) change where results land (bit positions from AMD's
        // machine-readable RDNA2 spec).
        if (ins.raw.first().copied().unwrap_or(0) >> 16) & 1 != 0 {
            return Err("MUBUF LDS (load to LDS) unsupported".into());
        }
        if (ins.raw.get(1).copied().unwrap_or(0) >> 23) & 1 != 0 {
            return Err("MUBUF TFE status return unsupported".into());
        }
        if let Some(Res::CbDyn(slot)) = self.res.get(&idx).and_then(|v| v.first()).copied() {
            let (offen, idxen) = (ins.aux.mask & 1 != 0, ins.aux.mask & 2 != 0);
            if (idxen && offen) || !self.zero_buffer_offset(ins) || !ins.mnemonic.starts_with("buffer_load_dword") {
                return Err("constant-buffer MUBUF form unsupported".into());
            }
            let n = Self::mubuf_dwords(&ins.mnemonic).ok_or("unsupported MUBUF op")?;
            let vd = match ins.dst[0] {
                Opnd::Vgpr(v) => v,
                _ => return Err("bad vdata".into()),
            };
            let base = if offen {
                format!("({:#x}u + {})", ins.aux.imm as u32, r(&ins.src[0])?)
            } else if idxen {
                // Indexed access to a constant buffer = a dynamically indexed cbuffer array.
                // Elements of cbuffer arrays are 16-byte registers, which is the stride the
                // CBV descriptor carries (inferred: the title reads 3 dwords per element).
                format!("({:#x}u + {} * 16u)", ins.aux.imm as u32, r(&ins.src[0])?)
            } else {
                format!("{:#x}u", ins.aux.imm as u32)
            };
            let t = self.tmp;
            self.tmp += 1;
            self.line(&format!("uint t{t} = {base} >> 2;"));
            for k in 0..n {
                self.line(&format!("uint t{t}_{k} = cb{slot}[((t{t} + {k}u) >> 2) & 4095u][(t{t} + {k}u) & 3u];"));
            }
            for k in 0..n {
                self.wv(&Opnd::Vgpr(vd + k as u8), &format!("t{t}_{k}"))?;
            }
            return Ok(());
        }
        let resolved = self.res.get(&idx).and_then(|v| v.first()).copied();
        if let Some(Res::HeapRaw(src)) = resolved {
            if ins.mnemonic.contains("_atomic_") {
                return Err("atomic through a heap descriptor unsupported".into());
            }
            if ins.mnemonic.starts_with("buffer_load_format_") && ins.aux.mask & 2 != 0 {
                return self.heap_typed_load(idx, ins, src);
            }
            if ins.mnemonic.starts_with("buffer_store_format_") && ins.aux.mask & 2 != 0 {
                return self.heap_typed_store(ins, src);
            }
            return self.heap_raw_transfer(ins, src);
        }
        if let Some(Res::Raw(slot, is_uav)) = resolved {
            if !ins.mnemonic.contains("_atomic_") {
                return self.raw_transfer(ins, slot, is_uav);
            }
        }
        if ins.mnemonic.contains("_atomic_") {
            // Buffer atomics → HLSL Interlocked* on the UAV element. With GLC set the
            // pre-operation value is returned in vdata.
            let (offen, idxen, glc) = (ins.aux.mask & 1 != 0, ins.aux.mask & 2 != 0, ins.aux.mask & 4 != 0);
            let raw = matches!(resolved, Some(Res::Raw(_, true)));
            let scalar_only = !offen && !idxen && matches!(resolved, Some(Res::Buf(_, true))) && matches!(ins.src[2], Opnd::Sgpr(_));
            if (offen && idxen) || !(raw || scalar_only || self.zero_buffer_offset(ins)) {
                return Err("atomic MUBUF addressing unsupported".into());
            }
            // IDXEN selects an element. OFFEN and SOFFSET add byte offsets.
            // With no register offset the immediate alone selects the counter.
            let index = if idxen { r(&ins.src[0])? } else { "0u".to_string() };
            let receiver = match resolved {
                Some(Res::Raw(slot, true)) => format!("raw_ubuf{slot}."),
                _ => String::new(),
            };
            let target = match (resolved, offen) {
                (Some(Res::Raw(_, true)), _) => self.raw_address(ins)?,
                (Some(Res::Buf(slot, true)), false) if scalar_only => {
                    let nwords = self.sbuf_words((true, slot));
                    let w = format!("(({} + {:#x}u) >> 2)", r(&ins.src[2])?, ins.aux.imm as u32);
                    format!("ubuf{slot}[{w} / {nwords}u].w[{w} % {nwords}u]")
                }
                (Some(Res::TBuf(slot, true)), false) if ins.aux.imm == 0 => format!("utb{slot}[{index}]"),
                (Some(Res::TBuf(slot, true)), true) => format!("utb{slot}[(({}) + {:#x}u) >> 2]", r(&ins.src[0])?, ins.aux.imm as u32),
                (Some(Res::Buf(slot, true)), false) => {
                    let (nwords, w) = (self.sbuf_words((true, slot)), ins.aux.imm as u32 / 4);
                    format!("ubuf{slot}[{index} + {}u].w[{}]", w / nwords, w % nwords)
                }
                (Some(Res::Buf(slot, true)), true) => {
                    let nwords = self.sbuf_words((true, slot));
                    let w = format!("((({}) + {:#x}u) >> 2)", r(&ins.src[0])?, ins.aux.imm as u32);
                    format!("ubuf{slot}[{w} / {nwords}u].w[{w} % {nwords}u]")
                }
                _ => return Err("unresolved atomic target".into()),
            };
            let vdata = ins.dst[0];
            let val = r(&vdata)?;
            let t = self.tmp;
            self.tmp += 1;
            self.line(&format!("uint t{t} = 0u; if (exec_lo & 1u) {{"));
            match ins.mnemonic.as_str() {
                "buffer_atomic_cmpswap" => {
                    let Opnd::Vgpr(v) = vdata else { return Err("expected atomic VGPR pair".into()) };
                    let compare = Opnd::Vgpr(v.checked_add(1).ok_or("atomic register pair overflow")?);
                    self.touch(&compare);
                    self.line(&format!("{receiver}InterlockedCompareExchange({target}, {}, {val}, t{t});", r(&compare)?));
                }
                "buffer_atomic_inc" | "buffer_atomic_dec" | "buffer_atomic_smin" | "buffer_atomic_smax" => {
                    // The UAV is declared uint. Perform signed comparisons and bounded
                    // increment/decrement against the value actually replaced by CAS.
                    let old = format!("old{t}");
                    let next = match ins.mnemonic.as_str() {
                        "buffer_atomic_inc" => format!("{old} >= {val} ? 0u : {old} + 1u"),
                        "buffer_atomic_dec" => format!("({old} == 0u || {old} > {val}) ? {val} : {old} - 1u"),
                        "buffer_atomic_smin" => format!("asuint(min(asint({old}), asint({val})))"),
                        _ => format!("asuint(max(asint({old}), asint({val})))"),
                    };
                    self.line(&format!("uint {old}; [allow_uav_condition] do {{ {old} = t{t}; {receiver}InterlockedCompareExchange({target}, {old}, {next}, t{t}); }} while (t{t} != {old});"));
                }
                op => {
                    let (func, arg) = match op {
                        "buffer_atomic_add" => ("InterlockedAdd", val),
                        "buffer_atomic_sub" => ("InterlockedAdd", format!("(0u - {val})")),
                        "buffer_atomic_umin" => ("InterlockedMin", val),
                        "buffer_atomic_umax" => ("InterlockedMax", val),
                        "buffer_atomic_and" => ("InterlockedAnd", val),
                        "buffer_atomic_or" => ("InterlockedOr", val),
                        "buffer_atomic_xor" => ("InterlockedXor", val),
                        "buffer_atomic_swap" => ("InterlockedExchange", val),
                        other => return Err(format!("{other} unsupported")),
                    };
                    self.line(&format!("{receiver}{func}({target}, {arg}, t{t});"));
                }
            }
            self.line("}");
            if glc {
                self.wv(&vdata, &format!("t{t}"))?;
            }
            return Ok(());
        }
        if ins.mnemonic == "buffer_store_format_d16_xyzw" {
            let Some(Res::TBuf(slot, true)) = self.res.get(&idx).and_then(|v| v.first()).copied() else {
                return Err("D16 store needs a typed UAV".into());
            };
            if self.tbuf_uint.contains(&slot) || ins.aux.mask & 1 != 0 || ins.aux.imm != 0 || !self.zero_buffer_offset(ins) {
                return Err("D16 store with atomic view or offset addressing unsupported".into());
            }
            let index = if ins.aux.mask & 2 != 0 { r(&ins.src[0])? } else { "0u".to_string() };
            let Opnd::Vgpr(vd) = ins.dst[0] else { return Err("invalid D16 data VGPR".into()) };
            let next = Opnd::Vgpr(vd.checked_add(1).ok_or("D16 VGPR pair overflows")?);
            self.touch(&next);
            let (lo, hi) = (r(&ins.dst[0])?, r(&next)?);
            let fields = format!("uint4({lo} & 65535u, {lo} >> 16, {hi} & 65535u, {hi} >> 16)");
            let value = match self.tbuf_ty.get(&(true, slot)) {
                Some(2) => format!("(asint({fields} << 16) >> 16)"),
                Some(_) => fields,
                None => format!("f16tof32({fields})"),
            };
            self.line(&format!("if (exec_lo & 1u) utb{slot}[{index}] = {value};"));
            return Ok(());
        }
        if let Some(Res::TBuf(slot, true)) = self.res.get(&idx).and_then(|v| v.first()).copied() {
            if self.tbuf_uint.contains(&slot) {
                // uint-typed UAV (see atomics): single-component raw loads/stores.
                let (offen, idxen) = (ins.aux.mask & 1 != 0, ins.aux.mask & 2 != 0);
                // A buffer used with atomics has a single 32-bit channel (D3D requires
                // R32_UINT/SINT for Interlocked*), so multi-component `_format_xy/xyz/xyzw`
                // accesses move only X; a load fills the missing channels with (0, 0, 1).
                let comps = match ins.mnemonic.rsplit('_').next() {
                    Some("x") => 1u8,
                    Some("xy") => 2,
                    Some("xyz") => 3,
                    Some("xyzw") => 4,
                    _ => 0,
                };
                if offen || ins.aux.imm != 0 || comps == 0 {
                    return Err("uint typed-buffer MUBUF form unsupported".into());
                }
                // Without IDXEN the access addresses element 0.
                let index = if idxen { r(&ins.src[0])? } else { "0u".to_string() };
                let vd = ins.dst[0];
                if ins.mnemonic.starts_with("buffer_store") {
                    self.touch(&vd);
                    self.line(&format!("if (exec_lo & 1u) utb{slot}[{index}] = {};", r(&vd)?));
                } else {
                    let t = self.tmp;
                    self.tmp += 1;
                    self.line(&format!("uint t{t} = utb{slot}[{index}];"));
                    self.wv(&vd, &format!("t{t}"))?;
                    if let Opnd::Vgpr(v) = vd {
                        for k in 1..comps {
                            self.wv(&Opnd::Vgpr(v + k), if k == 3 { "1u" } else { "0u" })?;
                        }
                    }
                }
                return Ok(());
            }
        }
        if let Some(Res::TBuf(slot, is_uav)) = self.res.get(&idx).and_then(|v| v.first()).copied() {
            // Typed buffer. The element type is declared float4: the view format is not
            // visible in the shader, so integer-format buffers are a known gap (TECHNICAL_DEBT).
            let (offen, idxen) = (ins.aux.mask & 1 != 0, ins.aux.mask & 2 != 0);
            if offen || ins.aux.imm != 0 || !self.zero_buffer_offset(ins) {
                return Err("typed-buffer MUBUF with offset addressing unsupported".into());
            }
            let n = match ins.mnemonic.rsplit('_').next() {
                Some("x") => 1u8,
                Some("xy") => 2,
                Some("xyz") => 3,
                Some("xyzw") => 4,
                _ => return Err("unsupported typed-buffer op".into()),
            };
            // Without IDXEN (and with no offset) the access addresses element 0.
            let index = if idxen { r(&ins.src[0])? } else { "0u".to_string() };
            let vd = match ins.dst[0] {
                Opnd::Vgpr(v) => v,
                _ => return Err("bad vdata".into()),
            };
            let nm = if is_uav { format!("utb{slot}") } else { format!("stb{slot}") };
            if ins.mnemonic.starts_with("buffer_store") {
                if !is_uav {
                    return Err("buffer_store_format to a non-UAV table".into());
                }
                let mut c = vec![];
                for k in 0..4u8 {
                    if k < n {
                        let o = Opnd::Vgpr(vd + k);
                        self.touch(&o);
                        c.push(f(&o)?);
                    } else {
                        c.push("0.0".into());
                    }
                }
                match self.tbuf_ty.get(&(is_uav, slot)) {
                    Some(t) => {
                        let mut c = vec![];
                        for k in 0..4u8 {
                            c.push(if k < n { r(&Opnd::Vgpr(vd + k))? } else { "0u".into() });
                        }
                        let ctor = if *t == 2 { "int4" } else { "uint4" };
                        self.line(&format!("if (exec_lo & 1u) {nm}[{index}] = {ctor}({});", c.join(", ")));
                    }
                    None => self.line(&format!("if (exec_lo & 1u) {nm}[{index}] = float4({});", c.join(", "))),
                }
            } else {
                let t = self.tmp;
                self.tmp += 1;
                let ety = match self.tbuf_ty.get(&(is_uav, slot)) {
                    Some(2) => "int4",
                    Some(_) => "uint4",
                    None => "float4",
                };
                self.line(&format!("{ety} t{t} = {nm}[{index}];"));
                for k in 0..n {
                    let c = XYZW[k as usize];
                    let e = match ety {
                        "float4" => format!("asuint(t{t}.{c})"),
                        "int4" => format!("(uint)t{t}.{c}"),
                        _ => format!("t{t}.{c}"),
                    };
                    self.wv(&Opnd::Vgpr(vd + k), &e)?;
                }
            }
            return Ok(());
        }
        let (slot, is_uav) = match self.res.get(&idx).and_then(|v| v.first()) {
            Some(Res::Buf(s, u)) => (*s, *u),
            _ => return Err("unresolved buffer".into()),
        };
        // Address = element index (IDXEN: first address register) * stride + byte offset
        // (OFFEN: the next address register, or the first when there is no index) + imm.
        let (offen, idxen) = (ins.aux.mask & 1 != 0, ins.aux.mask & 2 != 0);
        // SOFFSET is one more byte term of the address (buffer base + soffset + voffset +
        // index * stride + imm); fold a non-zero one into the run-time byte offset.
        let soff = if self.zero_buffer_offset(ins) {
            None
        } else {
            let o = ins.src.get(2).copied().ok_or("missing MUBUF SOFFSET")?;
            if !matches!(o, Opnd::Sgpr(_) | Opnd::ConstInt(_)) {
                return Err("MUBUF with a dynamic SOFFSET is unsupported".into());
            }
            self.touch(&o);
            Some(r(&o)?)
        };
        let n = Self::mubuf_dwords(&ins.mnemonic).ok_or("unsupported MUBUF op")?;
        let a0 = ins.src[0];
        let a1 = match a0 {
            Opnd::Vgpr(v) => Opnd::Vgpr(v + 1),
            other => other,
        };
        let index = if idxen { r(&a0)? } else { "0u".to_string() };
        let voff = match (idxen, offen) {
            (_, false) => None,
            (false, true) => Some(r(&a0)?),
            (true, true) => {
                self.touch(&a1);
                Some(r(&a1)?)
            }
        };
        let voff = match (voff, soff) {
            (Some(v), Some(s)) => Some(format!("({v}) + ({s})")),
            (v, None) => v,
            (None, s) => s,
        };
        let w0 = ins.aux.imm as u32 / 4;
        let vd = match ins.dst[0] {
            Opnd::Vgpr(v) => v,
            _ => return Err("bad vdata".into()),
        };
        let nm = if is_uav { format!("ubuf{slot}") } else { format!("sbuf{slot}") };
        // Element `k` of this access. With a run-time byte offset the buffer is addressed
        // flat over the declared element size (which assumes, like every structured-buffer
        // access here, that the declared element matches the view's stride).
        let nwords = self.sbuf_words((is_uav, slot));
        let t = self.tmp;
        self.tmp += 1;
        if let Some(vo) = &voff {
            self.line(&format!("uint t{t}w = (({vo}) + {:#x}u) >> 2;", ins.aux.imm as u32));
        }
        let elem = |k: u32| match &voff {
            // A constant offset past the element addresses the following elements.
            None if w0 + k >= nwords => format!("{nm}[{index} + {}u].w[{}]", (w0 + k) / nwords, (w0 + k) % nwords),
            None => format!("{nm}[{index}].w[{}]", w0 + k),
            Some(_) => format!("{nm}[{index} + (t{t}w + {k}u) / {nwords}u].w[(t{t}w + {k}u) % {nwords}u]"),
        };
        let subword = match ins.mnemonic.as_str() {
            "buffer_load_ubyte" | "buffer_load_sbyte" | "buffer_store_byte" => Some(8),
            "buffer_load_ushort" | "buffer_load_sshort" | "buffer_store_short" => Some(16),
            _ => None,
        };
        if let Some(bits) = subword {
            let offset = voff.as_deref().unwrap_or("0u");
            // Short accesses use naturally aligned addresses.
            let align = if bits == 16 { 2 } else { 3 };
            self.line(&format!("uint shift{t} = (({offset} + {}u) & {align}u) * 8u;", ins.aux.imm));
            let word = elem(0);
            let mask = (1u32 << bits) - 1;
            let data = Opnd::Vgpr(vd);
            if ins.mnemonic.starts_with("buffer_store") {
                if !is_uav { return Err("subword store requires a UAV".into()); }
                self.touch(&data);
                let value = r(&data)?;
                self.line(&format!("if (exec_lo & 1u) {{ uint old{t}, seen{t} = 0u; [allow_uav_condition] do {{ old{t} = seen{t}; uint v = (old{t} & ~({mask}u << shift{t})) | (({value} & {mask}u) << shift{t}); InterlockedCompareExchange({word}, old{t}, v, seen{t}); }} while (seen{t} != old{t}); }}"));
                return Ok(());
            }
            let field = format!("(({word} >> shift{t}) & {mask}u)");
            let value = if ins.mnemonic.starts_with("buffer_load_s") {
                format!("asuint(asint({field} << {0}) >> {0})", 32-bits)
            } else { field };
            return self.wv(&data, &value);
        }
        if ins.mnemonic.starts_with("buffer_store") {
            if !is_uav {
                return Err("buffer_store to a non-UAV table".into());
            }
            for k in 0..n {
                let o = Opnd::Vgpr(vd + k as u8);
                self.touch(&o);
                self.line(&format!("if (exec_lo & 1u) {} = {};", elem(k), r(&o)?));
            }
        } else {
            for k in 0..n {
                self.line(&format!("uint t{t}_{k} = {};", elem(k)));
            }
            for k in 0..n {
                self.wv(&Opnd::Vgpr(vd + k as u8), &format!("t{t}_{k}"))?;
            }
        }
        Ok(())
    }

    /// Geometry-shader LDS layout (read off WriteToSliceMainGS): input vertex data lives at
    /// `vertexBase + register*16 + component*4`, emitted vertices at
    /// `primBase + vertex*stride + register*16 + component*4`, stride = 16 * output registers.
    /// The translation seeds the vertex-offset registers with synthetic offsets (vertex k at
    /// dword offset k*0x400, i.e. byte k*0x1000 after the shader's own <<2), so an input
    /// address decodes exactly as (vertex = A >> 12, slot = (A & 0xfff) >> 2).
    fn gs_dims(&self) -> Result<(usize, usize, usize, &'static str, &'static str), String> {
        let gi = self.ex.gs_info.ok_or("geometry shader without PSV0 GS info")?;
        let (prim, nv) = match gi.input_primitive {
            1 => ("point", 1usize),
            2 => ("line", 2),
            3 => ("triangle", 3),
            other => return Err(format!("GS input primitive {other} unsupported")),
        };
        let stream = match gi.output_topology {
            1 => "PointStream",
            3 => "LineStream",
            5 => "TriangleStream",
            other => return Err(format!("GS output topology {other} unsupported")),
        };
        let in_dw = 4 * (self.ex.input_sig.iter().map(|e| e.register).max().unwrap_or(0) as usize + 1);
        let out_dw = 4 * (self.ex.output_sig.iter().map(|e| e.register).max().unwrap_or(0) as usize + 1);
        Ok((nv, in_dw, out_dw, prim, stream))
    }

    fn gs_max_vertices(&self) -> usize {
        let emits = self
            .insns
            .iter()
            .filter(|i| i.enc == Enc::Sopp && i.op == 0x10 && (i.aux.imm as u32 & 0xf) == 2 && (i.aux.imm as u32 >> 4) & 2 != 0)
            .count();
        (self.ex.gs_info.map(|g| g.max_vertex_count as usize).unwrap_or(0)).max(emits).max(1)
    }

    fn emit_gs_lds(&mut self, ins: &Insn) -> Result<(), String> {
        let (nv, in_dw, out_dw, _, _) = self.gs_dims()?;
        let gs_dw = out_dw * self.gs_max_vertices();
        let off = ins.aux.imm as u32;
        let (off0, off1) = (off & 0xff, (off >> 8) & 0xff);
        let addr = r(&ins.src[0])?;
        let t = self.tmp;
        self.tmp += 1;
        let in_at = |a: String| format!("tb_in[({a} >> 12) % {nv}u][(({a} & 0xfffu) >> 2) % {in_dw}u]");
        let vd = match ins.dst[0] {
            Opnd::Vgpr(v) => v,
            _ => return Err("bad vdst".into()),
        };
        match ins.mnemonic.as_str() {
            "ds_read_b32" => {
                self.line(&format!("uint t{t} = {};", in_at(format!("({addr} + {:#x}u)", off & 0xffff))));
                self.wv(&Opnd::Vgpr(vd), &format!("t{t}"))
            }
            "ds_read2_b32" => {
                self.line(&format!(
                    "uint t{t}a = {}; uint t{t}b = {};",
                    in_at(format!("({addr} + {:#x}u)", off0 * 4)),
                    in_at(format!("({addr} + {:#x}u)", off1 * 4))
                ));
                self.wv(&Opnd::Vgpr(vd), &format!("t{t}a"))?;
                self.wv(&Opnd::Vgpr(vd + 1), &format!("t{t}b"))
            }
            "ds_write_b32" => {
                self.line(&format!(
                    "if (exec_lo & 1u) tb_gs[(({addr} + {:#x}u) >> 2) % {gs_dw}u] = {};",
                    off & 0xffff,
                    r(&ins.src[1])?
                ));
                Ok(())
            }
            "ds_write2_b32" => {
                self.line(&format!(
                    "if (exec_lo & 1u) {{ tb_gs[(({addr} + {:#x}u) >> 2) % {gs_dw}u] = {}; tb_gs[(({addr} + {:#x}u) >> 2) % {gs_dw}u] = {}; }}",
                    off0 * 4,
                    r(&ins.src[1])?,
                    off1 * 4,
                    r(&ins.src[2])?
                ));
                Ok(())
            }
            _ => Err("unsupported LDS op".into()),
        }
    }

    /// `s_sendmsg`: in a geometry shader, message 2 = GS with op bits (1 = cut, 2 = emit),
    /// message 3 = GS_DONE.
    fn emit_sendmsg(&mut self, ins: &Insn) -> Result<(), String> {
        if !self.is_gs {
            return Err("s_sendmsg outside a geometry shader".into());
        }
        let imm = ins.aux.imm as u32 & 0xffff;
        let (msg, op, stream) = (imm & 0xf, (imm >> 4) & 3, (imm >> 8) & 3);
        match msg {
            3 => Ok(()),
            2 => {
                if stream != 0 {
                    return Err("GS output stream != 0 unsupported".into());
                }
                let (_, _, out_dw, _, _) = self.gs_dims()?;
                let maxv = self.gs_max_vertices();
                if op & 2 != 0 {
                    let mut body = format!("if (exec_lo & 1u) {{ SOut o = (SOut)0; uint tb_b = min(tb_emit, {}u) * {out_dw}u;", maxv - 1);
                    for (ei, e) in self.ex.output_sig.iter().enumerate() {
                        let (ty, _) = sig_type(e);
                        let comps: Vec<String> = (0..4)
                            .filter(|c| (e.mask >> c) & 1 != 0)
                            .map(|c| {
                                let w = format!("tb_gs[tb_b + {}u]", e.register as usize * 4 + c);
                                if e.comp_type == 3 { format!("asfloat({w})") } else { w }
                            })
                            .collect();
                        let _ = write!(body, " o.o{ei} = ({ty}){};", if comps.len() == 1 { comps[0].clone() } else { format!("{ty}({})", comps.join(", ")) });
                    }
                    body.push_str(" stream.Append(o); tb_emit++; }");
                    self.line(&body);
                }
                if op & 1 != 0 {
                    self.line("if (exec_lo & 1u) stream.RestartStrip();");
                }
                Ok(())
            }
            other => Err(format!("s_sendmsg message {other} unsupported")),
        }
    }

    fn require_compute_derivatives(&mut self) -> Result<(), String> {
        if self.is_cs && !self.compute_derivatives {
            // SM 6.6 groups derivatives into runs of four X lanes or 2x2 XY quads.
            let [x, y, z] = self.ex.psv_threads.ok_or("missing compute derivative thread group")?;
            let linear = x % 4 == 0 && y == 1 && z == 1;
            let planar = x % 2 == 0 && y % 2 == 0;
            if x == 0 || y == 0 || z == 0 || !(linear || planar) {
                return Err(format!("compute derivatives need a quad-compatible thread group, got {x}x{y}x{z}"));
            }
            self.compute_derivatives = true;
        }
        Ok(())
    }

    fn emit_mimg(&mut self, idx: usize, ins: &Insn) -> Result<(), String> {
        if self.packed.as_ref().is_some_and(|p| p.wide_images.contains(&idx)) {
            return packed_root::ps09_raster::emit_wide_image(self, ins, "wide_u10");
        }
        let rs = self.res.get(&idx).cloned().ok_or("unresolved image")?;
        // Instruction bits that change operand or result layout (RDNA2 ISA Table 41; bit
        // positions from AMD's machine-readable RDNA2 spec): UNORM dword0[12], TFE [16],
        // LWE [17]; A16 dword1[30], D16 [31].
        let w0 = ins.raw.first().copied().unwrap_or(0);
        let w1 = ins.raw.get(1).copied().unwrap_or(0);
        let unorm = (w0 >> 12) & 1 != 0;
        let a16 = (w1 >> 30) & 1 != 0;
        let d16 = (w1 >> 31) & 1 != 0;
        if (w0 >> 16) & 3 != 0 {
            // TFE / LWE append a status dword (texel fault for partially resident textures,
            // LOD warning) after the data.
            return Err("MIMG TFE/LWE status return unsupported".into());
        }
        // Address operand `i` (in dwords, or 16-bit halves when `a16`) as an expression:
        // non-NSA addresses are consecutive VGPRs from vaddr, NSA lists each VGPR.
        let nsa = ins.aux.imm2 as usize;
        let vaddr = match ins.src[0] {
            Opnd::Vgpr(n) => n,
            _ => return Err("bad vaddr".into()),
        };
        let addr_reg = |i: usize| -> Opnd { if nsa > 0 { ins.src[i] } else { Opnd::Vgpr(vaddr + i as u8) } };
        // Texel-addressed ops (no sampler): image_load / image_store, on SRVs or UAVs.
        let atomic = match ins.mnemonic.as_str() {
            "image_atomic_add" => Some("InterlockedAdd"),
            "image_atomic_umin" => Some("InterlockedMin"),
            "image_atomic_umax" => Some("InterlockedMax"),
            "image_atomic_or" => Some("InterlockedOr"),
            _ => None,
        };
        let heap_load = match rs.first().copied() {
            Some(Res::HeapTexture(src, dim)) if matches!(ins.mnemonic.as_str(), "image_load" | "image_load_mip") => Some((src, dim)),
            _ => None,
        };
        if let (Some(Res::HeapTexture(src, 1)), "image_store", false) = (rs.first().copied(), ins.mnemonic.as_str(), d16) {
            let mut c = vec![];
            for i in 0..2 {
                let o = addr_reg(if a16 { i / 2 } else { i });
                self.touch(&o);
                c.push(if a16 { format!("(({} >> {}u) & 0xffffu)", r(&o)?, 16 * (i % 2)) } else { r(&o)? });
            }
            let Opnd::Vgpr(vd) = ins.dst[0] else { return Err("bad vdata".into()) };
            let (mut comp, mut n) = (vec![], 0u8);
            for ch in 0..4 {
                if (ins.aux.mask >> ch) & 1 != 0 {
                    let o = Opnd::Vgpr(vd + n);
                    self.touch(&o);
                    comp.push(r(&o)?);
                    n += 1;
                } else {
                    comp.push("0u".into());
                }
            }
            let tex = self.bind_heap(HeapView::Tex2DRwUint, src);
            self.line(&format!("if (exec_lo & 1u) {tex}[uint2({})] = uint4({});", c.join(", "), comp.join(", ")));
            return Ok(());
        }
        if let Some((src, dim)) = heap_load {
            let seen = self.heap_load_ty.get(&idx).copied();
            let uint = self.compute_heap && seen == Some([false, true, false]) && dim == 1 && !d16;
            if !uint && self.pixel_heap && seen != Some([true, false, false]) {
                return Err("heap texel load of an unknown component type in a pixel shader".into());
            }
            if self.ex.stage == Stage::Vertex && seen.is_some_and(|seen| seen[1] || seen[2]) {
                return Err(format!("heap texel load at {:#x}: integer view unsupported", ins.offset));
            }
            // Texel load through a heap image descriptor (compute heap). Unless every use of
            // the texel takes an integer, a float view is assumed.
            let (view, ncoord) = match dim {
                1 if uint => (HeapView::Tex2DUint, 2),
                0 => (HeapView::Tex1D, 1), 1 => (HeapView::Tex2D, 2), 2 => (HeapView::Tex3D, 3),
                4 => (HeapView::Tex1DArray, 2), 5 => (HeapView::Tex2DArray, 3),
                _ => return Err(format!("heap texture load dim {dim} unsupported")),
            };
            let mut c = vec![];
            for i in 0..ncoord + usize::from(ins.mnemonic == "image_load_mip") {
                // With A16 each address is a 16-bit uint, two per VGPR.
                let o = addr_reg(if a16 { i / 2 } else { i });
                self.touch(&o);
                c.push(if a16 {
                    format!("(int)(({} >> {}u) & 0xffffu)", r(&o)?, 16 * (i % 2))
                } else {
                    format!("(int){}", r(&o)?)
                });
            }
            if ins.mnemonic == "image_load" {
                c.push("0".into());
            }
            let Opnd::Vgpr(vd) = ins.dst[0] else { return Err("bad vdata".into()) };
            let t = self.tmp;
            self.tmp += 1;
            let tex = self.bind_heap(view, src);
            let (ty, cast) = if uint { ("uint4", "") } else { ("float4", "asuint") };
            self.line(&format!("{ty} t{t} = {tex}.Load(int{}({}));", c.len(), c.join(", ")));
            if d16 {
                let chans: Vec<String> = (0..4).filter(|ch| (ins.aux.mask >> ch) & 1 != 0).map(|ch| format!("t{t}.{}", XYZW[ch])).collect();
                return self.write_d16(vd, &chans);
            }
            let mut n = 0u8;
            for ch in 0..4 {
                if (ins.aux.mask >> ch) & 1 != 0 {
                    self.wv(&Opnd::Vgpr(vd + n), &format!("{cast}(t{t}.{})", XYZW[ch]))?;
                    n += 1;
                }
            }
            return Ok(());
        }
        if atomic.is_some() || matches!(ins.mnemonic.as_str(), "image_load" | "image_load_mip" | "image_store" | "image_store_mip") {
            let with_mip = ins.mnemonic == "image_load_mip";
            let (slot, dim, is_uav) = match rs.first() {
                Some(Res::Tex(t, d)) => (*t, *d, false),
                Some(Res::Uav(t, d)) => (*t, *d, true),
                _ => return Err("unresolved image".into()),
            };
            // Integer address `i`: a dword, or with A16 a 16-bit uint, two per VGPR.
            let addr = |em: &mut Self, i: usize| -> Result<String, String> {
                if a16 {
                    let o = addr_reg(i / 2);
                    em.touch(&o);
                    Ok(format!("(({} >> {}u) & 0xffffu)", r(&o)?, 16 * (i % 2)))
                } else {
                    let o = addr_reg(i);
                    em.touch(&o);
                    r(&o)
                }
            };
            let ncoord = match dim {
                0 => 1,     // 1D
                1 => 2,     // 2D
                2 | 5 => 3, // 3D / 2D array
                4 => 2,     // 1D array
                6 => 2,     // 2D multisampled (the next component is the sample index)
                _ => return Err(format!("image dim {dim} unsupported for load/store")),
            };
            let store = ins.mnemonic.starts_with("image_store");
            if ins.mnemonic == "image_store_mip" {
                // Desktop UAV descriptors select one mip at view creation.
                if a16 || !self.immediate_zero_before(idx, addr_reg(ncoord)) {
                    return Err("image_store_mip requires a statically zero relative mip".into());
                }
            }
            let mut c = vec![];
            for i in 0..ncoord {
                c.push(addr(self, i)?);
            }
            let vd = match ins.dst[0] {
                Opnd::Vgpr(n) => n,
                _ => return Err("bad vdata".into()),
            };
            let as_uint = is_uav && self.uav_uint.contains(&slot);
            let ity = self.img_ty.get(&(is_uav, slot)).copied();
            // A packed zero stored to a scalar uint UAV survives either widening convention.
            let uint_zero = store && as_uint && ins.aux.mask == 1 && self.immediate_zero_before(idx, Opnd::Vgpr(vd));
            if d16 && !uint_zero && (as_uint || ity.is_some() || ins.mnemonic.starts_with("image_atomic_")) {
                // The conversion of integer formats to 16 bits is not specified.
                return Err("D16 image access to an integer-typed image unsupported".into());
            }
            if let Some(func) = atomic {
                // Image atomics → Interlocked* on a RWTexture<uint> texel; vdata carries the
                // operand in and the previous value out.
                if ins.aux.mask == 3 && as_uint && dim == 1 {
                    let visibility = matches!(self.ex.stage, Stage::Pixel | Stage::Compute)
                        && wide_atomic_coordinates(ins).is_ok()
                        && self.wide_visibility_uavs.contains(&slot);
                    if !visibility {
                        return Err("unknown two-word visibility atomic".into());
                    }
                    self.wide_visibility_uavs.insert(slot);
                    return packed_root::ps09_raster::emit_wide_image(self, ins, &format!("uav{slot}"));
                }
                if ins.aux.mask != 1 {
                    return Err("image atomic requires a verified two-word binding for DMASK=3 or DMASK=1 for one word".into());
                }
                if !as_uint {
                    return Err("image atomic on a non-UAV table".into());
                }
                let o = Opnd::Vgpr(vd);
                self.touch(&o);
                let t = self.tmp;
                self.tmp += 1;
                self.line(&format!("uint t{t} = 0u;"));
                self.line(&format!("if (exec_lo & 1u) {func}(uav{slot}[uint{ncoord}({})], {}, t{t});", c.join(", "), r(&o)?));
                if ins.raw[0] & (1 << 13) != 0 { // GLC requests the old value in VDATA.
                    self.wv(&o, &format!("t{t}"))?;
                }
                return Ok(());
            }
            if store && as_uint {
                let o = Opnd::Vgpr(vd);
                self.touch(&o);
                self.line(&format!("if (exec_lo & 1u) uav{slot}[uint{ncoord}({})] = {};", c.join(", "), r(&o)?));
                return Ok(());
            }
            if ins.mnemonic == "image_load" && as_uint {
                let t = self.tmp;
                self.tmp += 1;
                self.line(&format!("uint4 t{t} = uint4(uav{slot}[uint{ncoord}({})], 0u, 0u, 0u);", c.join(", ")));
                let mut n = 0u8;
                for ch in 0..4 {
                    if (ins.aux.mask >> ch) & 1 != 0 {
                        self.wv(&Opnd::Vgpr(vd + n), &format!("t{t}.{}", XYZW[ch]))?;
                        n += 1;
                    }
                }
                return Ok(());
            }
            if store {
                if !is_uav {
                    return Err("image_store to a non-UAV table".into());
                }
                if let Some(t) = ity {
                    // Integer texels: the registers are stored as they are.
                    let mut comp = vec![];
                    let mut n = 0u8;
                    for ch in 0..4 {
                        if (ins.aux.mask >> ch) & 1 != 0 {
                            let o = Opnd::Vgpr(vd + n);
                            self.touch(&o);
                            comp.push(r(&o)?);
                            n += 1;
                        } else {
                            comp.push("0u".into());
                        }
                    }
                    let ctor = if t == 2 { "int4" } else { "uint4" };
                    self.line(&format!("if (exec_lo & 1u) uav{slot}[uint{ncoord}({})] = {ctor}({});", c.join(", "), comp.join(", ")));
                    return Ok(());
                }
                // Enabled components come from consecutive VGPRs (D16: halves of consecutive
                // VGPRs, low half first); missing components are stored as 0.
                let mut comp = vec![];
                let mut n = 0u8;
                for ch in 0..4 {
                    if (ins.aux.mask >> ch) & 1 != 0 {
                        let o = Opnd::Vgpr(vd + if d16 { n / 2 } else { n });
                        self.touch(&o);
                        comp.push(if d16 { format!("f16tof32({} >> {}u)", r(&o)?, 16 * (n % 2)) } else { f(&o)? });
                        n += 1;
                    } else {
                        comp.push("0.0".into());
                    }
                }
                self.line(&format!(
                    "if (exec_lo & 1u) uav{slot}[uint{ncoord}({})] = float4({});",
                    c.join(", "),
                    comp.join(", ")
                ));
            } else {
                let t = self.tmp;
                self.tmp += 1;
                // image_load_mip carries the mip level as one more address component.
                let mip = if with_mip { format!("(int){}", addr(self, ncoord)?) } else { "0".to_string() };
                let e = if dim == 6 {
                    if is_uav {
                        return Err("multisampled UAV load unsupported".into());
                    }
                    let sample = addr(self, ncoord)?;
                    format!("tex{slot}.Load(int2({}), (int){sample})", c.iter().map(|x| format!("(int){x}")).collect::<Vec<_>>().join(", "))
                } else if is_uav {
                    if with_mip {
                        return Err("image_load_mip on a UAV unsupported".into());
                    }
                    format!("uav{slot}[uint{ncoord}({})]", c.join(", "))
                } else {
                    format!("tex{slot}.Load(int{}({}, {mip}))", ncoord + 1, c.iter().map(|x| format!("(int){x}")).collect::<Vec<_>>().join(", "))
                };
                let ety = match ity {
                    Some(2) => "int4",
                    Some(_) => "uint4",
                    None => "float4",
                };
                self.line(&format!("{ety} t{t} = {e};"));
                let chans: Vec<String> = (0..4).filter(|ch| (ins.aux.mask >> ch) & 1 != 0).map(|ch| format!("t{t}.{}", XYZW[ch])).collect();
                if d16 {
                    return self.write_d16(vd, &chans);
                }
                for (n, x) in chans.iter().enumerate() {
                    let e = match ety {
                        "float4" => format!("asuint({x})"),
                        "int4" => format!("(uint){x}"),
                        _ => x.clone(),
                    };
                    self.wv(&Opnd::Vgpr(vd + n as u8), &e)?;
                }
            }
            return Ok(());
        }
        let (tex, dim) = match rs.first() {
            Some(Res::Tex(slot, d)) => (format!("tex{slot}"), *d),
            Some(Res::HeapTexture(src, dim)) => {
                let (src, dim) = (*src, *dim);
                let view = match dim {
                    1 => HeapView::Tex2D,
                    3 => HeapView::TexCube,
                    _ => HeapView::Tex2DArray,
                };
                (self.bind_heap(view, src), dim)
            }
            _ => return Err("unresolved image".into()),
        };
        if ins.mnemonic == "image_get_resinfo" {
            if dim != 1 || a16 || d16 || ins.aux.mask & 4 != 0 {
                return Err("image_get_resinfo supports 2D width, height and mip count".into());
            }
            let t = self.tmp;
            self.tmp += 1;
            let Opnd::Vgpr(vd) = ins.dst[0] else { return Err("bad resinfo destination".into()) };
            self.line(&format!("uint w{t}, h{t}, levels{t}; {tex}.GetDimensions({}, w{t}, h{t}, levels{t});", r(&ins.src[0])?));
            let mut dst = vd;
            for (bit, field) in [(1,"w"), (2,"h"), (8,"levels")] {
                if ins.aux.mask & bit != 0 {
                    self.wv(&Opnd::Vgpr(dst), &format!("{field}{t}"))?;
                    dst = dst.wrapping_add(1);
                }
            }
            return Ok(());
        }
        let sslot = match rs.get(1) {
            Some(Res::Samp(s)) => Some(*s),
            Some(Res::HeapSampler(_) | Res::HeapSamplerDyn(_)) => None,
            _ => return Err("image op without sampler unsupported".into()),
        };
        // RDNA2 sample/gather family. Opcode = family base (0x20 sample, 0x40 gather4) +
        // modifier (0 none, 1 cl, 2 d, 3 d_cl, 4 l, 5 b, 6 b_cl, 7 lz) + 8 (depth compare)
        // + 0x10 (texel offset). Address operands arrive in this order:
        //   [offset] [bias] [zref] [ddx.. ddy..] coords.. [lod] [lod clamp]
        let op = ins.op;
        let gather = (0x40..0x60).contains(&op);
        if !gather && op != 0x60 && !(0x20..0x40).contains(&op) {
            return Err("unsupported image op".into());
        }
        let (modi, cmp, has_off) = (op & 7, op & 8 != 0, op & 0x10 != 0);
        // (coordinate count, derivative components, offset components)
        let (ncoord, nder, noff) = match dim {
            0 => (1usize, 1usize, 1usize),
            1 => (2, 2, 2),
            2 => (3, 3, 3),
            3 => (3, 3, 0),
            4 => (2, 1, 1),
            5 => (3, 2, 2),
            _ => return Err(format!("image dim {dim} unsupported")),
        };
        // Address operands. [offset] [bias] [zref] take a dword each, then the derivatives,
        // then coordinates, lod and clamp. With A16 (ISA Table 41: "PCF reference ... is
        // 32bit"; layout as Mesa's ACO emits it, aco_select_nir.cpp visit_tex /
        // emit_pack_v1): the bias is an f16 in the low half of its own dword, derivatives
        // stay 32-bit (16-bit derivatives are the separate _g16 opcodes), and coordinates,
        // lod and clamp are f16s packed two per VGPR, continuing across those operands.
        let mut next = 0usize;
        let mut take = |em: &mut Self| -> Opnd {
            let o = addr_reg(next);
            next += 1;
            em.touch(&o);
            o
        };
        // Texel offset: packed signed 6-bit fields (x [5:0], y [13:8], z [21:16]).
        let offset = if has_off {
            let o = take(self);
            if noff == 0 {
                return Err("texel offset on a cube image".into());
            }
            let sx = |v: u32, sh: u32| (((v >> sh) & 0x3f) as i32) << 26 >> 26;
            let reg = match o {
                Opnd::Vgpr(n) => n,
                _ => return Err("bad offset operand".into()),
            };
            match self.const_vgpr(idx, reg) {
                Some(v) => {
                    let c: Vec<String> = (0..noff).map(|k| sx(v, 8 * k as u32).to_string()).collect();
                    Some(if noff == 1 { c[0].clone() } else { format!("int{noff}({})", c.join(", ")) })
                }
                None if gather => {
                    // SM5 Gather accepts a run-time offset.
                    let x = r(&o)?;
                    let c: Vec<String> =
                        (0..noff).map(|k| format!("(asint({x} << {}) >> 26)", 26 - 8 * k)).collect();
                    Some(if noff == 1 { c[0].clone() } else { format!("int{noff}({})", c.join(", ")) })
                }
                None => return Err("non-constant texel offset on a sample op".into()),
            }
        } else {
            None
        };
        let bias = if matches!(modi, 5 | 6) {
            let o = take(self);
            Some(if a16 { format!("f16tof32({})", r(&o)?) } else { f(&o)? })
        } else {
            None
        };
        let zref = if cmp { Some(f(&take(self))?) } else { None };
        let grads = if matches!(modi, 2 | 3) {
            let mut dx = vec![];
            for _ in 0..nder {
                dx.push(f(&take(self))?);
            }
            let mut dy = vec![];
            for _ in 0..nder {
                dy.push(f(&take(self))?);
            }
            let v = |c: &Vec<String>| if c.len() == 1 { c[0].clone() } else { format!("float{}({})", c.len(), c.join(", ")) };
            Some((v(&dx), v(&dy)))
        } else {
            None
        };
        let base = next;
        let mut half = 0usize;
        let mut takef = |em: &mut Self| -> Result<String, String> {
            let k = half;
            half += 1;
            if a16 {
                let o = addr_reg(base + k / 2);
                em.touch(&o);
                Ok(format!("f16tof32({} >> {}u)", r(&o)?, 16 * (k % 2)))
            } else {
                let o = addr_reg(base + k);
                em.touch(&o);
                f(&o)
            }
        };
        let mut coords = vec![];
        for _ in 0..ncoord {
            coords.push(takef(self)?);
        }
        let lod = if modi == 4 { Some(takef(self)?) } else { None };
        let clamp = if matches!(modi, 1 | 3 | 6) { Some(takef(self)?) } else { None };
        if unorm {
            // Unnormalised (texel-space) coordinates: divide by the size of the view's mip 0.
            // Array slices are never normalised.
            let (outs, n) = match dim {
                0 => ("w, l", 1),
                4 => ("w, e, l", 1),
                1 => ("w, h, l", 2),
                5 => ("w, h, e, l", 2),
                2 => ("w, h, d, l", 3),
                _ => return Err("unnormalised coordinates on a cube image unsupported".into()),
            };
            let t = self.tmp;
            self.tmp += 1;
            let decl: Vec<String> = outs.split(", ").map(|v| format!("{v}{t}")).collect();
            self.line(&format!("uint {}; {tex}.GetDimensions(0u, {});", decl.join(", "), decl.join(", ")));
            for (k, sz) in ["w", "h", "d"].iter().take(n).enumerate() {
                coords[k] = format!("({} / (float){sz}{t})", coords[k]);
            }
        }
        // Cube images are addressed by hardware (s, t, face) coordinates (see tb_cube_dir).
        let cvec = if dim == 3 {
            format!("tb_cube_dir({})", coords.join(", "))
        } else if ncoord == 1 {
            coords[0].clone()
        } else {
            format!("float{ncoord}({})", coords.join(", "))
        };
        let smp = if let Some(Res::HeapSampler(key)) = rs.get(1).copied() {
            let name = format!("tb_sampler{idx}");
            let ty = if cmp { "SamplerComparisonState" } else { "SamplerState" };
            self.line(&format!("{ty} {name} = SamplerDescriptorHeap[tb_index{}_{}.{}];",
                key.register, key.space, XYZW[key.component as usize]));
            name
        } else if let Some(Res::HeapSamplerDyn(reg)) = rs.get(1).copied() {
            let name = format!("tb_sampler{idx}");
            let ty = if cmp { "SamplerComparisonState" } else { "SamplerState" };
            let index = ssrc_name(u32::from(reg))?;
            self.regs.insert(index.clone());
            self.line(&format!("{ty} {name} = SamplerDescriptorHeap[NonUniformResourceIndex({index})];"));
            name
        } else if let Some(s) = sslot {
            if cmp { format!("scmp{s}") } else { format!("samp{s}") }
        } else { return Err("missing sampler binding".into()); };
        let zero_off = if noff == 1 { "0".to_string() } else { format!("int{}({})", noff.max(1), vec!["0"; noff.max(1)].join(", ")) };
        // Optional trailing ", offset[, clamp]" arguments.
        let tail = |offset: &Option<String>, clamp: &Option<String>| -> Result<String, String> {
            Ok(match (offset, clamp) {
                (None, None) => String::new(),
                (Some(o), None) => format!(", {o}"),
                (o, Some(c)) => {
                    if noff == 0 {
                        return Err("LOD clamp on a cube image unsupported".into());
                    }
                    format!(", {}, {c}", o.clone().unwrap_or_else(|| zero_off.clone()))
                }
            })
        };
        let t = self.tmp;
        self.tmp += 1;
        let vd = match ins.dst[0] {
            Opnd::Vgpr(n) => n,
            _ => return Err("bad vdata".into()),
        };
        if op == 0x60 {
            if ins.aux.mask & !3 != 0 || d16 {
                return Err("image_get_lod requires 32-bit X/Y results".into());
            }
            self.require_compute_derivatives()?;
            self.line(&format!("float2 t{t} = float2({tex}.CalculateLevelOfDetail({smp}, {cvec}), {tex}.CalculateLevelOfDetailUnclamped({smp}, {cvec}));"));
            let mut dst = vd;
            for (bit, component) in [(1, "x"), (2,"y")] {
                if ins.aux.mask & bit != 0 {
                    self.wv(&Opnd::Vgpr(dst), &format!("asuint(t{t}.{component})"))?;
                    dst = dst.wrapping_add(1);
                }
            }
            return Ok(());
        }
        if gather {
            // gather4: dmask picks the channel; the four texels fill vdata..vdata+3.
            // These hierarchical depth passes convert their chosen integer mip to
            // f32 before issuing four red gathers on the same texture and sampler.
            let tslot = match rs.first().copied() {
                Some(Res::Tex(tslot, _)) => tslot,
                Some(Res::HeapTexture(..)) if modi != 4 && !cmp => u32::MAX,
                _ => return Err("unsupported direct heap gather".into()),
            };
            let depth_gather = match (self.ex.entry.as_deref(), tslot) {
                (Some("VirtualSmInvalidateInstancePagesCS"), 4) => {
                    self.wave_cull || packed_root::matches_program(&self.ex.isa, 7304, 0x8a6c22fd0266c12c)
                }
                (Some("GenerateConservativeDepthBufferCS"), 0) => {
                    packed_root::matches_program(&self.ex.isa, 812, 0xe3633db262ecebf9)
                }
                _ => false,
            };
            if modi == 4 && dim == 1 && !cmp && !has_off && !d16 && !a16 && !unorm
                && ins.aux.mask == 1 && sslot == Some(0)
                && ((self.is_cs && (matches!((self.ex.entry.as_deref(), tslot), (Some("InstanceCull" | "InstanceCullVSM"), 3) | (Some("PersistentClusterCull"), 3 | 5)) || (self.wave_cull && self.ex.entry.as_deref() == Some("CullPerPageDrawCommandsCs") && tslot == 0) || depth_gather
                    || (self.ex.entry.as_deref() == Some("InstanceCullBuildInstanceIdBufferCS") && tslot == 4
                        && [
                            (7072, 0x1fe3061314fe6030), (9944, 0x41f68c7d6b4a40ec),
                            (4492, 0x21c1b76c01e1b9ff), (7428, 0x94a6f79d724d3e25),
                            (7480, 0xf939a9961a9370bf), (10112, 0x056954bb8a896632),
                            (4372, 0x8279f28dabfc3ed1), (7008, 0xa4b009faa821ebab),
                            (6952, 0x54a23d7bfe154e74), (4316, 0xc0486c5c6f4e7160),
                            (7536, 0x6ae08e11efe46334), (7372, 0x17c731ece44188a8),
                            (10052, 0xf857b533d91c99c5), (4436, 0x7d6788e101f4109b),
                            (10008, 0x8685661900d8e524), (6912, 0x9381ab449cbbb186),
                        ].into_iter().any(|(size, hash)| packed_root::matches_program(&self.ex.isa, size, hash)))))
                    || (self.ex.stage == Stage::Pixel && tslot == 0
                        && self.ex.entry.as_deref() == Some("HZBTestPS")
                        && packed_root::matches_program(&self.ex.isa, 2108, 0x49d7bcccdaa0f843)))
                && matches!(ins.raw.first(), Some(0xf1100108 | 0xf110010a))
            {
                // This HZB program supplies an integer mip and a point sampler.
                // Gather around floor(uv * size - 0.5) in D3D component order.
                // Sampling texel centres preserves the bound sampler's edge mode.
                let lod = lod.as_ref().ok_or("missing HZB gather mip")?;
                self.line(&format!("uint w{t}, h{t}, m{t}; {tex}.GetDimensions(0u, w{t}, h{t}, m{t});"));
                self.line(&format!("uint l{t} = (uint)clamp({lod}, 0.0, float(max(1u, m{t}) - 1u));"));
                self.line(&format!("{tex}.GetDimensions(l{t}, w{t}, h{t}, m{t}); float2 sz{t} = max(float2(w{t},h{t}), 1.0);"));
                self.line(&format!("float2 base{t} = floor({cvec} * sz{t} - 0.5) + 0.5;"));
                let taps = ["0,1", "1,1", "1,0", "0,0"].map(|xy|
                    format!("{tex}.SampleLevel({smp}, (base{t} + float2({xy}))/sz{t}, l{t}).r"));
                self.line(&format!("float4 t{t} = float4({});", taps.join(", ")));
                for (k, channel) in XYZW.iter().enumerate() {
                    self.wv(&Opnd::Vgpr(vd + k as u8), &format!("asuint(t{t}.{channel})"))?;
                }
                return Ok(());
            }
            if modi == 4 && zref.is_none() && matches!(dim, 1 | 5) {
                // gather4_l: SM 6.x Gather has no LOD operand. Sample the four texel
                // centres of the footprint at the requested mip instead. A sample exactly at
                // a texel centre returns that texel under point or bilinear filtering, and
                // keeps the bound sampler's address mode. The mip rounds to the nearest level.
                let chan = match ins.aux.mask {
                    1 => "r",
                    2 => "g",
                    4 => "b",
                    8 => "a",
                    m => return Err(format!("gather4 with dmask {m:#x} unsupported")),
                };
                let lod = lod.as_ref().ok_or("missing gather4_l lod")?;
                let (uv, slice, dims, ext) = if dim == 5 {
                    (format!("({cvec}).xy"), Some(format!("({cvec}).z")), format!("w{t}, h{t}, e{t}, m{t}"), format!("uint e{t};"))
                } else {
                    (cvec.clone(), None, format!("w{t}, h{t}, m{t}"), String::new())
                };
                self.line(&format!("uint w{t}, h{t}, m{t}; {ext} {tex}.GetDimensions(0u, {dims});"));
                self.line(&format!("uint l{t} = (uint)clamp(round({lod}), 0.0, float(max(1u, m{t}) - 1u));"));
                self.line(&format!("{tex}.GetDimensions(l{t}, {dims}); float2 sz{t} = max(float2(w{t}, h{t}), 1.0);"));
                let off2 = offset.as_ref().map(|o| format!(" + float2({o})")).unwrap_or_default();
                self.line(&format!("float2 base{t} = floor({uv} * sz{t} - 0.5) + 0.5{off2};"));
                // D3D gather order: (0,1), (1,1), (1,0), (0,0) relative to the top-left texel.
                let taps = ["0,1", "1,1", "1,0", "0,0"].map(|xy| {
                    let p = format!("(base{t} + float2({xy})) / sz{t}");
                    let p = match &slice { Some(s) => format!("float3({p}, {s})"), None => p };
                    format!("{tex}.SampleLevel({smp}, {p}, l{t}).{chan}")
                });
                self.line(&format!("float4 t{t} = float4({});", taps.join(", ")));
                if d16 {
                    let chans: Vec<String> = XYZW.iter().map(|c| format!("t{t}.{c}")).collect();
                    return self.write_d16(vd, &chans);
                }
                for (k, channel) in XYZW.iter().enumerate() {
                    self.wv(&Opnd::Vgpr(vd + k as u8), &format!("asuint(t{t}.{channel})"))?;
                }
                return Ok(());
            }
            if !matches!(modi, 0 | 7) {
                return Err("gather4 with explicit LOD/bias unsupported".into());
            }
            let chan = match ins.aux.mask {
                1 => "Red",
                2 => "Green",
                4 => "Blue",
                8 => "Alpha",
                m => return Err(format!("gather4 with dmask {m:#x} unsupported")),
            };
            let off = offset.map(|o| format!(", {o}")).unwrap_or_default();
            let call = match &zref {
                Some(z) => format!("{tex}.GatherCmp{chan}({smp}, {cvec}, {z}{off})"),
                None => format!("{tex}.Gather{chan}({smp}, {cvec}{off})"),
            };
            self.line(&format!("float4 t{t} = {call};"));
            if d16 {
                let chans: Vec<String> = XYZW.iter().map(|c| format!("t{t}.{c}")).collect();
                return self.write_d16(vd, &chans);
            }
            for k in 0..4u8 {
                self.wv(&Opnd::Vgpr(vd + k), &format!("asuint(t{t}.{})", XYZW[k as usize]))?;
            }
            return Ok(());
        }
        // FXC (SM5) refuses implicit-derivative samples inside a loop it cannot unroll
        // (X3570/X3511). The console GPU computes those derivatives per quad regardless of
        // control flow; there is no SM5 equivalent, so inside emitted loops such samples
        // are APPROXIMATED with LOD 0 (exact for single-mip textures, too sharp otherwise).
        let in_loop = self.loop_depth > 0;
        if in_loop && matches!(modi, 0 | 1 | 5 | 6) {
            self.line("// APPROX-LOOP-SAMPLE: implicit-derivative sample inside a loop emitted as LOD 0");
        }
        let call = if let Some(z) = &zref {
            // Depth-compare sampling returns a single value.
            match modi {
                0 if in_loop => format!("{tex}.SampleCmpLevelZero({smp}, {cvec}, {z}{})", tail(&offset, &None)?),
                0 => format!("{tex}.SampleCmp({smp}, {cvec}, {z}{})", tail(&offset, &None)?),
                4 if !a16 && self.immediate_zero_before(idx, addr_reg(base + ncoord)) => format!("{tex}.SampleCmpLevelZero({smp}, {cvec}, {z}{})", tail(&offset, &None)?),
                7 => format!("{tex}.SampleCmpLevelZero({smp}, {cvec}, {z}{})", tail(&offset, &None)?),
                _ => return Err("depth-compare sample with explicit LOD/bias/gradient unsupported in SM5".into()),
            }
        } else {
            match modi {
                0 | 1 if in_loop => format!("{tex}.SampleLevel({smp}, {cvec}, 0.0{})", tail(&offset, &None)?),
                5 | 6 if in_loop => format!("{tex}.SampleLevel({smp}, {cvec}, {}{})", bias.clone().ok_or("missing bias")?, tail(&offset, &None)?),
                0 | 1 => format!("{tex}.Sample({smp}, {cvec}{})", tail(&offset, &clamp)?),
                2 | 3 => {
                    let (dx, dy) = grads.clone().ok_or("missing gradients")?;
                    format!("{tex}.SampleGrad({smp}, {cvec}, {dx}, {dy}{})", tail(&offset, &clamp)?)
                }
                4 => format!("{tex}.SampleLevel({smp}, {cvec}, {}{})", lod.clone().ok_or("missing lod")?, tail(&offset, &None)?),
                5 | 6 => format!("{tex}.SampleBias({smp}, {cvec}, {}{})", bias.clone().ok_or("missing bias")?, tail(&offset, &clamp)?),
                _ => format!("{tex}.SampleLevel({smp}, {cvec}, 0.0{})", tail(&offset, &None)?),
            }
        };
        if !in_loop && matches!(modi, 0 | 1 | 5 | 6) {
            self.require_compute_derivatives()?;
        }
        if zref.is_some() {
            self.line(&format!("float t{t} = {call};"));
            // The compare result is replicated to every requested channel.
            if d16 {
                let chans: Vec<String> = (0..ins.aux.mask.count_ones()).map(|_| format!("t{t}")).collect();
                return self.write_d16(vd, &chans);
            }
            let mut n = 0u8;
            for ch in 0..4 {
                if (ins.aux.mask >> ch) & 1 != 0 {
                    self.wv(&Opnd::Vgpr(vd + n), &format!("asuint(t{t})"))?;
                    n += 1;
                }
            }
            return Ok(());
        }
        self.line(&format!("float4 t{t} = {call};"));
        if d16 {
            let chans: Vec<String> = (0..4).filter(|ch| (ins.aux.mask >> ch) & 1 != 0).map(|ch| format!("t{t}.{}", XYZW[ch])).collect();
            return self.write_d16(vd, &chans);
        }
        let mut n = 0u8;
        for ch in 0..4 {
            if (ins.aux.mask >> ch) & 1 != 0 {
                let dst = Opnd::Vgpr(vd + n);
                self.wv(&dst, &format!("asuint(t{t}.{})", XYZW[ch]))?;
                n += 1;
            }
        }
        Ok(())
    }

    /// D16 image result: `vals` (float expressions) converted to f16 (round to nearest
    /// even) and packed two per VGPR from `vd`, low half first (ISA Table 41, D16 / DMASK).
    /// With an odd count the high half of the last VGPR is written as 0.
    fn write_d16(&mut self, vd: u8, vals: &[String]) -> Result<(), String> {
        for (k, pair) in vals.chunks(2).enumerate() {
            let lo = format!("tb_f16_rne({})", pair[0]);
            let e = match pair.get(1) {
                Some(hi) => format!("({lo} | (tb_f16_rne({hi}) << 16))"),
                None => lo,
            };
            self.wv(&Opnd::Vgpr(vd + k as u8), &e)?;
        }
        Ok(())
    }

    /// A literal move immediately preceding the use, with no branch entering after
    /// the move. The two instructions therefore see the same active lanes.
    fn immediate_zero_before(&self, idx: usize, reg: Opnd) -> bool {
        let Some(mov) = self.insns[..idx].iter().rev().find(|i| i.mnemonic != "s_waitcnt") else { return false; };
        mov.enc == Enc::Vop1 && mov.mnemonic == "v_mov_b32" && mov.aux.sdwa.is_none()
            && mov.dst.first() == Some(&reg)
            && matches!(mov.src.first(), Some(Opnd::ConstInt(0) | Opnd::Literal(0)))
            && !self.insns.iter().any(|i| {
                let target = i.offset as i64 + 4 + 4 * i.aux.imm;
                i.enc == Enc::Sopp && Self::branch_cond(i.op).is_some()
                    && target > mov.offset as i64 && target <= self.insns[idx].offset as i64
            })
    }

    /// Statically known value of a VGPR at instruction `idx`: the most recent write in
    /// program order must be a `v_mov_b32` of a constant. Used where HLSL demands a
    /// literal (texel offsets).
    fn const_vgpr(&self, idx: usize, reg: u8) -> Option<u32> {
        for ins in self.insns[..idx].iter().rev() {
            if ins.dst.iter().any(|d| matches!(d, Opnd::Vgpr(n) if *n == reg)) {
                if ins.mnemonic != "v_mov_b32" {
                    return None;
                }
                return match ins.src.first() {
                    Some(Opnd::ConstInt(v)) => Some(*v as i32 as u32),
                    Some(Opnd::Literal(v)) => Some(*v),
                    Some(Opnd::ConstF32(f)) => Some(f.to_bits()),
                    _ => None,
                };
            }
        }
        None
    }

    fn emit_exp(&mut self, ins: &Insn, is_ps: bool) -> Result<(), String> {
        let target = ins.aux.imm as u32;
        let en = ins.aux.mask;
        let compr = ins.aux.imm2 & 1 != 0;
        let mut comp = vec![];
        if compr {
            for i in 0..4usize {
                let src = &ins.src[i / 2];
                let on = (en >> (i / 2 * 2)) & 1 != 0 || (en >> i) & 1 != 0;
                comp.push(if on {
                    let word = format!("(({} >> {}) & 0xffffu)", r(src)?, (i % 2) * 16);
                    if self.cur_idx.checked_sub(1).and_then(|idx| self.insns.get(idx))
                        .is_some_and(|prior| prior.mnemonic == "v_cvt_pknorm_u16_f32" && prior.dst.first() == Some(src))
                    {
                        format!("((float){word} / 65535.0)")
                    } else {
                        format!("f16tof32({word})")
                    }
                } else {
                    "0.0".into()
                });
            }
        } else {
            for i in 0..4usize {
                comp.push(if (en >> i) & 1 != 0 { f(&ins.src[i])? } else { "0.0".into() });
            }
        }
        let v4 = format!("float4({})", comp.join(", "));
        // The exported registers as raw 32-bit values, when they are available unpacked.
        let raw: Option<Vec<String>> = if compr {
            self.epilog_exp.get(&self.cur_idx).copied().map(|(a, b)| {
                (0..4usize)
                    .map(|i| {
                        let id = if i < 2 { a } else { b };
                        let on = (en >> (i / 2 * 2)) & 1 != 0 || (en >> i) & 1 != 0;
                        if on && id != usize::MAX { format!("tb_pk{id}{}", if i % 2 == 0 { "a" } else { "b" }) } else { "0u".to_string() }
                    })
                    .collect()
            })
        } else {
            let mut v = vec![];
            for i in 0..4usize {
                v.push(if (en >> i) & 1 != 0 { r(&ins.src[i])? } else { "0u".into() });
            }
            Some(v)
        };
        if is_ps && ins.aux.imm2 & 4 != 0 {
            // Valid-mask export: lanes that are inactive here are killed (pixel discard).
            self.line("if ((exec_lo & 1u) == 0u) discard;");
        }
        if is_ps && target == 9 {
            return Ok(()); // null export (no colour written)
        }
        if is_ps && target == 8 {
            // MRTZ exports depth in X and the per-fragment stencil reference in Y.
            // Keep their independent channel enables and their respective float/integer types.
            if compr || en == 0 || en & !3 != 0 {
                return Err(format!("unsupported MRTZ export format (mask={en}, compressed={compr})"));
            }
            if en & 1 != 0 {
                if !self.ex.output_sig.iter().any(|e| matches!(e.sysval, 65 | 67 | 68)) {
                    return Err("depth export without an SV_Depth output".into());
                }
                self.line(&format!("if (exec_lo & 1u) o.od = {};", f(&ins.src[0])?));
            }
            if en & 2 != 0 {
                let slot = self.ex.output_sig.iter().position(|e| e.sysval == 69 && e.comp_type == 1 && e.mask == 1)
                    .ok_or("stencil export without a scalar uint SV_StencilRef output")?;
                self.line(&format!("if (exec_lo & 1u) o.o{slot} = {};", r(&ins.src[1])?));
            }
            return Ok(());
        }
        if !is_ps && (32..64).contains(&target) && (target - 32) as usize >= self.out_params.len() {
            // Hardware needs at least one parameter export; with no matching output
            // element nothing downstream can read it.
            self.line("// (parameter export with no output-signature element: ignored)");
            return Ok(());
        }
        // (element index, swizzle of the exported register that feeds it)
        let osig = &self.ex.output_sig;
        let mut assigns: Vec<(usize, String)> = Vec::new();
        if is_ps {
            if target > 7 {
                return Err(format!("PS export target {target} unsupported"));
            }
            // MRT export k feeds the k-th declared SV_Target (outputs are compacted:
            // a shader writing SV_Target0 and SV_Target4 exports MRT 0 and MRT 1).
            let ei = osig
                .iter()
                .enumerate()
                .filter(|(_, e)| e.sysval == 64)
                .nth(target as usize)
                .map(|(i, _)| i)
                .ok_or_else(|| format!("MRT export {target} has no SV_Target output element"))?;
            assigns.push((ei, "xyzw"[..sig_type(&osig[ei]).1].to_string()));
        } else if (12..=15).contains(&target) {
            // Clip distances follow POS0 when there is no miscellaneous position
            // export (point size, layer, viewport, etc.). The hardware mask is
            // in plane order, independent of the D3D output register number.
            if target == 13 && self.ex.stage == Stage::Vertex && !compr
                && osig.iter().any(|e| e.sysval == 1)
                && !osig.iter().any(|e| e.name.eq_ignore_ascii_case("PSIZE"))
            {
                let mut extra = osig.iter().enumerate().filter(|(_, e)| !matches!(e.sysval, 0 | 1));
                if let Some((ei, clip)) = extra.next() {
                    // The export enables all four channels; only the declared planes are used.
                    if matches!(clip.sysval, 2 | 3) && clip.index == 0 && clip.comp_type == 3
                        && en & clip.mask == clip.mask && matches!(clip.mask, 1 | 3 | 7 | 15) && extra.next().is_none()
                    {
                        let swizzle = &"xyzw"[..clip.mask.count_ones() as usize];
                        self.line(&format!("if (exec_lo & 1u) o.o{ei} = {v4}.{swizzle};"));
                        return Ok(());
                    }
                }
            }
            // RDNA2's miscellaneous position export packs the render-target
            // array index into POS1.z[10:0]. Output-signature packing describes
            // D3D registers, not the component used by the hardware export.
            if target == 13 && self.ex.stage == Stage::Vertex && !compr && en & 4 != 0
                && osig.iter().all(|e| matches!(e.sysval, 0 | 1 | 4))
            {
                if let Some(ei) = osig.iter().position(|e| e.sysval == 4 && e.comp_type == 1) {
                    self.line(&format!("if (exec_lo & 1u) o.o{ei} = {} & 0x7ffu;", r(&ins.src[2])?));
                    return Ok(());
                }
            }
            if target != 12 {
                return Err(format!("position export {target} unsupported"));
            }
            match osig.iter().position(|e| e.sysval == 1) {
                Some(ei) => assigns.push((ei, "xyzw".to_string())),
                None => {
                    // A vertex shader feeding a geometry shader (entry "MainForGS") still
                    // carries a position export, but its D3D output signature has no
                    // SV_Position: the next stage consumes only the parameter exports.
                    self.line("// (position export with no SV_Position output element: ignored)");
                    return Ok(());
                }
            }
        } else if (32..64).contains(&target) {
            let param = self.out_params[(target - 32) as usize].clone();
            let Param::Full(reg) = param else {
                let raw = raw.filter(|_| !compr).ok_or("half-float parameter fed by a packed export")?;
                let mut lines = vec![];
                for (ei, e) in osig.iter().enumerate().filter(|(_, e)| e.sysval == 0 && param.registers().contains(&e.register)) {
                    let mut k = 0;
                    for c in (0..4usize).filter(|c| e.mask >> c & 1 != 0) {
                        let (chan, high) = param.channel(e.register, c).ok_or("unmapped half-float parameter")?;
                        let shift = if high == Some(true) { 16 } else { 0 };
                        let swizzle = if e.mask.count_ones() == 1 { String::new() } else { format!(".{}", XYZW[k]) };
                        lines.push(format!("if (exec_lo & 1u) o.o{ei}{swizzle} = f16tof32(({} >> {shift}) & 0xffffu);", raw[chan]));
                        k += 1;
                    }
                }
                for l in lines { self.line(&l); }
                return Ok(());
            };
            for (ei, e) in osig.iter().enumerate() {
                if e.sysval == 0 && e.register == reg {
                    let sw: String = (0..4).filter(|c| (e.mask >> c) & 1 != 0).map(|c| XYZW[c]).collect();
                    assigns.push((ei, sw));
                }
            }
        } else {
            return Err(format!("export target {target} unsupported"));
        }
        let mut lines = vec![];
        for (ei, sw) in assigns {
            let (ty, _) = sig_type(&osig[ei]);
            // Integer elements carry the register bits themselves; converting them through
            // float (or half) would destroy them.
            let val = match (osig[ei].comp_type, &raw) {
                (1, Some(rc)) => format!("uint4({})", rc.join(", ")),
                (2, Some(rc)) => format!("int4(uint4({}))", rc.join(", ")),
                (1 | 2, None) => return Err("integer output element fed by a packed export without a recognised pack epilogue".into()),
                (_, Some(rc)) if compr => format!("float4({})", rc.iter().map(|x| format!("asfloat({x})")).collect::<Vec<_>>().join(", ")),
                _ => v4.clone(),
            };
            lines.push(format!("if (exec_lo & 1u) o.o{ei} = ({ty})({val}.{sw});"));
        }
        for l in lines {
            self.line(&l);
        }
        Ok(())
    }

    /// Declarations of every register variable the body uses. Shaders with
    /// register-relative moves keep their VGPRs in one array (`tb_v`), with each `vN` name
    /// defined as an element of it, so `VGPR[n + M0]` is a plain dynamic index.
    fn reg_decls(&self) -> String {
        let is_v = |r: &str| r.len() > 1 && r.starts_with('v') && r[1..].bytes().all(|b| b.is_ascii_digit());
        let mut out = String::new();
        let scalars: Vec<String> =
            self.regs.iter().filter(|r| !(self.vreg_array && is_v(r))).map(|r| {
                if r == "tb_fp_mode" { format!("{r} = 0xc0u") } else { format!("{r} = 0u") }
            }).collect();
        if !scalars.is_empty() {
            out.push_str(&format!("    uint {};", scalars.join(", ")));
        }
        if self.vreg_array {
            out.push_str("\n    uint tb_v[256] = (uint[256])0;");
            for n in 0..256 {
                out.push_str(&format!("\n#define v{n} tb_v[{n}]"));
            }
        }
        out
    }

    /// Samplers indexed statically from the sampler descriptor heap. HLSL only
    /// allows SamplerDescriptorHeap inside a function, so these are locals of main.
    fn heap_sampler_locals(&self, h: &mut String) {
        for slot in &self.heap_sampler_slots {
            if self.samp_cmp.contains(slot) {
                let _ = writeln!(h, "    SamplerComparisonState scmp{slot} = SamplerDescriptorHeap[{slot}];");
            } else {
                let _ = writeln!(h, "    SamplerState samp{slot} = SamplerDescriptorHeap[{slot}];");
            }
        }
    }

    /// HLSL declarations of every resource the shader uses (constant buffers, textures,
    /// samplers, buffers, UAVs, the embedded code table and LDS).
    fn resource_decls(&self) -> Result<String, String> {
        let ex = self.ex;
        if let Some(slot) = self.table_samplers.intersection(&self.heap_sampler_slots).next() {
            return Err(format!("sampler s{slot} is used from the root signature and from the heap"));
        }
        let mut h = String::new();
        if let Some(ref packed) = self.packed { packed.declarations(&mut h); }
        let root_constants = sampler_heap::constants(ex).or_else(|| {
            // A packed plan already reads its arguments by name.
            if self.packed.is_some() { return None; }
            sampler_heap::rootless_constants(ex)
        });
        if root_constants.is_none() && self.packed.is_none()
            && (self.pixel_heap || (self.compute_heap && ex.stage == Stage::Vertex))
        {
            // A heap index taken from an unresolved root constant would silently be zero.
            if let Some(reg) = sampler_heap::rootless_constant_sgprs(ex).iter().find(|r| self.live.contains(r)) {
                return Err(format!("root constant {reg} of a root-less program is not resolved"));
            }
        }
        if let Some((_, key)) = root_constants {
            let (slot, space, count) = (key.register, key.space, key.count);
            if self.cb_size.contains_key(&slot) && self.binding_spaces.get(&(2, slot)).copied().unwrap_or(0) == space { return Err("root constants overlap a table CBV".into()); }
            let members = match count {
                1 => format!("uint tb_index{slot}_{space};"),
                2..=4 => format!("uint{count} tb_index{slot}_{space};"),
                // Scalars pack without padding: the buffer is exactly the constants.
                _ => (0..count).map(|k| format!("uint {};", sampler_heap::constant_name(&key, k))).collect::<Vec<_>>().join(" "),
            };
            writeln!(h, "cbuffer TBIndex{slot}_{space} : register(b{slot}, space{space}) {{ {members} }};").unwrap();
        }
        if self.resource_heap_base.is_some() && (self.heap_map || !self.compute_heap) {
            h.push_str("ByteAddressBuffer tb_heap_map : register(t0, space32766);\nstruct TBHeapConstants { uint4 data[4096]; };\nuint tb_heap_index(uint i) { return tb_heap_map.Load(0) == 0u ? i : tb_heap_map.Load(4u+i*4u); }\n");
        } else if self.compute_heap {
            // Without TuxBox's index map the guest heap index is the host heap index.
            h.push_str("struct TBHeapConstants { uint4 data[4096]; };\nuint tb_heap_index(uint i) { return i; }\n");
        }
        for (slot, dwords) in &self.cb_size {
            let space = self.space_suffix(2, *slot);
            let _ = writeln!(h, "cbuffer CB{slot} : register(b{}{space}) {{ uint4 cb{slot}[{}]; }};", self.register_of(2, *slot), dwords.div_ceil(4).max(1));
        }
        for (slot, dim) in &self.tex {
            let ty = match dim {
                0 => "Texture1D",
                4 => "Texture1DArray",
                1 => "Texture2D",
                2 => "Texture3D",
                3 => "TextureCube",
                5 => "Texture2DArray",
                6 => "Texture2DMS",
                _ => return Err(format!("texture dim {dim} unsupported")),
            };
            let e = match self.img_ty.get(&(false, *slot)) {
                Some(2) => "int4",
                Some(_) => "uint4",
                None => "float4",
            };
            let space = self.space_suffix(0, *slot);
            let _ = writeln!(h, "{ty}<{e}> tex{slot} : register(t{}{space});", self.register_of(0, *slot));
        }
        for slot in self.samp.iter().filter(|s| !self.heap_sampler_slots.contains(s)) {
            let space = self.space_suffix(3, *slot);
            let _ = writeln!(h, "SamplerState samp{slot} : register(s{}{space});", self.register_of(3, *slot));
        }
        for slot in &self.samp_cmp {
            let space = self.space_suffix(3, *slot);
            if self.samp.contains(slot) {
                return Err(format!("sampler s{slot} used both as a regular and a comparison sampler"));
            }
            if self.heap_sampler_slots.contains(slot) {
                continue;
            }
            let _ = writeln!(h, "SamplerComparisonState scmp{slot} : register(s{}{space});", self.register_of(3, *slot));
        }
        let device_fence = self.persistent_queue || self.packed.as_ref().is_some_and(|p| !p.device_sync_before.is_empty() || !p.device_sync_barriers.is_empty())
            || (self.is_cs && self.insns.iter().any(|i| i.enc == Enc::Mubuf && matches!(i.op, 0x71 | 0x72)));
        let coherent = if device_fence { "globallycoherent " } else { "" };
        for (is_uav, slot) in &self.raw {
            let space = self.space_suffix(u32::from(*is_uav), *slot);
            if *is_uav {
                let _ = writeln!(h, "{coherent}RWByteAddressBuffer raw_ubuf{slot} : register(u{}{space});", self.register_of(1, *slot));
            } else {
                let _ = writeln!(h, "ByteAddressBuffer raw_sbuf{slot} : register(t{}{space});", self.register_of(0, *slot));
            }
        }
        for (is_uav, slot) in self.sbuf.keys() {
            let space = self.space_suffix(u32::from(*is_uav), *slot);
            let (ty, nm, reg) = if *is_uav { ("RWStructuredBuffer", "ubuf", "u") } else { ("StructuredBuffer", "sbuf", "t") };
            let qualifier = if *is_uav { coherent } else { "" };
            let _ = writeln!(h, "struct TbElem_{nm}{slot} {{ uint w[{}]; }};
{qualifier}{ty}<TbElem_{nm}{slot}> {nm}{slot} : register({reg}{}{space});", self.sbuf_words((*is_uav, *slot)), self.register_of(u32::from(*is_uav), *slot));
            if self.sbuf_flat.contains(&(*is_uav, *slot)) {
                // Word at a byte offset: the element stride is a property of the view, not of
                // the shader, so it is queried.
                let _ = writeln!(h, "uint tb_sbw_{nm}{slot}(uint off) {{ uint n, st; {nm}{slot}.GetDimensions(n, st); st = max(st, 4u); uint e = off / st; return {nm}{slot}[e].w[(off - e * st) >> 2]; }}");
            }
        }
        for (is_uav, slot) in &self.tbuf {
            let space = self.space_suffix(u32::from(*is_uav), *slot);
            let _ = if *is_uav && self.tbuf_uint.contains(slot) {
                writeln!(h, "{coherent}RWBuffer<uint> utb{slot} : register(u{}{space});", self.register_of(1, *slot))
            } else {
                let e = match self.tbuf_ty.get(&(*is_uav, *slot)) {
                    Some(2) => "int4",
                    Some(_) => "uint4",
                    None => "float4",
                };
                if *is_uav {
                    writeln!(h, "{coherent}RWBuffer<{e}> utb{slot} : register(u{}{space});", self.register_of(1, *slot))
                } else {
                    writeln!(h, "Buffer<{e}> stb{slot} : register(t{}{space});", self.register_of(0, *slot))
                }
            };
        }
        for (slot, dim) in &self.uav {
            // Textures used with image atomics hold one uint per texel.
            let e = match self.img_ty.get(&(true, *slot)) {
                Some(2) => "int4",
                Some(_) => "uint4",
                None => "float4",
            };
            let ty = match (dim, self.uav_uint.contains(slot)) {
                (0, false) => format!("RWTexture1D<{e}>"),
                (1, false) => format!("RWTexture2D<{e}>"),
                (2, false) => format!("RWTexture3D<{e}>"),
                (4, false) => format!("RWTexture1DArray<{e}>"),
                (5, false) => format!("RWTexture2DArray<{e}>"),
                (0, true) => "RWTexture1D<uint>".to_string(),
                (1, true) if self.wide_visibility_uavs.contains(slot) => "RWTexture2D<uint64_t>".to_string(),
                (1, true) => "RWTexture2D<uint>".to_string(),
                (2, true) => "RWTexture3D<uint>".to_string(),
                (4, true) => "RWTexture1DArray<uint>".to_string(),
                (5, true) => "RWTexture2DArray<uint>".to_string(),
                _ => return Err(format!("UAV dim {dim} unsupported")),
            };
            let space = self.space_suffix(1, *slot);
            let _ = writeln!(h, "{coherent}{ty} uav{slot} : register(u{}{space});", self.register_of(1, *slot));
        }
        if self.uses_code {
            // Constant tables embedded after the program in the code section. Code pointers
            // are byte offsets from the program start, so readers subtract the program
            // length (in dwords) before indexing. Only the tail is emitted: FXC allows at
            // most 4096 indexable literals.
            let words: Vec<String> = ex
                .isa_data
                .get(ex.isa.len()..)
                .unwrap_or(&[])
                .chunks_exact(4)
                .map(|c| format!("{:#x}", u32::from_le_bytes([c[0], c[1], c[2], c[3]])))
                .collect();
            let _ = writeln!(h, "static const uint tb_code[{}] = {{ {} }};", words.len().max(1), if words.is_empty() { "0".to_string() } else { words.join(", ") });
        }
        if self.micropoly_vote.is_some() {
            writeln!(h, "groupshared uint tb_lds[784];\ngroupshared uint tb_raster_votes[2];").unwrap();
        } else if self.is_cs && self.uses_lds {
            let words = self.reflected_lds_words.or_else(|| wave_cull::lds_words(self)).unwrap_or(8192);
            let _ = writeln!(h, "groupshared uint tb_lds[{words}];");
        }
        Ok(h)
    }

    fn finish(&mut self, is_ps: bool, output_order: &[String], stencil_target: Option<u32>) -> Result<String, String> {
        let ex = self.ex;
        let mut h = String::new();
        let _ = writeln!(h, "// Generated by tuxbox-xshader from RDNA2 microcode ({:?}, {} instrs).", ex.stage, self.insns.len());
        self.push_helpers(&mut h);
        h.push_str(&self.resource_decls()?);
        if self.is_cs {
            // Local thread ID is in v0..v2. Packed argument blocks use s4 for group X;
            // the descriptor-table ABI places group IDs after the root parameters.
            let [tx, ty_, tz] = compute_group(ex);
            let nparams = ex.root_sig.as_ref().map(|r| r.params.len()).unwrap_or(4);
            let g = match self.packed.as_ref().map(|p| &p.abi) {
                None if self.resolve_wave32 || lone_lane::on() && ray_resolve::applies(ex) => ray_resolve::group_x_register(ex)
                    .ok_or("unverified resolve compute user-data layout")?,
                Some(RootAbi::RegisterWave32 | RootAbi::RegisterWave64) => 6 + 2 * (nparams - 2), // two tables followed by root CBV pairs
                Some(RootAbi::ArgumentBlock) => 4,
                Some(RootAbi::Ps09Raster) => 11,
                None if ex.root_sig.as_ref().is_some_and(|r| r.flags & 0x400 != 0 && r.params.len() == 2 && r.params.iter().all(|p| matches!(p.kind, crate::meta::RootParamKind::Table(_))))
                    && tuxbox_dxbc::compute_table::two_tables_and_heap(&ex.fssm) => 7,
                None if sampler_heap::user_data_end(ex).is_some() =>
                    sampler_heap::user_data_end(ex).unwrap(),
                None => sampler_heap::constants(ex).map_or(4 + nparams, |(start, c)| usize::from(start) + c.count as usize),
            };
            if self.compute_swizzle || self.indexed_lane {
                writeln!(h, "groupshared uint tb_swizzle[{}];", tx * ty_ * tz).unwrap();
            }
            if let Some(width) = self.required_wave_size.filter(|_| !self.portable_wave64) {
                writeln!(h, "[WaveSize({width})]").unwrap();
            }
            // These kernels enable all 64 lanes for a reduction. Start the extra
            // rows inactive so they never participate in the guest's memory work.
            let padded = self.wave_cull && matches!([tx, ty_, tz], [4, 4, 1] | [32, 1, 1]);
            let host_y = if padded { 64 / tx } else { ty_ };
            let _ = writeln!(h, "[numthreads({tx}, {host_y}, {tz})]
void main(uint3 gid : SV_GroupID, uint3 gtid : SV_GroupThreadID) {{");
            let active = if self.resolve_wave32 { "0xffffffffu".to_owned() }
                else if padded { format!("uint(gtid.y < {ty_}u)") } else { "1u".to_owned() };
            let _ = writeln!(h, "    uint exec_lo = {active}, exec_hi = 0u, vcc_lo = 0u, vcc_hi = 0u, scc = 0u, m0 = 0u, nul = 0u;");
            self.heap_sampler_locals(&mut h);
            for r in ["v0", "v1", "v2"] {
                self.regs.insert(r.to_string());
            }
            for k in 0..3 {
                self.regs.insert(format!("s{}", g + k));
            }
            let _ = writeln!(h, "{}", self.reg_decls());
            if self.page_waves || self.wave_cull {
                writeln!(h, "    uint tb_wave_tid = gtid.x + {tx}u * (gtid.y + {ty_}u * gtid.z);").unwrap();
            }
            let masked_dpp = self.insns.iter().any(|i| i.op == OP_DPP_BINARY && group_extrema::destination_mask(i).is_some());
            if self.compute_swizzle || self.indexed_lane || self.single_wave32 || masked_dpp {
                writeln!(h, "    uint tb_lane = gtid.x + {tx}u * (gtid.y + {ty_}u * gtid.z);").unwrap();
            }
            if self.resolve_wave32 {
                writeln!(h, "    v0 = WaveGetLaneIndex(); v1 = gtid.y; v2 = gtid.z; uint tb_lane_bit = 1u << v0;").unwrap();
            } else {
                writeln!(h, "    v0 = gtid.x; v1 = gtid.y; v2 = gtid.z;").unwrap();
            }
            let _ = writeln!(h, "    s{} = gid.x; s{} = gid.y; s{} = gid.z;", g, g + 1, g + 2);
            sampler_heap::init_constants(ex, self.packed.is_some(), &self.regs, &mut h);
            if let Some(plan) = &self.packed { plan.init_args(&mut h); }
            h.push_str(&self.heap_handle_decls());
            h.push_str(&self.body);
            h.push_str("}
");
            return Ok(h);
        }
        // Outputs matching per-vertex pixel-shader inputs must use the same modifier, so the
        // producer needs the macro too (both stages are compiled for the same shader model).
        let out_uses_macro = !is_ps && self.out_interp.0.iter().any(|(_, m)| m == "TB_VERTEX_INPUT");
        if !self.pulled_attrs.is_empty() || out_uses_macro {
            h.push_str("#if __SHADER_TARGET_MAJOR >= 6\n#define TB_VERTEX_INPUT nointerpolation\n#else\n#define TB_VERTEX_INPUT\n#endif\n");
        }
        let _ = writeln!(h, "struct SIn {{");
        // HLSL requires system-generated inputs (SV_IsFrontFace, SV_SampleIndex,
        // SV_PrimitiveID, ...) to follow all ordinary parameters, so emit them last.
        // Field names stay tied to the signature element index.
        let sysgen = |e: &SigElement| is_ps && !matches!(e.sysval, 0 | 1);
        for pass in 0..2 {
            for (ei, e) in ex.input_sig.iter().enumerate() {
                if sysgen(e) != (pass == 1) {
                    continue;
                }
                let (ty, _) = sig_type(e);
                // DXIL validation: with a conservative depth output (SV_DepthLessEqual /
                // SV_DepthGreaterEqual) the position input must be centroid-interpolated.
                let cons_depth = ex.output_sig.iter().any(|o| {
                    let s = semantic(o);
                    s.starts_with("SV_DepthLessEqual") || s.starts_with("SV_DepthGreaterEqual")
                });
                // A parameter read exclusively with V_INTERP_MOV P0 comes from
                // the provoking vertex. Keep other attributes smoothly interpolated.
                let attr = self.ps_attrs.iter().position(|p| e.sysval == 0 && p.registers().contains(&e.register));
                let flat_half = e.comp_type == 6 && ex.flat_inputs.contains(&e.register);
                let flat_p0 = is_ps && attr.is_some_and(|a| self.flat_attrs.contains(&a));
                let pulled = is_ps && attr.is_some_and(|a| self.pulled_attrs.contains(&a));
                let interp = if !is_ps {
                    ""
                } else if e.sysval == 1 && cons_depth {
                    "linear noperspective centroid "
                } else if pulled {
                    "TB_VERTEX_INPUT "
                } else if !matches!(e.comp_type, 3 | 6) || flat_p0 || flat_half {
                    "nointerpolation "
                } else if e.name.contains("_centroid") {
                    "centroid "
                } else {
                    ""
                };
                // SV_Position is not mirrored: the producer declares it `precise` with its own
                // rules, and it had a register of its own in every DXC listing checked.
                if is_ps && !interp.is_empty() && e.sysval != 1 {
                    self.ps_in_interp.0.push((semantic(e), interp.trim_end().to_string()));
                }
                let _ = writeln!(h, "    {interp}{ty} i{ei} : {};", semantic(e));
            }
        }
        if !self.pulled_attrs.is_empty() {
            h.push_str("#if __SHADER_TARGET_MAJOR >= 6\n    float3 tb_bary : SV_Barycentrics;\n#endif\n");
        }
        let pos_field = match ex.input_sig.iter().position(|e| e.sysval == 1) {
            Some(ei) => format!("i.i{ei}"),
            None => "i.tb_pos".to_string(),
        };
        if is_ps && self.uses_quad && pos_field == "i.tb_pos" {
            let _ = writeln!(h, "    float4 tb_pos : SV_Position;");
        }
        let _ = writeln!(h, "}};\nstruct SOut {{");
        let mut outputs: Vec<_> = ex.output_sig.iter().enumerate().collect();
        if !output_order.is_empty() && matches!(ex.stage, Stage::Vertex | Stage::Geometry) {
            outputs.sort_by_cached_key(|(_, e)| {
                let name = semantic(e);
                output_order.iter().position(|s| s.eq_ignore_ascii_case(&name)).unwrap_or(usize::MAX)
            });
        }
        for (ei, e) in outputs {
            let (ty, _) = sig_type(e);
            if let Some(slot) = stencil_target.filter(|_| e.sysval == 69) {
                let _ = writeln!(h, "    uint o{ei} : SV_Target{slot};");
            } else if matches!(e.sysval, 65 | 67 | 68) {
                let _ = writeln!(h, "    float od : {};", semantic(e).trim_end_matches('0'));
            } else if e.sysval == 1 && !is_ps {
                // `precise`: no fast-math on the position computation. The title relies on a
                // depth-only pass and the base pass producing identical depth (the base pass
                // tests without writing); the two are separate shaders here, and without this
                // the compiler may contract or reorder them differently (observed as holes
                // in the character where the base pass failed the depth test).
                let _ = writeln!(h, "    precise {ty} o{ei} : {};", semantic(e));
            } else {
                let interp = if is_ps { None } else { self.out_interp.get(&semantic(e)) };
                let interp = interp.map(|m| format!("{m} ")).unwrap_or_default();
                let _ = writeln!(h, "    {interp}{ty} o{ei} : {};", semantic(e));
            }
        }
        let no_out = ex.output_sig.is_empty();
        if no_out {
            // No outputs (e.g. depth-only / kill-only pixel shader): HLSL forbids empty structs.
            let cut = h.rfind("struct SOut {").unwrap_or(h.len());
            h.truncate(cut);
            let _ = writeln!(h, "void main(SIn i) {{");
        } else if self.is_gs {
            let (nv, _, _, prim, stream) = self.gs_dims()?;
            let _ = writeln!(
                h,
                "}};\n[maxvertexcount({})]\nvoid main({prim} SIn i[{nv}], inout {stream}<SOut> stream) {{",
                self.gs_max_vertices()
            );
        } else {
            let _ = writeln!(h, "}};");
            if is_ps && ex.output_sig.iter().all(|out| out.sysval == 64)
                && crate::meta::pixel_depth_before_shader(&ex.fssm)
            {
                h.push_str("[earlydepthstencil]\n");
            }
            let _ = writeln!(h, "SOut main(SIn i) {{\n    SOut o = (SOut)0;");
        }
        let _ = writeln!(h, "    uint exec_lo = 1u, exec_hi = 0u, vcc_lo = 0u, vcc_hi = 0u, scc = 0u, m0 = 0u, nul = 0u;");
        self.heap_sampler_locals(&mut h);
        // Vertex attributes arrive after the fetch call in consecutive groups of four VGPRs
        // from v4, one group per input register the shader reads. A register none of whose
        // elements is read (read mask 0 in the input signature) gets no group: in the skinned
        // depth-only shader d15c18830a4616f9 ATTRIBUTE1/2 (tangents) are unread and the bone
        // indices of ATTRIBUTE3 arrive in v8..v11, not v16..v19.
        let mut init = String::new();
        if self.is_gs {
            // GS entry ABI (from WriteToSliceMainGS): s3 carries the wave's primitive count in
            // bits [15:8]; v0/v1 hold the vertex offsets (v0 = offset0 | offset1 << 16, v1 =
            // offset2), in dwords. One invocation = one primitive.
            let (nv, in_dw, out_dw, _, _) = self.gs_dims()?;
            let _ = writeln!(init, "    uint tb_in[{nv}][{in_dw}]; uint tb_gs[{}]; uint tb_emit = 0u;", out_dw * self.gs_max_vertices());
            let _ = writeln!(init, "    {{ for (uint a = 0u; a < {}u; a++) tb_gs[a] = 0u; }}", out_dw * self.gs_max_vertices());
            let _ = writeln!(init, "    {{ for (uint k = 0u; k < {nv}u; k++) for (uint a = 0u; a < {in_dw}u; a++) tb_in[k][a] = 0u; }}");
            for k in 0..nv {
                for (ei, e) in ex.input_sig.iter().enumerate() {
                    let mut n = 0;
                    for c in 0..4usize {
                        if (e.mask >> c) & 1 != 0 {
                            let acc = format!("i[{k}].i{ei}.{}", XYZW[n]);
                            let val = if e.comp_type == 3 { format!("asuint({acc})") } else { format!("(uint)({acc})") };
                            let _ = writeln!(init, "    tb_in[{k}][{}] = {val};", e.register as usize * 4 + c);
                            n += 1;
                        }
                    }
                }
            }
            for rname in ["s3", "v0", "v1"] {
                self.regs.insert(rname.to_string());
            }
            let _ = writeln!(init, "    s3 = 0x100u; v0 = 0x04000000u; v1 = 0x800u;");
        }
        // In the merged vertex-stage prologue, s3[7:0] is the vertex count.
        // The per-invocation HLSL register model handles one active vertex.
        let merged_count = ex.stage == Stage::Vertex && self.insns.windows(2).take(8).any(|pair| {
            let (count, mask) = (&pair[0], &pair[1]);
            matches!(count.mnemonic.as_str(), "s_bfe_i32" | "s_bfe_u32")
                && count.src == [Opnd::Sgpr(3), Opnd::Literal(0x80000)]
                && mask.mnemonic == "s_bfm_b64" && mask.dst == [Opnd::ExecLo]
                && matches!(mask.src.as_slice(), [width, Opnd::ConstInt(0)] if count.dst == [*width])
        });
        if merged_count {
            init.push_str("    s3 = 1u;\n");
        }
        // ES-GS puts five GS parameters before the ordinary four VS input registers.
        // A standalone VS receives vertex and instance IDs even when it pulls its
        // own buffers and never calls a hardware fetch shader.
        let merged_fetch = merged_count && self.insns.iter().take(16).any(|ins| {
            ins.mnemonic == "s_swappc_b64" && ins.dst == [Opnd::Sgpr(12)] && ins.src == [Opnd::Sgpr(12)]
        });
        let standalone_vs = ex.stage == Stage::Vertex && !merged_count;
        if merged_fetch || standalone_vs {
            for (i, elem) in ex.input_sig.iter().enumerate() {
                let reg = match (elem.sysval, merged_fetch) {
                    (6, false) => "v0", // SV_VertexID
                    (8, false) => "v3", // SV_InstanceID
                    (6, true) => "v5",
                    (8, true) => "v8",
                    _ => continue,
                };
                if standalone_vs && !self.live.iter().any(|r| r == reg) {
                    continue;
                }
                self.regs.insert(reg.to_string());
                let _ = writeln!(init, "    {reg} = (uint)i.i{i};");
            }
        }
        if !is_ps && !self.is_gs {
            let mut group = 0usize;
            for reg in distinct_regs(ex.input_sig.iter().filter(|e| e.sysval == 0)) {
                let read = ex.input_sig.iter().any(|e| e.sysval == 0 && e.register == reg && e.rw_mask != 0);
                if ex.stage == Stage::Vertex && !read {
                    continue;
                }
                let first = if merged_fetch { 9 + 4 * group } else if ex.stage == Stage::Vertex { 4 + 4 * group } else { 4 + 4 * reg as usize };
                group += 1;
                for c in 0..4usize {
                    let v = format!("v{}", first + c);
                    self.regs.insert(v.clone());
                    let val = match sig_lookup(&ex.input_sig, reg, c) {
                        Some((ei, k)) => {
                            let acc = format!("i.i{ei}.{}", XYZW[k]);
                            if ex.input_sig[ei].comp_type == 3 { format!("asuint({acc})") } else { format!("(uint)({acc})") }
                        }
                        // Unsupplied components default to (0,0,0,1) like the hardware fetch.
                        None => (if c == 3 { "0x3f800000u" } else { "0u" }).to_string(),
                    };
                    let _ = writeln!(init, "    {v} = {val};");
                }
            }
        }
        if !self.regs.is_empty() {
            let _ = writeln!(h, "{}", self.reg_decls());
        }
        if is_ps && self.uses_quad {
            let _ = writeln!(init, "    uint tb_quad = ((uint){pos_field}.x & 1u) | (((uint){pos_field}.y & 1u) << 1);");
        }
        if ex.stage == Stage::Vertex {
            sampler_heap::init_constants(ex, self.packed.is_some(), &self.regs, &mut init);
        }
        if is_ps {
            if !self.pulled_attrs.is_empty() {
                init.push_str("#if __SHADER_TARGET_MAJOR >= 6\n");
                for (reg, weight) in [("v0","y"),("v1","z")] {
                    if self.regs.contains(reg) { let _ = writeln!(init, "    {reg} = asuint(i.tb_bary.{weight});"); }
                }
                init.push_str("#endif\n");
            }
            // The native rasterizer packs enabled inputs into consecutive VGPRs.
            sampler_heap::init_constants(ex, self.packed.is_some(), &self.regs, &mut init);
            if let Some(plan) = &self.packed { plan.init_args(&mut init); }
            if let Some(mask) = ex.ps09_pixel_inputs {
                // Bit 7 (line stipple) occupies v0 in both observed layouts.
                // Neither shader reads it. The next enabled input occupies v1.
                if self.live.iter().any(|r| r == "v0") {
                    return Err("PS09 line-stipple input unsupported".into());
                }
                match mask {
                    0x8080 => {
                        let ei = ex.input_sig.iter().position(|e| e.sysval == 1).ok_or("missing PS09 raster position")?;
                        if self.regs.contains("v1") {
                            writeln!(init, "    v1 = (uint(i.i{ei}.x) & 65535u) | (uint(i.i{ei}.y) << 16);").unwrap();
                        }
                    }
                    0x1080 => {
                        let ei = ex.input_sig.iter().position(|e| e.sysval == 9).ok_or("missing PS09 front-face input")?;
                        if self.regs.contains("v1") {
                            writeln!(init, "    v1 = asuint(i.i{ei} != 0u ? 1.0 : -1.0);").unwrap();
                        }
                    }
                    _ => return Err(format!("unsupported PS09 pixel input mask {mask:#x}")),
                }
            } else {
                use crate::meta::{pixel_system_vgpr, PixelSystemInput};
                let layer = ex.input_sig.iter().position(|e| e.sysval == 4 && e.rw_mask != 0);
                let ancillary = layer.and_then(|field| {
                    pixel_system_vgpr(&ex.fssm, PixelSystemInput::Ancillary).map(|slot| (format!("v{slot}"), field))
                });
                if let Some(ei) = ex.input_sig.iter().position(|e| e.sysval == 1) {
                    let p = format!("i.i{ei}");
                    let positions = crate::meta::position_vgprs(&ex.fssm, ex.input_sig[ei].rw_mask);
                    // POS_W_FLOAT is the interpolated clip W, matching D3D SV_Position.w.
                    // Native shaders issue a reciprocal themselves when they need 1/W.
                    let w = if positions.is_some() { format!("{p}.w") } else { format!("1.0 / {p}.w") };
                    let exprs = [format!("{p}.x"), format!("{p}.y"), format!("{p}.z"), w];
                    let positions = positions.unwrap_or([Some(2), Some(3), Some(4), Some(5)]);
                    for (slot, expr) in positions.into_iter().zip(&exprs) {
                        let Some(slot) = slot else { continue; };
                        let v = format!("v{slot}");
                        if self.live.contains(&v) && !ancillary.as_ref().is_some_and(|(reg, _)| *reg == v) {
                            let _ = writeln!(init, "    {v} = asuint({expr});");
                        }
                    }
                }
                if let (Some(slot), Some(field)) = (
                    pixel_system_vgpr(&ex.fssm, PixelSystemInput::FrontFace),
                    ex.input_sig.iter().position(|e| e.sysval == 9 && e.rw_mask != 0),
                ) {
                    let reg = format!("v{slot}");
                    if self.live.contains(&reg) {
                        // Conventional DX pixel programs consume the integer facing
                        // predicate. The FSSM register is zero on a back-facing primitive.
                        let _ = writeln!(init, "    {reg} = i.i{field} != 0u ? 0xffffffffu : 0u;");
                    }
                }
                if let Some((reg, field)) = ancillary.filter(|(reg, _)| self.live.contains(reg)) {
                    // RTA occupies bits 28:16 and the sample number bits 11:8.
                    let mut packed = format!("((uint)i.i{field} & 8191u) << 16");
                    if let Some(sample) = ex.input_sig.iter().position(|e| e.sysval == 10 && e.rw_mask != 0) {
                        let _ = write!(packed, " | (((uint)i.i{sample} & 15u) << 8)");
                    }
                    let _ = writeln!(init, "    {reg} = {packed};");
                }
            }
        }
        h.push_str(&init);
        h.push_str(&self.heap_handle_decls());
        h.push_str(&self.body);
        // Native stencil reference export stores the low byte. An R8_UINT
        // render target instead saturates a 32-bit shader result at 255.
        if let Some(ei) = stencil_target.and_then(|_| ex.output_sig.iter().position(|e| e.sysval == 69)) {
            let _ = writeln!(h, "    o.o{ei} &= 255u;");
        }
        h.push_str(if no_out || self.is_gs { "}\n" } else { "    return o;\n}\n" });
        Ok(h)
    }
}

/// User SGPR of a single FSSM user-data record of `class` (5: resource heap base,
/// 6: sampler heap base).
fn compute_user_sgpr(ex: &ShaderExtract, class: u32) -> Option<u8> {
    let mut rows = tuxbox_dxbc::compute_table::user_data_records(&ex.fssm)?.filter(|&[c, ..]| c == class);
    let [_, _, _, _, loc] = rows.next()?;
    if rows.next().is_some() {
        return None;
    }
    u8::try_from(loc & 0xff).ok()
}

/// The sampler heap base of a mesh program, from its one FSSM class-6 row.
fn mesh_sampler_heap_base(ex: &ShaderExtract) -> Option<u8> {
    if ex.stage != Stage::Mesh || ex.root_sig.as_ref()?.flags & 0x800 == 0 {
        return None;
    }
    compute_user_sgpr(ex, 6)?.checked_add(8)
}

/// Whether an instruction uses SCC.
fn reads_scc(ins: &Insn) -> bool {
    ins.src.contains(&Opnd::Scc)
        || matches!(ins.mnemonic.as_str(), "s_cbranch_scc0" | "s_cbranch_scc1"
            | "s_cselect_b32" | "s_cselect_b64" | "s_addc_u32" | "s_subb_u32"
            | "s_cmov_b32" | "s_cmov_b64" | "s_cmovk_i32")
}

/// Whether the SCC that the instruction at `at` uses may come from a lane mask. A scalar
/// comparison or sum gives every lane of the wave the same SCC.
fn scc_from_mask(code: &[Insn], at: usize) -> bool {
    for (i, op) in code[..at].iter().enumerate().rev() {
        let m = op.mnemonic.as_str();
        if m.starts_with("s_cmp_") || m.starts_with("s_cmpk_") || m.starts_with("s_bitcmp") {
            return op.src.iter().chain(&op.dst).any(|o| matches!(o, Opnd::ExecLo | Opnd::ExecHi | Opnd::VccLo | Opnd::VccHi))
                || m.ends_with("_u64");
        }
        let scalar_alu = matches!(op.enc, Enc::Sop1 | Enc::Sop2 | Enc::Sopk);
        if scalar_alu && !matches!(m, "s_mov_b32" | "s_mov_b64" | "s_movk_i32" | "s_cselect_b32" | "s_cselect_b64" | "s_mul_i32" | "s_mul_hi_u32" | "s_mul_hi_i32" | "s_brev_b32" | "s_bitset1_b32" | "s_bitset0_b32" | "s_pack_ll_b32_b16" | "s_bfm_b32" | "s_cmov_b32" | "s_cmov_b64") {
            return true;
        }
        // Another path may arrive here with its own SCC.
        let here = code[i + 1].offset as i64;
        if code.iter().any(|b| b.enc == Enc::Sopp && Emitter::branch_cond(b.op).is_some() && b.offset as i64 + 4 + b.aux.imm * 4 == here) {
            return true;
        }
        if op.enc == Enc::Sopp && Emitter::branch_cond(op.op).is_some() {
            return true;
        }
    }
    true
}

/// Mesh shaders index the resource descriptor heap through one FSSM class-5
/// dword. User data sits above the eight system SGPRs, so the hardware register
/// is `(location & 0xff) + 8`. The row is the shape captured on every mesh
/// launch that failed only on this register: class 5, offset 0, width 1.
fn mesh_heap_base(ex: &ShaderExtract) -> Option<u8> {
    if ex.stage != Stage::Mesh || ex.required_features? & (1 << 25) == 0 {
        return None;
    }
    let root = ex.root_sig.as_ref()?;
    // Root flag 0x400 is RESOURCE_HEAP_DIRECTLY_INDEXED.
    if root.flags & 0x400 == 0 {
        return None;
    }
    let mut rows = tuxbox_dxbc::compute_table::user_data_records(&ex.fssm)?
        .filter(|&[class, ..]| class == 5);
    let Some([5, 0, 1, 0, loc]) = rows.next() else {
        return None;
    };
    if rows.next().is_some() {
        return None;
    }
    u8::try_from(loc & 0xff).ok()?.checked_add(8)
}

/// Whether dwords `1..n` of the heap descriptor at `base` are still the loaded ones.
fn heap_words(sym: &HashMap<u16, Sym>, base: u16, n: u16) -> bool {
    let origin = match sym.get(&base) {
        Some(Sym::HeapDescriptor(_, origin) | Sym::HeapSamplerIndex(origin)) => *origin,
        _ => return false,
    };
    (1..n).all(|k| sym.get(&(base + k)) == Some(&Sym::HeapWord(k as u8, origin)))
}

/// A complete `n`-dword resource descriptor fetched at a run-time heap index.
fn heap_descriptor(sym: &HashMap<u16, Sym>, base: u16, n: u16) -> bool {
    matches!(sym.get(&base), Some(Sym::HeapDescriptor(words, _)) if *words == u32::from(n))
        && heap_words(sym, base, n)
}

/// Pixel shaders receive descriptor heap bases like compute: the user SGPR
/// named by one FSSM user-data row (class 5 the resource heap, 6 the sampler heap),
/// when SFI0 declares that heap directly indexed (bits 25 and 26). Rooted pixel
/// shaders also allow class 5 when `RESOURCE_HEAP_DIRECTLY_INDEXED` (0x400) is set
/// and `sampler_heap::resource_base` does not already claim the root layout.
fn pixel_heap_base(ex: &ShaderExtract, class: u32) -> Option<u8> {
    if ex.stage != Stage::Pixel || ex.required_features? & (1 << (20 + class)) == 0 {
        return None;
    }
    if let Some(root) = ex.root_sig.as_ref() {
        // Class 5 needs CBV_SRV_UAV_HEAP_DIRECTLY_INDEXED (0x400), class 6
        // SAMPLER_HEAP_DIRECTLY_INDEXED (0x800).
        let indexed = match class { 5 => root.flags & 0x400 != 0, 6 => root.flags & 0x800 != 0, _ => false };
        if !indexed || sampler_heap::resource_base(ex).is_some() {
            return None;
        }
    }
    let mut rows = tuxbox_dxbc::compute_table::user_data_records(&ex.fssm)?.filter(|&[c, ..]| c == class);
    let [_, 0, 1, 0, loc @ 0..=0xff] = rows.next()? else { return None };
    if rows.next().is_some() {
        return None;
    }
    u8::try_from(loc).ok()
}

/// Compute and vertex shaders that index the resource descriptor heap receive
/// its base in the user SGPR named by FSSM user-data class 5 (class 6 is the
/// sampler heap). RAGE's skinning and VfxVBuffer vertex shaders fetch
/// `ResourceDescriptorHeap[root constant]` from s18 this way, behind three
/// descriptor tables, a root SRV and five root constants: the same form as
/// compute, rather than the pixel layout `sampler_heap::resource_base` checks.
fn compute_heap_base(ex: &ShaderExtract) -> Option<u8> {
    if !matches!(ex.stage, Stage::Compute | Stage::Vertex) || ex.required_features? & (1 << 25) == 0 {
        return None;
    }
    if ex.root_sig.as_ref().is_some_and(|root| root.flags & 0x400 == 0) {
        return None;
    }
    let mut rows = tuxbox_dxbc::compute_table::user_data_records(&ex.fssm)?
        .filter(|&[class, ..]| class == 5);
    let [_, _, _, _, loc] = rows.next()?;
    if rows.next().is_some() {
        return None;
    }
    u8::try_from(loc & 0xff).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vector_add_sub_can_discard_the_carry_destination() {
        use super::float64::tests::translate;

        // gfx1030 VOP3B add, sub and subrev with SDST=null; the _ci_ forms read VCC.
        for (opcode, sources) in [
            (0xd70f_7d04, 0x0002_0501), (0xd710_7d04, 0x0002_0501),
            (0xd719_7d04, 0x0002_0501), (0xd528_7d04, 0x01aa_0501),
            (0xd529_7d04, 0x01aa_0501), (0xd52a_7d04, 0x01aa_0501),
        ] {
            let hlsl = translate(Stage::Compute, &[opcode, sources]).unwrap();
            assert!(hlsl.contains("v4 = t"), "{hlsl}");
            assert!(!hlsl.lines().any(|line| {
                let stmt = line.trim_start();
                stmt.starts_with("vcc_lo =") || stmt.starts_with("vcc_hi =") || stmt.starts_with("nul =")
            }), "{hlsl}");
            assert!(translate(Stage::Compute, &[opcode, sources | (1 << 29)]).is_err());
        }
    }

    #[test]
    fn vector_carry_clamp_saturates_the_result_and_keeps_the_overflow_flag() {
        use super::float64::tests::translate;

        for (opcode, limit) in [(0xd70f_ea04, "0xffffffffu"), (0xd710_ea04, "0u")] {
            // gfx1030: v_{add,sub}_co_u32 v4, vcc_lo, v1, v2 clamp
            let hlsl = translate(Stage::Compute, &[opcode, 0x0002_0501]).unwrap();
            assert!(hlsl.contains(&format!("if (t0o != 0u) t0 = {limit};")), "{hlsl}");
            assert!(hlsl.lines().any(|line| line.trim_start().starts_with("vcc_lo =") && line.contains("t0o")));
        }

        // In e32 the same bit belongs to the source register v66.
        let hlsl = translate(Stage::Compute, &[0x5008_8501]).unwrap();
        assert!(!hlsl.contains("if (t0o != 0u)"), "{hlsl}");
    }

    #[test]
    fn unsigned_no_carry_clamp_is_specific_to_the_long_encoding() {
        use super::float64::tests::translate;
        // LLVM gfx1030: v_{add,sub,subrev}_nc_u32_e64 v4, v1, v2 clamp.
        for (opcode, saturated, wrapping) in [
            (0xd525_8004, "(min(v1, ~v2) + v2)", "(v1 + v2)"),
            (0xd526_8004, "(max(v1, v2) - v2)", "(v1 - v2)"),
            (0xd527_8004, "(max(v2, v1) - v1)", "(v2 - v1)"),
        ] {
            let hlsl = translate(Stage::Compute, &[opcode, 0x0002_0501]).unwrap();
            assert!(hlsl.contains(saturated), "{hlsl}");
            let hlsl = translate(Stage::Compute, &[opcode & !0x8000, 0x0002_0501]).unwrap();
            assert!(hlsl.contains(wrapping), "{hlsl}");
        }
        // In e32 the same bit is part of the second source register (v66).
        assert!(translate(Stage::Compute, &[0x4a08_8501]).unwrap().contains("(v1 + v66)"));
    }

    #[test]
    fn mul24_clamps_the_full_product_plus_accumulator_before_packing() {
        for (words, args) in [
            ([0xd50b_8004, 0x0002_0501], "v1, v2, 0u, false"), // unsigned MUL
            ([0xd542_8004, 0x040e_0501], "v1, v2, v3, true"), // signed MAD
            ([0xd543_8004, 0x040e_0501], "v1, v2, v3, false"), // unsigned MAD
            ([0x1208_04f9, 0x0606_3201], "v1, v2, 0u, true"), // signed MUL, BYTE_2
        ] {
            let shader = translate_valu_probe(&words, 5).unwrap();
            assert!(shader.contains(&format!("= tb_mad24_sat({args});")));
            if words[0] == 0x1208_04f9 {
                assert!(shader.contains("0xff00ffffu"));
            }
        }
        let ordinary = translate_valu_probe(&[0xd50b_0004, 0x0002_0501], 5).unwrap();
        assert!(!ordinary.contains("tb_mad24_sat"));
        // Integer products have no floating output scale; SDWA DST_UNUSED=3 is reserved.
        for words in [
            [0xd50b_8004, 0x0802_0501],
            [0x1208_04f9, 0x0606_7201],
            [0x1208_04f9, 0x0606_3a01],
        ] {
            assert!(translate_valu_probe(&words, 5).is_err());
        }
    }

    #[test]
    fn half_rounding_reads_the_entry_destination_and_obeys_fp_mode() {
        // s_denorm_mode 4; v_floor_f16_e64 v6,-|v1| mul:2
        let hlsl = float64::tests::translate(
            Stage::Compute, &[0xbfa5_0004, 0xd5db_0106, 0x2800_0101],
        ).unwrap();
        assert!(hlsl.lines().next().unwrap().split_whitespace().any(|word| word == "v6"));
        let write = hlsl.lines().find(|line| line.contains("v6 & 0xffff0000u")).unwrap();
        assert!(write.contains("tb_unary_f16(") && write.contains("tb_fp_mode, 0u)"));
        assert!(write.contains("tb_half_output(tb_unary_f16(") && write.contains("tb_fp_mode, 2.0, false"));
    }

    #[test]
    fn half_conversion_only_reads_destination_for_partial_writes() {
        // Both directions, signed and unsigned: convert v1.h into v6.h.
        for opcode in [0x7e0c_a0f9, 0x7e0c_a2f9, 0x7e0c_a4f9, 0x7e0c_a6f9, 0x7e0c_c6f9, 0x7e0c_c8f9] {
            for (extension, preserve) in [(0x51501, true), (0x50501, false), (0x50d01, false), (0x51601, false)] {
                let hlsl = float64::tests::translate(Stage::Compute, &[opcode, extension]).unwrap();
                let inputs = hlsl.lines().next().unwrap();
                assert_eq!(inputs.split_whitespace().any(|word| word == "v6"), preserve);
            }
        }
    }

    #[test]
    fn fract_f16_applies_the_mode_and_modifiers_to_the_low_half() {
        // s_denorm_mode 4; v_fract_f16_e64 v4,-|v1| mul:2 (gfx1030).
        let hlsl = float64::tests::translate(
            Stage::Compute, &[0xbfa5_0004, 0xd5df_0104, 0x2800_0101],
        ).unwrap();
        assert!(hlsl.contains("v4 & 0xffff0000u"));
        assert!(hlsl.contains("tb_fp_mode, 4u), tb_fp_mode, 2.0, false)"));
        // An unwritten v6 supplies the high half through the entry ABI.
        let first_write = float64::tests::translate(Stage::Compute, &[0x7e0c_bf04]).unwrap();
        assert!(first_write.lines().next().unwrap().split_whitespace().any(|word| word == "v6"));
    }

    #[test]
    fn fract_f32_uses_the_guest_denormal_mode() {
        // gfx1030: v_fract_f32 v8, v4, optionally preceded by s_denorm_mode 3.
        let fract = 0x7e10_4104;
        for (code, mode) in [(vec![fract], "0u"), (vec![0xbfa5_0003, fract], "tb_fp_mode")] {
            let hlsl = float64::tests::translate(Stage::Compute, &code).unwrap();
            assert!(hlsl.contains(&format!("tb_fract_f32(asfloat(v4), {mode})")));
            assert!(hlsl.contains("float tb_fract_f32("));
        }
    }

    #[test]
    fn conversion_overflow_bit_can_be_set_cleared_and_read() {
        // MODE[23] is FP16_OVFL. Reads including reserved bits remain unsupported.
        let get = 0xb91a_05c1;
        let hlsl = float64::tests::translate(Stage::Compute, &[
            0xba80_05c1, 1, get, 0xba80_05c1, 0, get,
        ]).unwrap();
        assert!(hlsl.contains("((tb_fp_mode >> 23) & 0x1u)"));
        assert!(hlsl.contains("((0x1u & 0x1u) << 23)") && hlsl.contains("((0x0u & 0x1u) << 23)"));
        assert!(float64::tests::translate(Stage::Compute, &[get | (1 << 11)]).is_err());
    }

    #[test]
    fn single_to_half_reads_the_old_destination_only_for_partial_writes() {
        // v_cvt_f16_f32 v6,v1 and SDWA with DWORD source.
        for (words, preserves) in [
            (vec![0x7e0c_1501], true),
            (vec![0x7e0c_14f9, 0x0006_1501], true),  // WORD_1, PRESERVE
            (vec![0x7e0c_14f9, 0x0006_0501], false), // WORD_1, PAD
            (vec![0x7e0c_14f9, 0x0006_1601], false), // DWORD
        ] {
            let hlsl = float64::tests::translate(Stage::Compute, &words).unwrap();
            let incoming = hlsl.lines().next().unwrap().split_whitespace().any(|word| word == "v6");
            assert_eq!(incoming, preserves);
        }
    }

    #[test]
    fn reads_all_mode_fields_through_ieee() {
        // s_getreg_b32 s26, hwreg(MODE, shift, width).
        for shift in 0..10 {
            for width in 1..=10 - shift {
                let get = 0xb91a_0001 | (shift << 6) | ((width - 1) << 11);
                let hlsl = float64::tests::translate(Stage::Compute, &[get]).unwrap();
                let mask = (1u32 << width) - 1;
                assert!(hlsl.contains(&format!("((tb_fp_mode >> {shift}) & {mask:#x}u)")));
            }
        }
        // LOD_CLAMPED (bit 10) is updated by texture instructions.
        for get in [0xb91a_0281, 0xb91a_0a41] {
            assert!(float64::tests::translate(Stage::Compute, &[get]).is_err());
        }
    }

    #[test]
    fn wide_mode_writes_update_readable_bits() {
        // Write MODE[9:0] from s5, overwrite MODE[10:6] with 9, read bit 8.
        let hlsl = float64::tests::translate(Stage::Compute,
            &[0xb985_4801, 0xba80_2181, 9, 0xb91a_0201]).unwrap();
        assert!(hlsl.lines().next().unwrap().split_whitespace().any(|reg| reg == "s5"));
        assert!(hlsl.contains("((s5 & 0x3ffu) << 0)"));
        assert!(hlsl.contains("((0x9u & 0xfu) << 6)"));
        // The immediate form's encoding has a zero in the SGPR field, but
        // leaves the entry value of s0 available to the following v_mov.
        let hlsl = float64::tests::translate(Stage::Compute,
            &[0xba80_2181, 9, 0x7e08_0200]).unwrap();
        assert!(hlsl.lines().next().unwrap().split_whitespace().any(|reg| reg == "s0"));
    }

    #[test]
    fn sdwa_f32_applies_modifiers_before_preserving_unselected_bits() {
        use super::float64::tests::translate;
        // gfx1030: v_add_f32_sdwa v4, -|v1|, v2 clamp mul:2 dst_sel:WORD_1
        // dst_unused:UNUSED_PRESERVE src0_sel:DWORD src1_sel:DWORD
        let hlsl = translate(Stage::Compute, &[0x0608_04f9, 0x0636_7501]).unwrap();
        assert!(hlsl.contains("saturate((((-abs(asfloat(v1))) + asfloat(v2)) * 2.0))"), "{hlsl}");
        assert!(hlsl.contains("(v4) & 0xffffu"), "{hlsl}");
    }

    #[test]
    fn sdwa_f32_scales_a_full_word_without_source_modifiers() {
        use super::float64::tests::translate;
        // gfx1030: v_mul_f32_sdwa v4, v1, v2 div:2 (all selectors DWORD).
        let hlsl = translate(Stage::Compute, &[0x1008_04f9, 0x0606_d601]).unwrap();
        assert!(hlsl.contains("v4 = asuint(((asfloat(v1) * asfloat(v2)) * 0.5))"), "{hlsl}");
    }

    #[test]
    fn sdwa_f32_rounding_applies_input_and_output_modifiers() {
        use super::float64::tests::translate;
        // gfx1030: v_<op>_f32_sdwa v4, -|v1| mul:2, both selectors DWORD.
        for (opcode, intrinsic) in [
            (0x7e08_40f9, "tb_fract_f32"), (0x7e08_42f9, "trunc"), (0x7e08_44f9, "ceil"),
            (0x7e08_46f9, "round"), (0x7e08_48f9, "floor"),
        ] {
            let hlsl = translate(Stage::Compute, &[opcode, 0x0036_5601]).unwrap();
            let mode = if intrinsic == "tb_fract_f32" { ", 0u" } else { "" };
            let result = format!("asuint((({intrinsic}((-abs(asfloat(v1))){mode})) * 2.0))");
            assert!(hlsl.contains(&result), "{hlsl}");
        }
    }

    #[test]
    fn sdwa_integer_to_float_has_float_output_and_integer_input_modifiers() {
        use super::float64::tests::translate;
        // gfx1030: signed/unsigned dword, signed nibble offset, then unsigned bytes 0..3.
        // Each writes v4 from v1 with clamp mul:4 and both selectors DWORD.
        let sdwa = 0x0006_b601;
        for opcode in [0x7e08_0af9, 0x7e08_0cf9, 0x7e08_1cf9, 0x7e08_22f9,
                       0x7e08_24f9, 0x7e08_26f9, 0x7e08_28f9] {
            let hlsl = translate(Stage::Compute, &[opcode, sdwa]).unwrap();
            assert!(hlsl.lines().any(|line| line.contains("v4 = asuint(saturate(")
                && line.ends_with(" * 4.0)));")), "{hlsl}");
            // SRC0_NEG is invalid for an integer input.
            let error = translate(Stage::Compute, &[opcode, sdwa | (1 << 20)]).unwrap_err();
            assert!(error.contains("unsupported SDWA neg/abs"), "{error}");
        }
    }

    #[test]
    fn widen_f16_reads_a_half_precision_source_before_applying_modifiers() {
        use super::float64::tests::translate;
        // gfx1030: v_cvt_f32_f16_e32 v4, 1/(2*pi) (inline selector 248).
        let hlsl = translate(Stage::Compute, &[0x7e08_16f8]).unwrap();
        assert!(hlsl.contains("asfloat(0x3e230000u)"), "{hlsl}");
        // v_cvt_f32_f16_sdwa v4, -|v1| clamp mul:2 dst_sel:DWORD src0_sel:WORD_1
        let hlsl = translate(Stage::Compute, &[0x7e08_16f9, 0x0035_7601]).unwrap();
        assert!(hlsl.contains("((v1 >> 16) & 0xffffu)"), "{hlsl}");
        assert!(hlsl.contains("saturate((((-abs(f16tof32("), "{hlsl}");
    }

    #[test]
    fn byte_pack_uses_the_current_single_precision_round_mode() {
        // gfx1030: v_cvt_pk_u8_f32 v4, v1, v0, v4, with and without s_round_mode.
        let pack = [0xd55e_0004, 0x0412_0101];
        let translate = |words: &[u32]| float64::tests::translate(Stage::Compute, words).unwrap();
        let default = translate(&pack);
        assert!(default.contains("tb_round_u8(asfloat(v1), 0u)"));
        let mut words = vec![0xbfa4_0001];
        words.extend(pack);
        assert!(translate(&words).contains("tb_round_u8(asfloat(v1), tb_fp_mode)"));
    }

    fn dpp16_word(src0_reg: u32, ctrl: u32, fi: u32, bc: u32, bank_m: u32, row_m: u32) -> u32 {
        // [src0_reg:8][ctrl:9][fi:1][bc:1][bank_m:4][row_m:4]
        src0_reg | (ctrl << 8) | (fi << 18) | (bc << 19) | (bank_m << 24) | (row_m << 28)
    }

    fn dpp16_move_insn(ctrl: u32) -> Insn {
        Insn {
            offset: 0x062c,
            dwords: 2,
            enc: Enc::Vop1,
            op: 0,
            raw: vec![0x7e0802fa, ctrl],
            mnemonic: "v_mov_b32".to_string(),
            dst: vec![Opnd::Vgpr(4)],
            src: vec![Opnd::Raw(250)],
            aux: Default::default(),
        }
    }

    #[test]
    fn dpp16_row_moves_select_the_source_lane_by_xor() {
        let mirrors = [(0x141, 7), (0x140, 15)];
        for (ctrl, mask) in mirrors.into_iter().chain((0..16).map(|mask| (0x160 + mask, mask))) {
            let mut insns = [dpp16_move_insn(dpp16_word(2, ctrl, 0, 1, 0xf, 0xf))];
            lower_wave_reductions(&mut insns).unwrap();
            assert_eq!(insns[0].op, OP_ROW_XOR_MOVE);
            assert_eq!(insns[0].src, [Opnd::Vgpr(2)]);
            assert_eq!(insns[0].aux.imm, i64::from(mask));
        }
    }

    #[test]
    fn dpp16_quad_perm_fi1_is_accepted_and_keeps_the_perm_in_aux_imm() {
        let mut insns = vec![dpp16_move_insn(dpp16_word(0, 0x0, 1, 1, 0xf, 0xf))];
        lower_wave_reductions(&mut insns).expect("quad_perm accepted");
        assert_eq!(insns[0].op, OP_QUAD_MOVE);
        assert_eq!(insns[0].src, vec![Opnd::Vgpr(0)]);
        assert_eq!(insns[0].aux.imm, 0);
    }

    #[test]
    fn dpp16_unimplemented_control_is_refused() {
        // Zero is outside the row_ror range. Quad moves with negation are also unsupported.
        let mut insns = vec![dpp16_move_insn(dpp16_word(2, 0x120, 0, 1, 0xf, 0xf))];
        assert!(lower_wave_reductions(&mut insns).is_err());
        let mut insns = vec![dpp16_move_insn(dpp16_word(2, 0x0, 1, 1, 0xf, 0xf) | (1u32 << 20))];
        assert!(lower_wave_reductions(&mut insns).is_err());
    }

    #[test]
    fn dpp16_mov_missing_control_word_is_refused() {
        let mut insns = vec![Insn { raw: vec![0x7e0802fa], ..dpp16_move_insn(0) }];
        assert!(lower_wave_reductions(&mut insns).is_err());
    }

    #[test]
    fn pixel_heap_base_accepts_rooted_resource_heap_without_heap_map() {
        let mut fssm = vec![0u8; 0x124];
        fssm[..8].copy_from_slice(b"FSSM\x01\x01\0\0");
        let len = (fssm.len() - 16) as u32;
        fssm[8..12].copy_from_slice(&len.to_le_bytes());
        fssm[0xe8..0xf0].copy_from_slice(&0x100u64.to_le_bytes());
        fssm[0xf8..0xfc].copy_from_slice(&1u32.to_le_bytes());
        for (k, v) in [5u32, 0, 1, 0, 10].into_iter().enumerate() {
            fssm[0x110 + k * 4..0x114 + k * 4].copy_from_slice(&v.to_le_bytes());
        }
        let mut ex = ShaderExtract {
            stage: Stage::Pixel,
            shader_model: (6, 6),
            required_features: Some(1 << 25 | 1 << 26),
            bitcode_stripped: true,
            fspx_config: None,
            ps09_pixel_inputs: None,
            fssm,
            entry: Some("main".into()),
            isa_data: Vec::new(),
            isa: Vec::new(),
            isa_offset_in_fssm: 0,
            psv_threads: None,
            flat_inputs: Vec::new(),
            flat_outputs: Vec::new(),
            isa_preamble: [0, 64, 1, 1],
            has_input_sig: false,
            has_output_sig: false,
            has_psv0: false,
            has_root_sig: false,
            input_sig: Vec::new(),
            output_sig: Vec::new(),
            resources: Vec::new(),
            gs_info: None,
            mesh_info: None,
            primitive_sig: vec![],
            root_sig: None,
        };
        assert_eq!(pixel_heap_base(&ex, 5), Some(10));
        ex.root_sig = Some(crate::meta::RootSig { version: 2, flags: 0x401, params: vec![], num_static_samplers: 0, static_samplers: vec![] });
        assert_eq!(pixel_heap_base(&ex, 5), Some(10));
        assert_eq!(pixel_heap_base(&ex, 6), None);
        ex.root_sig.as_mut().unwrap().flags = 0x1;
        assert_eq!(pixel_heap_base(&ex, 5), None);
    }

    fn pixel_wave_test_extract() -> ShaderExtract {
        ShaderExtract {
            stage: Stage::Pixel,
            shader_model: (6, 6),
            required_features: None,
            bitcode_stripped: true,
            fspx_config: None,
            ps09_pixel_inputs: None,
            fssm: Vec::new(),
            entry: Some("main".into()),
            isa_data: Vec::new(),
            isa: Vec::new(),
            isa_offset_in_fssm: 0,
            psv_threads: None,
            flat_inputs: Vec::new(),
            flat_outputs: Vec::new(),
            isa_preamble: [0, 64, 1, 1],
            has_input_sig: false,
            has_output_sig: false,
            has_psv0: false,
            has_root_sig: false,
            input_sig: Vec::new(),
            output_sig: Vec::new(),
            resources: Vec::new(),
            gs_info: None,
            mesh_info: None,
            primitive_sig: vec![],
            root_sig: None,
        }
    }

    #[test]
    fn pixel_fi0_quad_exchange_requires_a_proven_exec_mask() {
        let ex = pixel_wave_test_extract();
        let emit = |prefix: &[u32], bound: u32| {
            // GFX1030: v_or_b32_dpp v7, v5, v6 quad_perm:[1,0,3,2].
            let mut words = prefix.to_vec();
            words.extend([0x380e_0cfa, dpp16_word(5, 0xb1, 0, bound, 15, 15)]);
            let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
            let mut insns = tuxbox_dxbc::rdna2::disassemble(&bytes);
            lower_wave_reductions(&mut insns)?;
            let mut em = Emitter::new(&ex, insns);
            em.pixel_wave = true;
            em.emit_body(true)
        };
        let full = 0xbefe_04c1; // s_mov_b64 exec, -1
        let cmpx = 0x7daa_1081; // v_cmpx_ne_u32 1, v8
        for bound in [0, 1] {
            emit(&[full], bound).unwrap();
            emit(&[full, cmpx], bound).unwrap();
            for prefix in [&[][..], &[cmpx], &[0xbf84_0001, full, cmpx]] {
                let error = emit(prefix, bound).unwrap_err();
                assert!(error.contains("requires a proven source EXEC mask"), "{error}");
            }
        }
    }

    #[test]
    fn pixel_wave_quad_exchange_works_in_block_dispatch() {
        let ex = pixel_wave_test_extract();
        let mut insns = vec![Insn { offset: 0x0, ..dpp16_move_insn(dpp16_word(0, 0xb1, 1, 1, 0xf, 0xf)) }];
        lower_wave_reductions(&mut insns).unwrap();
        let mut em = Emitter::new(&ex, insns.clone());
        em.pixel_wave = true;
        em.emit_body_dispatch(true, 0).expect("pixel_wave block dispatch quad exchange");
        assert!(em.body.contains("QuadReadAcrossX("));

        let mut em_non_pw = Emitter::new(&ex, insns);
        let err = em_non_pw.emit_body_dispatch(true, 0).expect_err("non-pixel_wave refused");
        assert!(err.contains("quad exchange in a block-dispatched program"), "{err}");
    }

    #[test]
    fn dpp16_quad_exchange_in_loop_allowed_only_for_pixel_wave_quad_ops() {
        let ex = ShaderExtract {
            stage: Stage::Pixel,
            shader_model: (6, 6),
            required_features: None,
            bitcode_stripped: true,
            fspx_config: None,
            ps09_pixel_inputs: None,
            fssm: Vec::new(),
            entry: Some("main".into()),
            isa_data: Vec::new(),
            isa: Vec::new(),
            isa_offset_in_fssm: 0,
            psv_threads: None,
            flat_inputs: Vec::new(),
            flat_outputs: Vec::new(),
            isa_preamble: [0, 64, 1, 1],
            has_input_sig: false,
            has_output_sig: false,
            has_psv0: false,
            has_root_sig: false,
            input_sig: Vec::new(),
            output_sig: Vec::new(),
            resources: Vec::new(),
            gs_info: None,
            mesh_info: None,
            primitive_sig: vec![],
            root_sig: None,
        };
        let loop_insns = |dpp_word: u32| {
            let mut insns = vec![
                Insn { offset: 0x0, ..dpp16_move_insn(dpp_word) },
                Insn {
                    offset: 0x8,
                    dwords: 1,
                    enc: Enc::Sopp,
                    op: 4, // s_cbranch_scc0
                    raw: vec![0xbf84fffd],
                    mnemonic: "s_cbranch_scc0".into(),
                    dst: vec![],
                    src: vec![],
                    aux: tuxbox_dxbc::rdna2::InsnAux { imm: -3, ..Default::default() },
                },
            ];
            lower_wave_reductions(&mut insns).unwrap();
            insns
        };
        let quad_insns = loop_insns(dpp16_word(0, 0xb1, 1, 1, 0xf, 0xf));
        let mut em = Emitter::new(&ex, quad_insns.clone());
        em.pixel_wave = true;
        em.emit_body(true).expect("pixel_wave quad exchange in loop");
        assert!(em.body.contains("QuadReadAcrossX("));

        let mut em_non_pw = Emitter::new(&ex, quad_insns);
        let err = em_non_pw.emit_body(true).expect_err("non-pixel_wave refused");
        assert!(err.contains("DPP16 exchange inside backward control flow"), "{err}");

        let mirror_insns = loop_insns(dpp16_word(2, 0x140, 0, 1, 0xf, 0xf));
        let mut em_mirror = Emitter::new(&ex, mirror_insns);
        em_mirror.pixel_wave = true;
        let err = em_mirror.emit_body(true).expect_err("row_mirror refused");
        assert!(err.contains("DPP16 exchange inside backward control flow"), "{err}");
    }

    #[test]
    fn f16_interp_descriptor_decodes_attr_channel_and_high() {
        // attr=1, chan=0, high=1 (0b00000001 01_00 00_01)
        assert_eq!(Emitter::f16_interp_descriptor(0x00020101), (1, 0, true));
        // attr=1, chan=1, high=0
        assert_eq!(Emitter::f16_interp_descriptor(0x00020041), (1, 1, false));
        // attr=1, chan=0, high=0
        assert_eq!(Emitter::f16_interp_descriptor(0x00020001), (1, 0, false));
    }

    #[test]
    fn typed_buffer_16_16_16_16_uint_channels_read_16_bit_words_from_two_dwords() {
        // 8-byte element: [lo: ch0, ch1] [hi: ch2, ch3].
        assert_eq!(tbuf_16_16_16_16_uint_channel(0, "packed", "packed1"), "((packed >> 0u) & 65535u)");
        assert_eq!(tbuf_16_16_16_16_uint_channel(1, "packed", "packed1"), "((packed >> 16u) & 65535u)");
        assert_eq!(tbuf_16_16_16_16_uint_channel(2, "packed", "packed1"), "((packed1 >> 0u) & 65535u)");
        assert_eq!(tbuf_16_16_16_16_uint_channel(3, "packed", "packed1"), "((packed1 >> 16u) & 65535u)");
    }

    #[test]
    fn ff1_b64_points_into_low_word_when_it_is_set() {
        assert_eq!(
            ff1_b64_expr("s2", "s3"),
            "(s2 != 0u ? uint(firstbitlow(s2)) : s3 != 0u ? 32u + uint(firstbitlow(s3)) : 0xffffffffu)"
        );
    }

    #[test]
    fn flbit_b64_checks_high_word_before_low_word() {
        assert_eq!(
            flbit_b64_expr("s2", "s3"),
            "(s3 != 0u ? 31u - firstbithigh(s3) : s2 != 0u ? 63u - firstbithigh(s2) : ~0u)"
        );
    }

    #[test]
    fn typed_buffer_extraction_masks_to_16_bits() {
        // Each channel is masked with 0xffff: no channel may leak into a wider one.
        for channel in 0..4u32 {
            let expr = tbuf_16_16_16_16_uint_channel(channel, "a", "b");
            assert!(expr.contains("& 65535u"), "channel {channel}: {expr}");
        }
    }

    #[test]
    fn ff0_b32_inverts_operand_for_firstbitlow() {
        assert_eq!(ff0_b32_expr("s2"), "firstbitlow(~s2)");
    }

    #[test]
    fn ff0_b64_checks_low_word_before_high_word() {
        assert_eq!(
            ff0_b64_expr("s2", "s3"),
            "(~s2 != 0u ? uint(firstbitlow(~s2)) : ~s3 != 0u ? 32u + uint(firstbitlow(~s3)) : 0xffffffffu)"
        );
    }

    // --- Masked prologue decode paths (int and floating) -------------------

    fn masked_insn(mnemonic: &str, enc: Enc, dst: Vec<Opnd>, src: Vec<Opnd>) -> Insn {
        Insn {
            offset: 0x100,
            dwords: 1,
            enc,
            op: 0,
            raw: vec![0],
            mnemonic: mnemonic.to_string(),
            dst,
            src,
            aux: Default::default(),
        }
    }

    fn masked_shuffle(mnemonic: &str, selector: u32, acc: u8, ctrl: u32) -> Insn {
        let mut ins = masked_insn(mnemonic, Enc::Vop2, vec![Opnd::Vgpr(acc)], vec![Opnd::Raw(selector as u16), Opnd::Vgpr(acc)]);
        ins.raw = vec![0x4a0606e9, ctrl << 8 | u32::from(acc)];
        ins
    }

    fn masked_readlane(acc: u8, lane: u32, dst: u8) -> Insn {
        let mut ins = masked_insn(
            "v_readlane_b32",
            Enc::Vop3,
            vec![Opnd::Vgpr(dst)],
            vec![Opnd::Vgpr(acc), Opnd::ConstInt(lane as i64), Opnd::Sgpr(0)],
        );
        ins.raw = vec![0xd760_0000 | u32::from(dst), ((128 + lane) << 9) | 256 | u32::from(acc)];
        ins
    }

    fn masked_cross_row(mnemonic: &str, acc: u8, tmp: u8) -> Insn {
        masked_insn(mnemonic, Enc::Vop2, vec![Opnd::Vgpr(acc)], vec![Opnd::Vgpr(tmp), Opnd::Vgpr(acc)])
    }

    fn masked_perm(acc: u8, tmp: u8) -> Insn {
        masked_insn("v_permlanex16_b32", Enc::Vop3, vec![Opnd::Vgpr(tmp)], vec![Opnd::Vgpr(acc), Opnd::ConstInt(0), Opnd::ConstInt(0)])
    }

    fn masked_reduction_insns(
        width: u32,
        op: &str,
        data_reg: u8,
        float_bits: Option<u32>,
        ident_reg: u8,
    ) -> Vec<Insn> {
        let is_float = float_bits.is_some();
        let acc = 1u8;
        let ctrl = [0xde54c1u32, 0x936012, 0x000924, 0xff0940];
        let selector = [233u32, 233, 233, 250];
        let mut insns = Vec::new();
        // Prologue: saveexec + (literal identity move | AND) + CNDMASK.
        if width == 32 {
            insns.push(masked_insn("s_or_saveexec_b32", Enc::Sop1, vec![Opnd::Sgpr(0)], vec![Opnd::ConstInt(-1)]));
        } else {
            insns.push(masked_insn("s_or_saveexec_b64", Enc::Sop1, vec![Opnd::Sgpr(0)], vec![Opnd::ConstInt(-1)]));
        }
        if let Some(bits) = float_bits {
            insns.push(masked_insn("v_mov_b32", Enc::Vop1, vec![Opnd::Vgpr(ident_reg)], vec![Opnd::Literal(bits)]));
        }
        if width == 32 {
            insns.push(masked_insn("s_and_b32", Enc::Sopc, vec![Opnd::Sgpr(2)], vec![Opnd::Sgpr(26), Opnd::Sgpr(0)]));
        } else {
            insns.push(masked_insn("s_and_b64", Enc::Sopc, vec![Opnd::Sgpr(2)], vec![Opnd::Sgpr(80), Opnd::Sgpr(0)]));
        }
        let cndmask_src = if is_float {
            vec![Opnd::Vgpr(ident_reg), Opnd::Vgpr(data_reg), Opnd::Sgpr(2)]
        } else {
            vec![Opnd::ConstInt(0), Opnd::Vgpr(data_reg), Opnd::Sgpr(2)]
        };
        insns.push(masked_insn("v_cndmask_b32", Enc::Vop2, vec![Opnd::Vgpr(acc)], cndmask_src));
        for (i, (&c, &s)) in ctrl.iter().zip(selector.iter()).enumerate() {
            let _ = i;
            insns.push(masked_shuffle(op, s, acc, c));
        }
        insns.push(masked_perm(acc, ident_reg + 4));
        insns.push(masked_cross_row(op, acc, ident_reg + 4));
        if width == 64 {
            insns.push(masked_readlane(acc, 32, ident_reg + 3));
            insns.push(masked_insn(op, Enc::Vop2, vec![Opnd::Vgpr(acc)], vec![Opnd::Sgpr(ident_reg + 3), Opnd::Vgpr(acc)]));
        }
        insns.push(masked_readlane(acc, 0, ident_reg + 2));
        if width == 32 {
            insns.push(masked_insn("s_mov_b32", Enc::Sop1, vec![Opnd::ExecLo], vec![Opnd::Sgpr(0)]));
        } else {
            insns.push(masked_insn("s_mov_b64", Enc::Sop1, vec![Opnd::ExecLo], vec![Opnd::Sgpr(0)]));
        }
        insns
    }

    #[test]
    fn int_masked_add_prologue_lowers_full_block() {
        let mut insns = masked_reduction_insns(32, "v_add_nc_u32", 3, None, 1);
        lower_wave_reductions(&mut insns).expect("int masked prologue lowered");
        for ins in insns.iter().skip(3).take(4) {
            assert_eq!(ins.op, wave_sum::DPP_REDUCE, "shuffled instruction {ins:?}");
        }
        assert_eq!(insns[9].op, wave_sum::READ_LANE);
        assert_eq!(insns[9].mnemonic, "v_readlane_b32");
        assert_eq!(insns[9].dst, vec![Opnd::Sgpr(3)]);
    }

    #[test]
    fn floating_min_masked_prologue_with_register_identity_lowers() {
        // +inf held in a VGPR materialized by a literal move.
        let mut insns = masked_reduction_insns(64, "v_min_f32", 4, Some(f32::INFINITY.to_bits()), 2);
        lower_wave_reductions(&mut insns).expect("floating masked prologue lowered");
        assert_eq!(insns[4].op, wave_sum::DPP_REDUCE);
        // Both readlanes became READ_LANE and returned scalars.
        assert_eq!(insns[10].op, wave_sum::READ_LANE);
        assert_eq!(insns[10].dst, vec![Opnd::Sgpr(5)]);
        assert_eq!(insns[12].op, wave_sum::READ_LANE);
        assert_eq!(insns[12].dst, vec![Opnd::Sgpr(4)]);
    }

    #[test]
    fn floating_max_masked_prologue_with_register_identity_lowers() {
        // -inf held in a VGPR materialized by a literal move.
        let mut insns = masked_reduction_insns(32, "v_max_f32", 4, Some(f32::NEG_INFINITY.to_bits()), 2);
        lower_wave_reductions(&mut insns).expect("floating masked prologue lowered");
        assert_eq!(insns[4].op, wave_sum::DPP_REDUCE);
        assert_eq!(insns[10].op, wave_sum::READ_LANE);
        assert_eq!(insns[10].dst, vec![Opnd::Sgpr(4)]);
    }

    #[test]
    fn floating_masked_prologue_rejects_unrelated_identity_register() {
        // The identity move lives in a different register than the CNDMASK
        // operand: must refuse, not guess.
        let mut insns = masked_reduction_insns(32, "v_min_f32", 4, Some(f32::INFINITY.to_bits()), 2);
        insns[1].dst = vec![Opnd::Vgpr(9)]; // identity materialized in the wrong register
        assert!(lower_wave_reductions(&mut insns).is_err());
    }

    fn lane_insn(mnemonic: &str, op: u32, dst: Vec<Opnd>, src: Vec<Opnd>) -> Insn {
        Insn {
            offset: 0x100,
            dwords: 1,
            enc: Enc::Vop3,
            op,
            raw: vec![0],
            mnemonic: mnemonic.to_string(),
            dst,
            src,
            aux: Default::default(),
        }
    }

    fn sgpr_lane_readlane(v: u8, lane: u8, dst: u8) -> Insn {
        lane_insn("v_readlane_b32", 0x360,
                  vec![Opnd::Vgpr(dst)],
                  vec![Opnd::Vgpr(v), Opnd::Sgpr(lane), Opnd::Sgpr(0)])
    }

    fn const_lane_readlane(v: u8, lane: i64, dst: u8) -> Insn {
        lane_insn("v_readlane_b32", 0x360,
                  vec![Opnd::Vgpr(dst)],
                  vec![Opnd::Vgpr(v), Opnd::ConstInt(lane), Opnd::Sgpr(0)])
    }

    fn spill_writelane(v: u8, lane: i64, value: u8) -> Insn {
        lane_insn("v_writelane_b32", 0x361,
                  vec![Opnd::Vgpr(v)],
                  vec![Opnd::Sgpr(value), Opnd::ConstInt(lane), Opnd::Sgpr(0)])
    }

    #[test]
    fn sgpr_lane_readlane_becomes_group_read_when_group_reads() {
        let mut insns = vec![sgpr_lane_readlane(4, 2, 7)];
        lower_lane_spills(&mut insns, false, true).expect("group read");
        assert_eq!(insns[0].mnemonic, "tb_group_readlane");
        assert_eq!(insns[0].dst, vec![Opnd::Sgpr(7)]);
        assert_eq!(insns[0].src, vec![Opnd::Vgpr(4), Opnd::Sgpr(2)]);
    }

    #[test]
    fn sgpr_lane_readlane_refused_without_group_reads() {
        let mut insns = vec![sgpr_lane_readlane(4, 2, 7)];
        let err = lower_lane_spills(&mut insns, false, false).expect_err("refused");
        assert!(err.contains("non-constant lane operand"));
    }

    #[test]
    fn sgpr_lane_readlane_refused_on_spill_area() {
        // A vreg written by v_writelane is slotted per constant lane; a dynamic
        // lane cannot address that slot, so it must stay refused.
        let mut insns = vec![spill_writelane(4, 0, 2), sgpr_lane_readlane(4, 2, 7)];
        let err = lower_lane_spills(&mut insns, false, true).expect_err("refused");
        assert!(err.contains("non-constant lane operand"));
    }

    #[test]
    fn const_lane_readlane_still_group_read_when_group_reads() {
        let mut insns = vec![const_lane_readlane(4, 3, 7)];
        lower_lane_spills(&mut insns, false, true).expect("group read");
        assert_eq!(insns[0].mnemonic, "tb_group_readlane");
        assert_eq!(insns[0].dst, vec![Opnd::Sgpr(7)]);
        assert_eq!(insns[0].src, vec![Opnd::Vgpr(4), Opnd::ConstInt(3)]);
    }

    #[test]
    fn floating_max_masked_prologue_rejects_immediate_select_identity() {
        // A select with an immediate identity does not match the register-held
        // floating shape.
        let mut insns = masked_reduction_insns(32, "v_max_f32", 4, Some(f32::NEG_INFINITY.to_bits()), 2);
        insns[3].src[0] = Opnd::ConstInt(f32::NEG_INFINITY.to_bits() as i64);
        assert!(lower_wave_reductions(&mut insns).is_err());
    }
}

#[cfg(test)]
mod election_order_regression {
    use super::*;
    fn op(name: &str, enc: Enc, dst: Vec<Opnd>, src: Vec<Opnd>, offset: usize) -> Insn {
        Insn { offset, dwords: 1, enc, op: 0, raw: vec![0], mnemonic: name.into(), dst, src, aux: Default::default() }
    }
    #[test]
    fn election_ignores_independent_single_word_moves() {
        let mut ops = vec![
            op("v_mbcnt_lo_u32_b32", Enc::Vop3, vec![Opnd::Vgpr(2)], vec![Opnd::ConstInt(-1), Opnd::ConstInt(0)], 0),
            op("s_ff1_i32_b32", Enc::Sop1, vec![Opnd::Sgpr(7)], vec![Opnd::ExecLo], 4),
            op("s_mov_b32", Enc::Sop1, vec![Opnd::Sgpr(8)], vec![Opnd::Sgpr(6)], 8),
            op("v_cmp_eq_u32", Enc::Vop3, vec![Opnd::VccLo], vec![Opnd::Vgpr(2), Opnd::Sgpr(7)], 12),
        ];
        assert!(verify_lane_election(&mut ops, 1, true).is_ok());
        ops[2].mnemonic = "s_mov_b64".into(); // s[6:7] reads the elected lane.
        assert!(verify_lane_election(&mut ops, 1, true).is_err());
        ops[2].mnemonic = "s_mov_b32".into();
        ops[2].dst = vec![Opnd::Sgpr(7)];
        assert!(verify_lane_election(&mut ops, 1, true).is_err());
    }

    #[test]
    fn later_saveexec_uses_the_redefined_vcc() {
        let mut ops = vec![
            op("v_mbcnt_lo_u32_b32", Enc::Vop3, vec![Opnd::Vgpr(2)], vec![Opnd::ConstInt(-1), Opnd::ConstInt(0)], 0),
            op("s_ff1_i32_b32", Enc::Sop1, vec![Opnd::Sgpr(7)], vec![Opnd::ExecLo], 4),
            op("v_cmp_eq_u32", Enc::Vopc, vec![Opnd::VccLo], vec![Opnd::Sgpr(7), Opnd::Vgpr(2)], 8),
            op("v_cmp_eq_u32", Enc::Vopc, vec![Opnd::VccLo], vec![Opnd::ConstInt(0), Opnd::Vgpr(4)], 12),
            op("s_and_saveexec_b32", Enc::Sop1, vec![Opnd::Sgpr(20)], vec![Opnd::VccLo], 16),
        ];
        assert!(verify_lane_election(&mut ops, 1, true).is_ok());
        ops.remove(3);
        assert!(verify_lane_election(&mut ops, 1, true).is_err());
    }

    #[test]
    fn symmetric_equality_still_requires_full_lane_count() {
        let mut ops = vec![
            op("v_mbcnt_lo_u32_b32", Enc::Vop3, vec![Opnd::Vgpr(2)], vec![Opnd::ConstInt(-1), Opnd::ConstInt(0)], 0),
            op("s_ff1_i32_b32", Enc::Sop1, vec![Opnd::Sgpr(7)], vec![Opnd::ExecLo], 4),
            op("v_cmp_eq_u32", Enc::Vop3, vec![Opnd::VccLo], vec![Opnd::Vgpr(2), Opnd::Sgpr(7)], 8),
        ];
        assert!(verify_lane_election(&mut ops, 1, true).is_ok());
        ops[2].src.swap(0, 1);
        assert!(verify_lane_election(&mut ops, 1, true).is_ok());
        ops[2].mnemonic = "v_cmp_ne_u32".into();
        assert!(verify_lane_election(&mut ops, 1, true).is_err());
        ops[2].mnemonic = "v_cmp_eq_u32".into();
        ops[0].src[0] = Opnd::ConstInt(0);
        assert!(verify_lane_election(&mut ops, 1, true).is_err());
    }
}

#[cfg(test)]
mod reduction_execz_regression {
    use super::*;

    mod fixture {
        include!("../examples/support/wave_guard_fixture.rs");
    }

    /// Build native masked reductions with optional pre-guard edges and an
    /// independent tail EXECZ; width selects the verified 32/64-lane fixture.
    fn guarded_shader(width: u32, prefix: &[u32]) -> ShaderExtract {
        let (mut shader, mut words, end) = fixture::shader(width);
        words.truncate(end);
        let skip = words.len() as u32;
        words.splice(..0, prefix.iter().copied().chain([0xbe87_0391, 0xbf88_0000 | skip]));
        words.extend([0xbf88_0001, 0xbf80_0000, 0xbf81_0000]);
        shader.isa = words.into_iter().flat_map(u32::to_le_bytes).collect();
        shader.isa_data = shader.isa.clone();
        shader
    }

    #[test]
    fn reduction_execz_votes_only_at_proven_guards() {
        for width in [32, 64] {
            let translated = translate(&guarded_shader(width, &[])).expect("guarded reduction");
            assert_eq!(translated.required_wave_size, Some(width));
            assert_eq!(translated.hlsl.matches("!WaveActiveAnyTrue((exec_lo & 1u) != 0u)").count(), 1);
            assert!(translated.hlsl.contains("if ((exec_lo & 1u) == 0u)"));
        }
    }

    #[test]
    fn reduction_execz_accepts_direct_rejoin_at_vote() {
        for width in [32, 64] {
            // Skip one local vector instruction and rejoin exactly at the guard.
            let translated = translate(&guarded_shader(width, &[0xbf88_0002, 0x4a00_0081]))
                .expect("direct rejoin before wave vote");
            assert_eq!(translated.hlsl.matches("!WaveActiveAnyTrue((exec_lo & 1u) != 0u)").count(), 1);
        }
    }

    #[test]
    fn reduction_execz_behind_a_bypass_branches_with_the_whole_wave() {
        for width in [32, 64] {
            let (mut shader, mut words, end) = fixture::shader(width);
            words.truncate(end);
            let skip = words.len() as u32 + 1;
            // The first edge bypasses the vote but lands before the prologue.
            words.splice(..0, [0xbf88_0002, 0xbe87_0391, 0xbf88_0000 | skip, 0xbf80_0000]);
            words.push(0xbf81_0000);
            shader.isa = words.into_iter().flat_map(u32::to_le_bytes).collect();
            shader.isa_data = shader.isa.clone();
            // One targeted vote cannot cover this shape, so every branch votes.
            let translated = translate(&shader).expect("whole-wave branches");
            assert_eq!(translated.hlsl.matches("!WaveActiveAnyTrue((exec_lo & 1u) != 0u)").count(), 2);
            assert!(!translated.hlsl.contains("if ((exec_lo & 1u) == 0u)"));
        }
    }

    #[test]
    fn dispatched_lanes_wait_for_their_wave_at_a_barrier() {
        let op = |offset, op, mnemonic: &str, imm| Insn {
            offset, dwords: 1, enc: Enc::Sopp, op, raw: vec![0xbf80_0000 | op << 16], mnemonic: mnemonic.into(),
            dst: Vec::new(), src: Vec::new(), aux: tuxbox_dxbc::rdna2::InsnAux { imm, ..Default::default() },
        };
        let emit = |barrier: bool| {
            let shader = guarded_shader(32, &[]);
            // A lane leaves the loop at its first EXECZ and may reach the barrier before the others.
            let mut emitter = Emitter::new(&shader, vec![
                op(0, 8, "s_cbranch_execz", 1),
                op(4, 2, "s_branch", -2),
                if barrier { op(8, 0x0a, "s_barrier", 0) } else { op(8, 0, "s_nop", 0) },
                op(12, 1, "s_endpgm", 0),
            ]);
            emitter.emit_body_dispatch(false, 0).unwrap();
            emitter.body
        };
        let body = emit(true);
        assert!(body.contains("bool tb_wait = tb_pc == 2u;"), "{body}");
        let block = body.find("if (tb_sel == 2u) {").expect("the barrier starts a block");
        let wait = body.find("WaveActiveAnyTrue(!tb_wait)").expect("waits for its wave");
        let sync = body.find("GroupMemoryBarrierWithGroupSync();").unwrap();
        assert!(wait < block && block < sync, "{body}");
        assert!(body.contains("uint tb_sel = tb_wait ? 0xfffffffeu : tb_pc;"), "{body}");
        let body = emit(false);
        assert!(!body.contains("tb_wait") && body.contains("uint tb_sel = tb_pc;"), "{body}");
    }

    #[test]
    fn reduction_execz_refuses_block_dispatch() {
        let shader = guarded_shader(32, &[]);
        let mut emitter = Emitter::new(&shader, Vec::new());
        emitter.reduction_execz_guards.insert(0);
        assert!(emitter.emit_body_dispatch(false, 0).unwrap_err().contains("EXECZ vote"));
    }
}
