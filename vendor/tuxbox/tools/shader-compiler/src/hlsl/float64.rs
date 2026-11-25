//! Exactly specified double-precision forms on register pairs: rounding to
//! an integral value and conversions between f64 and 32-bit integers.
//! Like the f64 comparisons they use bit operations on `uint2` (low, high)
//! pairs, so no host FP64 support is required. Each result is exact; the
//! only mode dependence is whether f64 input denormals are flushed.
use super::{r, Emitter, Enc, Insn, Opnd};

pub(super) const HELPERS: &str = r#"
uint2 tb_f64_round(uint2 x, uint mode, bool keepDenormals) {
    // mode: 0 toward zero, 1 floor, 2 ceil, 3 nearest even.
    uint e = (x.y >> 20) & 0x7ffu, sign = x.y & 0x80000000u;
    if (e == 0x7ffu) return ((x.y & 0xfffffu) | x.x) != 0u ? uint2(x.x, x.y | 0x80000u) : x;
    if (e >= 1075u) return x;
    if (e == 0u && !keepDenormals) x = uint2(0u, sign);
    bool nonzero = ((x.y & 0x7fffffffu) | x.x) != 0u;
    if (e < 1023u) {
        bool one = mode == 1u ? (sign != 0u && nonzero) : mode == 2u ? (sign == 0u && nonzero)
            : mode == 3u && e == 1022u && ((x.y & 0xfffffu) | x.x) != 0u;
        return uint2(0u, sign | (one ? 0x3ff00000u : 0u));
    }
    uint f = 1075u - e;
    uint2 mask = f >= 32u ? uint2(0xffffffffu, (1u << (f - 32u)) - 1u) : uint2((1u << f) - 1u, 0u);
    uint2 unit = f >= 32u ? uint2(0u, 1u << (f - 32u)) : uint2(1u << f, 0u);
    uint2 frac = x & mask, t = x & ~mask;
    bool hasFrac = (frac.x | frac.y) != 0u, up = false;
    if (mode == 1u) up = sign != 0u && hasFrac;
    if (mode == 2u) up = sign == 0u && hasFrac;
    if (mode == 3u) {
        uint2 half = f > 32u ? uint2(0u, 1u << (f - 33u)) : uint2(1u << (f - 1u), 0u);
        bool above = frac.y > half.y || (frac.y == half.y && frac.x > half.x);
        bool tie = frac.x == half.x && frac.y == half.y;
        up = above || (tie && ((t.x & unit.x) | (t.y & unit.y)) != 0u);
    }
    if (up) {
        uint lo = t.x + unit.x;
        t = uint2(lo, t.y + unit.y + (lo < t.x ? 1u : 0u));
    }
    return t;
}
uint tb_f64_to_i32(uint2 x, bool signedResult) {
    // Truncate; saturate out-of-range values and return 0 for NaN.
    uint e = (x.y >> 20) & 0x7ffu;
    bool neg = (x.y >> 31) != 0u;
    if (e == 0x7ffu && ((x.y & 0xfffffu) | x.x) != 0u) return 0u;
    if (e < 1023u) return 0u;
    if (signedResult && e >= 1054u) return neg ? 0x80000000u : 0x7fffffffu;
    if (!signedResult && neg) return 0u;
    if (!signedResult && e >= 1055u) return 0xffffffffu;
    uint s = 52u - (e - 1023u);
    uint hi = (x.y & 0xfffffu) | 0x100000u;
    uint v = s >= 32u ? hi >> (s - 32u) : (hi << (32u - s)) | (x.x >> s);
    return signedResult && neg ? 0u - v : v;
}
uint2 tb_i32_to_f64(uint v, bool signedInput) {
    uint sign = 0u;
    if (signedInput && (v >> 31) != 0u) { sign = 0x80000000u; v = 0u - v; }
    if (v == 0u) return uint2(0u, 0u);
    uint p = firstbithigh(v), rest = v ^ (1u << p), sh = 52u - p;
    uint2 m = sh >= 32u ? uint2(0u, rest << (sh - 32u)) : uint2(rest << sh, rest >> (32u - sh));
    return uint2(m.x, sign | ((1023u + p) << 20) | m.y);
}
"#;

