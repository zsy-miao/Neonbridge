//! Floating-point comparisons and 64-bit integer register pairs. Bit operations keep these usable on SM5
//! hardware and preserve both NaN classes without host FP64 arithmetic.
use super::{r, Emitter, Insn, Opnd};

pub(super) const FLOAT_HELPERS: &str = r#"
uint tb_float_relation(uint2 a, uint2 b, uint sign, uint infinity, uint minNormal, bool keepDenormals) {
    uint ax = a.y & (sign-1u), bx = b.y & (sign-1u);
    if (ax > infinity || (ax == infinity && a.x != 0u) ||
        bx > infinity || (bx == infinity && b.x != 0u)) return 8u;
    if (!keepDenormals) {
        if (ax < minNormal) { a.x = 0u; ax = 0u; }
        if (bx < minNormal) { b.x = 0u; bx = 0u; }
    }
    bool negA = (a.y & sign) != 0u, negB = (b.y & sign) != 0u;
    if ((ax | a.x | bx | b.x) == 0u) return 2u;
    if (negA != negB) return negA ? 1u : 4u;
    if (ax == bx && a.x == b.x) return 2u;
    bool lessMagnitude = ax < bx || (ax == bx && a.x < b.x);
    return (lessMagnitude != negA) ? 1u : 4u;
}
bool tb_class_f64(uint2 bits, uint mask) {
    // Fold payload bits below bit 32 into the least significant fraction
    // bit. The quiet bit, exponent, sign and test for zero are preserved.
    return tb_cmp_class(bits.y | uint(bits.x != 0u), mask, 20u, 2047u);
}
"#;

fn sign_modifiers(mut bits: String, sign: u32, mask: u8, source: u8) -> String {
    if mask & (1 << source) != 0 {
        bits = format!("({bits} & {:#x}u)", sign - 1);
    }
    if mask & (8 << source) != 0 {
        bits = format!("({bits} ^ {sign:#x}u)");
    }
    bits
}

