//! `v_rndne_f32` / `v_rndne_f16`: round to nearest, ties to even, spelled out
//! rather than left to HLSL `round`, whose tie rule is the back end's choice
//! (DXIL lowers it to round_ne, other targets may round ties away). x - trunc(x)
//! is exact in binary32, so the tie test is exact; signed zero, infinities and
//! NaN pass through. TB_RNDNE_DRIVER=1 emits `round` instead.

pub(super) const HELPER: &str = r#"
float tb_rndne(float x) {
    float t = trunc(x);
    float d = abs(x - t);
    if (d > 0.5 || (d == 0.5 && fmod(t, 2.0) != 0.0)) t += x < 0.0 ? -1.0 : 1.0;
    return t;
}
"#;

pub(super) fn rndne(value: &str) -> String {
    if std::env::var("TB_RNDNE_DRIVER").is_ok_and(|v| v.trim() == "1") {
        format!("round({value})")
    } else {
        format!("tb_rndne({value})")
    }
}

#[cfg(test)]
mod tests {
    use super::super::translate_valu_probe;

    /// The emitted helper, transcribed (HLSL fmod keeps the dividend's sign).
    fn tb_rndne(x: f32) -> f32 {
        let mut t = x.trunc();
        let d = (x - t).abs();
        if d > 0.5 || (d == 0.5 && t % 2.0 != 0.0) {
            t += if x < 0.0 { -1.0 } else { 1.0 };
        }
        t
    }

    #[test]
    fn ties_go_to_even_and_specials_pass_through() {
        for (x, want) in [(0.5f32, 0.0f32), (1.5, 2.0), (2.5, 2.0), (-0.5, -0.0), (-1.5, -2.0), (-2.5, -2.0), (0.49999997, 0.0), (8388607.5, 8388608.0), (8388609.0, 8388609.0), (-7.75, -8.0)] {
            assert_eq!(tb_rndne(x).to_bits(), want.to_bits(), "{x}");
        }
        assert_eq!(tb_rndne(-0.3).to_bits(), (-0.0f32).to_bits());
        assert_eq!(tb_rndne(f32::INFINITY), f32::INFINITY);
        assert_eq!(tb_rndne(f32::NEG_INFINITY), f32::NEG_INFINITY);
        assert!(tb_rndne(f32::NAN).is_nan());
        // Every binary32 in a stride across the whole range against the host's
        // ties-to-even rounding.
        let mut bits = 0u32;
        loop {
            let x = f32::from_bits(bits);
            if !x.is_nan() {
                assert_eq!(tb_rndne(x).to_bits(), x.round_ties_even().to_bits(), "{bits:#010x}");
            }
            match bits.checked_add(4099) {
                Some(next) => bits = next,
                None => break,
            }
        }
        // Every binary16 value (exact in binary32), as the f16 form evaluates it.
        for h in 0..=0xffffu16 {
            let x = half_to_f32(h);
            if !x.is_nan() {
                assert_eq!(tb_rndne(x).to_bits(), x.round_ties_even().to_bits(), "{h:#06x}");
            }
        }
    }

    fn half_to_f32(h: u16) -> f32 {
        let (e, m) = (i32::from((h >> 10) & 31), f32::from(h & 0x3ff));
        let v = match e {
            31 if m == 0.0 => f32::INFINITY,
            31 => f32::NAN,
            0 => m * 2f32.powi(-24),
            _ => (1024.0 + m) * 2f32.powi(e - 25),
        };
        if h & 0x8000 != 0 { -v } else { v }
    }

    /// VOP1 `op v0, v7` (src0 = v7, the opaque register the reference method
    /// feeds exact-result inputs through).
    fn vop1(op: u32) -> u32 {
        0x7e00_0000 | (op << 9) | 0x107
    }

    #[test]
    fn unary_math_translates_to_the_exact_forms() {
        let cases: [(u32, &str); 12] = [
            (35, "tb_rndne("),
            (94, "tb_rndne("),
            (42, "1.0 / "),
            (84, "1.0 / "),
            (46, "rsqrt("),
            (86, "rsqrt("),
            (51, "sqrt("),
            (85, "sqrt("),
            (36, "floor("),
            (91, "floor("),
            (33, "trunc("),
            (93, "trunc("),
        ];
        for (op, want) in cases {
            let hlsl = translate_valu_probe(&[vop1(op)], 8).unwrap();
            assert!(hlsl.contains(want), "op {op}: {hlsl}");
            assert!(hlsl.contains("v7"), "op {op} reads v7: {hlsl}");
            if want != "tb_rndne(" {
                assert!(!hlsl.lines().any(|l| l.contains("v0 =") && l.contains("round(")), "op {op}: {hlsl}");
            }
        }
        for op in [35, 94] {
            let hlsl = translate_valu_probe(&[vop1(op)], 8).unwrap();
            assert!(!hlsl.lines().any(|l| l.contains("v0 =") && l.contains(" round(")), "op {op}: {hlsl}");
        }
    }
}