/// Forms whose source is an f64 register pair.
fn pair_source(m: &str) -> bool {
    matches!(m, "v_trunc_f64" | "v_floor_f64" | "v_ceil_f64" | "v_rndne_f64" | "v_cvt_i32_f64" | "v_cvt_u32_f64")
}

/// Forms whose result is an f64 register pair.
pub(super) fn pair_result(m: &str) -> bool {
    matches!(m, "v_trunc_f64" | "v_floor_f64" | "v_ceil_f64" | "v_rndne_f64" | "v_cvt_f64_i32" | "v_cvt_f64_u32")
}

pub(super) fn handles(m: &str) -> bool {
    pair_source(m) || pair_result(m)
}

/// High register of each pair source, for the live-in analysis.
pub(super) fn pair_source_highs(ins: &Insn) -> Option<u32> {
    match ins.src.first() {
        Some(Opnd::Vgpr(n)) if pair_source(&ins.mnemonic) => Some(u32::from(*n) + 1),
        _ => None,
    }
}

impl Emitter<'_> {
    pub(super) fn lower_f64(&mut self, ins: &Insn) -> Result<bool, String> {
        let m = ins.mnemonic.as_str();
        if !handles(m) {
            return Ok(false);
        }
        // Ordinary VGPR operands only; the VOP3 form without abs, neg,
        // clamp or omod. Verified in compute shaders only.
        let plain = match ins.enc {
            Enc::Vop1 => ins.aux.sdwa.is_none() && ins.raw.len() == 1,
            Enc::Vop3 => ins.raw.len() == 2 && ins.raw[0] & 0xff00 == 0 && ins.raw[1] >> 27 == 0,
            _ => false,
        };
        let (Some(Opnd::Vgpr(dst)), Some(Opnd::Vgpr(src))) = (ins.dst.first(), ins.src.first()) else {
            return Err("unsupported f64 operand".into());
        };
        if !plain || !ins.is_complete() {
            return Err("unsupported f64 encoding or modifiers".into());
        }
        if self.ex.stage != super::Stage::Compute {
            return Err("unverified f64 shader stage".into());
        }
        let (dst, src) = (*dst, *src);
        if (pair_result(m) && dst == u8::MAX) || (pair_source(m) && src == u8::MAX) {
            return Err("VGPR pair overflows".into());
        }
        let source = if pair_source(m) {
            let high = Opnd::Vgpr(src + 1);
            self.touch(&high);
            format!("uint2({}, {})", r(&Opnd::Vgpr(src))?, r(&high)?)
        } else {
            r(&Opnd::Vgpr(src))?
        };
        let keep = if self.regs.contains("tb_fp_mode") { "(tb_fp_mode & 0x40u) != 0u" } else { "true" };
        let value = match m {
            "v_trunc_f64" => format!("tb_f64_round({source}, 0u, {keep})"),
            "v_floor_f64" => format!("tb_f64_round({source}, 1u, {keep})"),
            "v_ceil_f64" => format!("tb_f64_round({source}, 2u, {keep})"),
            "v_rndne_f64" => format!("tb_f64_round({source}, 3u, {keep})"),
            "v_cvt_f64_i32" => format!("tb_i32_to_f64({source}, true)"),
            "v_cvt_f64_u32" => format!("tb_i32_to_f64({source}, false)"),
            "v_cvt_i32_f64" => format!("tb_f64_to_i32({source}, true)"),
            _ => format!("tb_f64_to_i32({source}, false)"),
        };
        if !pair_result(m) {
            self.wv(&Opnd::Vgpr(dst), &value)?;
            return Ok(true);
        }
        let t = self.tmp;
        self.tmp += 1;
        self.line(&format!("uint2 t{t}f64 = {value};"));
        self.wv(&Opnd::Vgpr(dst), &format!("t{t}f64.x"))?;
        let high = Opnd::Vgpr(dst + 1);
        self.touch(&high);
        self.wv(&high, &format!("t{t}f64.y"))?;
        Ok(true)
    }
}

#[cfg(test)]
#[path = "../tests/hlsl/float64.rs"]
pub(super) mod tests;