impl Emitter<'_> {
    fn compare_pair(&mut self, operand: &Opnd, ty: &str) -> Result<(String, String), String> {
        let constant = match *operand {
            Opnd::Literal(bits) => Some(match ty {
                "f64" => u64::from(bits) << 32,
                "i64" => i64::from(bits as i32) as u64,
                _ => u64::from(bits),
            }),
            // RDNA2 ISA table 20 defines the double-precision ROM entry for
            // 1/(2*pi) independently of the rounded single-precision value.
            Opnd::ConstF32(value) => Some(if value.to_bits() == 0x3e22_f983 {
                0x3fc4_5f30_6dc9_c882
            } else {
                f64::from(value).to_bits()
            }),
            _ => None,
        };
        if let Some(bits) = constant {
            return Ok((
                format!("{:#x}u", bits as u32),
                format!("{:#x}u", bits >> 32),
            ));
        }
        Ok((r(operand)?, self.pair_high(operand)?))
    }

    /// V_CVT_F32_F64's result bits; abs and neg act on the binary64 sign.
    pub(super) fn cvt_f32_f64(&mut self, ins: &Insn) -> Result<String, String> {
        let (low, high) = self.compare_pair(&ins.src[0], "f64")?;
        let high = sign_modifiers(high, 0x8000_0000, ins.aux.mask, 0);
        Ok(format!("tb_cvt_f32_f64({low}, {high})"))
    }

    pub(super) fn compare_float(&mut self, ins: &Insn, mask: u8) -> Result<String, String> {
        let (sign, infinity, min_normal, input_mode) = if ins.mnemonic.ends_with("_f64") {
            (0x8000_0000, 0x7ff0_0000, 0x0010_0000, 0x40)
        } else if ins.mnemonic.ends_with("_f32") {
            (0x8000_0000, 0x7f80_0000, 0x0080_0000, 0x10)
        } else {
            (0x8000, 0x7c00, 0x0400, 0x40)
        };
        let mut source = |i: usize| -> Result<String, String> {
            let operand = &ins.src[i];
            let (low, high) = if ins.mnemonic.ends_with("_f64") {
                self.compare_pair(operand, "f64")?
            } else if sign == 0x8000 && matches!(operand, Opnd::ConstF32(_)) {
                ("0u".into(), format!("tb_f16_rne(asfloat({}))", r(operand)?))
            } else {
                ("0u".into(), r(operand)?)
            };
            let high = sign_modifiers(high, sign, mask, i as u8);
            Ok(format!("uint2({low}, {high})"))
        };
        let a = source(0)?;
        if ins.mnemonic.contains("_class_") {
            // Classification uses the original bits even in flush-to-zero modes.
            let classes = r(&ins.src[1])?;
            return Ok(if ins.mnemonic.ends_with("_f64") {
                format!("tb_class_f64({a}, {classes})")
            } else {
                let (fraction, exponent) = if sign == 0x8000 { (10, 31) } else { (23, 255) };
                format!("tb_cmp_class(({a}).y, {classes}, {fraction}u, {exponent}u)")
            });
        }
        let b = source(1)?;
        let mode = if self.regs.contains("tb_fp_mode") {
            "tb_fp_mode"
        } else {
            "0xc0u"
        };
        // Binary16 predicates straddle two opcode blocks. The upper block
        // complements each ordered predicate and includes the unordered case.
        let pred = if sign == 0x8000 {
            (ins.op & 7) | ((ins.op >> 2) & 8)
        } else {
            ins.op & 15
        };
        Ok(format!("((tb_float_relation({a}, {b}, {sign:#x}u, {infinity:#x}u, {min_normal:#x}u, ({mode} & {input_mode:#x}u) != 0u) & {pred}u) != 0u)"))
    }

    pub(super) fn compare_int64(&mut self, ins: &Insn, mask: u8) -> Result<String, String> {
        let ty = if ins.mnemonic.ends_with("_i64") {
            "i64"
        } else {
            "u64"
        };
        let (a, ah) = self.compare_pair(&ins.src[0], ty)?;
        if mask != 0 {
            return Err("unsupported 64-bit integer compare modifiers".into());
        }
        let (b, bh) = self.compare_pair(&ins.src[1], ty)?;
        let (high_a, high_b) = if ty == "i64" {
            (format!("asint({ah})"), format!("asint({bh})"))
        } else {
            (ah.clone(), bh.clone())
        };
        Ok(match ins.op & 7 {
            0 => "false".into(),
            7 => "true".into(),
            2 => format!("({ah} == {bh} && {a} == {b})"),
            5 => format!("({ah} != {bh} || {a} != {b})"),
            pred => {
                let (high, low) = match pred {
                    1 => ("<", "<"),
                    3 => ("<", "<="),
                    4 => (">", ">"),
                    6 => (">", ">="),
                    _ => unreachable!(),
                };
                format!("({high_a} {high} {high_b} || ({ah} == {bh} && {a} {low} {b}))")
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::translate_valu_probe;

    #[test]
    fn wave32_comparisons_and_carry_leave_the_next_scalar_intact() {
        // v_cmp_gt_u32 s3,v0,v1 witnesses wave32. The following comparison
        // and add-with-carry use even destinations in that same program.
        let words = [0xd4c40003, 0x02020300, 0xd4c40022, 0x02020300, 0xd70f0200, 0x02020300];
        let narrow = translate_valu_probe(&words, 2).unwrap();
        for assignment in ["s4 = 0u;", "s35 = 0u;", "s3 = 0u;"] {
            assert!(!narrow.lines().any(|line| line.trim() == assignment));
        }
        // Even-only masks retain the existing high word when width is unknown.
        let wide = translate_valu_probe(&words[2..], 2).unwrap();
        assert!(wide.lines().any(|line| line.trim() == "s35 = 0u;"));
        assert!(wide.lines().any(|line| line.trim() == "s3 = 0u;"));
    }

    // VOPC v_cmp_class_f16 vcc, v0, v1 (op 0x8f) and v_cmpx_class_f16 (op 0x9f).
    const CMP_CLASS_F16: u32 = 0x7d1e_0300;
    const CMPX_CLASS_F16: u32 = 0x7d3e_0300;

    #[test]
    fn class_f16_classifies_at_half_precision_and_cmpx_writes_exec() {
        let cmp = translate_valu_probe(&[CMP_CLASS_F16], 2).unwrap();
        assert!(cmp.contains("tb_cmp_class(") && cmp.contains("10u, 31u)"), "{cmp}");
        let cmpx = translate_valu_probe(&[CMPX_CLASS_F16], 2).unwrap();
        assert!(cmpx.contains("10u, 31u)"), "{cmpx}");
        assert!(cmpx.lines().any(|l| l.trim_start().starts_with("exec_lo =")), "{cmpx}");
    }

    /// The emitted tb_cmp_class, transcribed, against the ISA's ten classes:
    /// 0 signaling NaN, 1 quiet NaN, 2 -inf, 3 -normal, 4 -denormal, 5 -0,
    /// 6 +0, 7 +denormal, 8 +normal, 9 +inf.
    #[test]
    fn every_class_bit_at_f16() {
        fn tb_cmp_class(b: u32, mask: u32, fraction_bits: u32, exponent_mask: u32) -> bool {
            let unit = 1u32 << fraction_bits;
            let e = (b >> fraction_bits) & exponent_mask;
            let m = b & (unit - 1);
            let neg = b & ((exponent_mask + 1) << fraction_bits) != 0;
            let c = if e == exponent_mask {
                if m != 0 { u32::from(m & (unit >> 1) != 0) } else if neg { 2 } else { 9 }
            } else if e == 0 {
                if m == 0 { if neg { 5 } else { 6 } } else if neg { 4 } else { 7 }
            } else if neg { 3 } else { 8 };
            (mask >> c) & 1 != 0
        }
        // One f16 value per class; the high half holds junk the class must ignore.
        let samples: [(u32, u32); 10] =
            [(0x7c01, 0), (0x7e00, 1), (0xfc00, 2), (0xbc00, 3), (0x8001, 4), (0x8000, 5), (0x0000, 6), (0x03ff, 7), (0x7bff, 8), (0x7c00, 9)];
        for (bits, class) in samples {
            for bit in 0..10 {
                assert_eq!(tb_cmp_class(0xabcd_0000 | bits, 1 << bit, 10, 31), bit == class, "{bits:#06x} bit {bit}");
            }
        }
    }
}
