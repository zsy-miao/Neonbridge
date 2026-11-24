//! `v_cvt_norm_i16_f16` / `v_cvt_norm_u16_f16`: a binary16 value, after its
//! abs/neg modifiers, clamped to [-1, 1] (signed) or [0, 1] (unsigned) and
//! scaled by 32767 or 65535. The product of the 11-bit significand and the
//! scale is exact in 27 bits, so the result is rounded once, to nearest even,
//! from the exact value (a float multiply would round twice). NaN gives 0.
//! TB_CVT_NORM_FLOAT=1 uses the driver's float multiply and round instead.

pub(super) const HELPER: &str = r#"
uint tb_cvt_norm16(uint h, uint scale, bool signedResult) {
    uint a = h & 0x7fffu;
    bool neg = (h & 0x8000u) != 0u;
    if (a > 0x7c00u || (neg && !signedResult)) return 0u;
    uint q = scale;
    if (a < 0x3c00u) {
        uint e = a >> 10;
        uint p = ((a & 0x3ffu) | (e != 0u ? 0x400u : 0u)) * scale;
        uint shift = 25u - max(e, 1u);
        uint rem = p & ((1u << shift) - 1u), halfway = 1u << (shift - 1u);
        q = p >> shift;
        if (rem > halfway || (rem == halfway && (q & 1u) != 0u)) q += 1u;
    }
    return (neg ? 0u - q : q) & 0xffffu;
}
"#;

pub(super) fn uses(mnemonic: &str) -> bool {
    matches!(mnemonic, "v_cvt_norm_i16_f16" | "v_cvt_norm_u16_f16")
}

/// The HLSL expression for the instruction, given the source's binary16 bits
/// (modifiers already applied to the bits) and its float value (modifiers
/// applied) for the driver fallback.
pub(super) fn expression(mnemonic: &str, bits: &str, value: &str) -> String {
    let signed = mnemonic == "v_cvt_norm_i16_f16";
    if std::env::var("TB_CVT_NORM_FLOAT").is_ok_and(|v| v.trim() == "1") {
        return if signed {
            format!("(asuint((int)round(clamp({value}, -1.0, 1.0) * 32767.0)) & 0xffffu)")
        } else {
            format!("(uint)round(clamp({value}, 0.0, 1.0) * 65535.0)")
        };
    }
    let (scale, flag) = if signed { (32767, "true") } else { (65535, "false") };
    format!("tb_cvt_norm16({bits}, {scale}u, {flag})")
}

#[cfg(test)]
mod tests {
    use super::super::translate_valu_probe;

    /// The emitted helper, transcribed.
    fn norm16(h: u32, scale: u32, signed: bool) -> u32 {
        let a = h & 0x7fff;
        let neg = h & 0x8000 != 0;
        if a > 0x7c00 || (neg && !signed) {
            return 0;
        }
        let mut q = scale;
        if a < 0x3c00 {
            let e = a >> 10;
            let p = ((a & 0x3ff) | if e != 0 { 0x400 } else { 0 }) * scale;
            let shift = 25 - e.max(1);
            let rem = p & ((1 << shift) - 1);
            let half = 1 << (shift - 1);
            q = p >> shift;
            if rem > half || (rem == half && q & 1 != 0) {
                q += 1;
            }
        }
        (if neg { 0u32.wrapping_sub(q) } else { q }) & 0xffff
    }

    /// Exact reference: f16 -> f64 (exact), clamp, scale (exact in f64), RNE.
    fn reference(h: u32, signed: bool) -> u32 {
        let a = h & 0x7fff;
        if a > 0x7c00 {
            return 0;
        }
        let e = (a >> 10) as i32;
        let m = f64::from(a & 0x3ff);
        let mag = if e == 0x1f { f64::INFINITY } else if e == 0 { m * 2f64.powi(-24) } else { (1024.0 + m) * 2f64.powi(e - 25) };
        let x = if h & 0x8000 != 0 { -mag } else { mag };
        let (lo, scale) = if signed { (-1.0, 32767.0) } else { (0.0, 65535.0) };
        let y = (x.clamp(lo, 1.0) * scale).round_ties_even();
        (y as i32 as u32) & 0xffff
    }

    #[test]
    fn every_half_rounds_once_from_the_exact_product() {
        for h in 0..=0xffffu32 {
            assert_eq!(norm16(h, 32767, true), reference(h, true), "i16 {h:#06x}");
            assert_eq!(norm16(h, 65535, false), reference(h, false), "u16 {h:#06x}");
        }
        assert_eq!(norm16(0x3c00, 32767, true), 0x7fff);
        assert_eq!(norm16(0xfc00, 32767, true), 0x8001);
        assert_eq!(norm16(0x7e00, 65535, false), 0);
        assert_eq!(norm16(0xbc00, 65535, false), 0);
    }

    #[test]
    fn both_forms_translate_to_the_helper() {
        // VOP1 v_cvt_norm_i16_f16 v0, v1 (op 99) and v_cvt_norm_u16_f16 v0, v1 (op 100).
        let i = translate_valu_probe(&[0x7e00_c701], 2).unwrap();
        assert!(i.contains("tb_cvt_norm16(") && i.contains("32767u, true)"), "{i}");
        let u = translate_valu_probe(&[0x7e00_c901], 2).unwrap();
        assert!(u.contains("65535u, false)"), "{u}");
    }

    #[test]
    fn cvt_16_from_half_saturates_and_masks() {
        // VOP1 v_cvt_u16_f16 v0, v1 (op 82) and v_cvt_i16_f16 v0, v1 (op 83).
        let u = translate_valu_probe(&[0x7e00_a501], 2).unwrap();
        assert!(u.contains("> 0x7c00u ? 0u") && u.contains("clamp(") && u.contains("0.0, 65535.0)"), "{u}");
        let i = translate_valu_probe(&[0x7e00_a701], 2).unwrap();
        assert!(i.contains("> 0x7c00u ? 0u") && i.contains("-32768.0, 32767.0)) & 0xffffu"), "{i}");
        // The emitted expressions, transcribed: 32768..65504 saturate, infinities go to
        // the limits, NaN gives 0, negatives give 0 for u16, upper bits stay zero.
        let half = |h: u32| -> f32 {
            let e = ((h >> 10) & 31) as i32;
            let m = (h & 0x3ff) as f32;
            let v = if e == 31 { f32::INFINITY } else if e == 0 { m * 2f32.powi(-24) } else { (1024.0 + m) * 2f32.powi(e - 25) };
            if h & 0x8000 != 0 { -v } else { v }
        };
        let to_u16 = |h: u32| if h & 0x7fff > 0x7c00 { 0 } else { half(h).clamp(0.0, 65535.0) as u32 };
        let to_i16 = |h: u32| if h & 0x7fff > 0x7c00 { 0 } else { (half(h).clamp(-32768.0, 32767.0) as i32 as u32) & 0xffff };
        assert_eq!(to_i16(0x7800), 0x7fff); // 32768
        assert_eq!(to_i16(0x7bff), 0x7fff); // 65504
        assert_eq!(to_i16(0x7c00), 0x7fff);
        assert_eq!(to_i16(0xfc00), 0x8000);
        assert_eq!(to_i16(0xbc00), 0xffff); // -1, upper bits zero
        assert_eq!(to_i16(0x7e00), 0);
        assert_eq!(to_u16(0x7bff), 65504);
        assert_eq!(to_u16(0x7c00), 65535);
        assert_eq!(to_u16(0xbc00), 0);
        assert_eq!(to_u16(0xfc00), 0);
        assert_eq!(to_u16(0xfe00), 0);
    }
}
