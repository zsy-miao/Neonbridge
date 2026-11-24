//! VOP3 operates on one selected half of a register. VOP3P computes both
//! halves independently, with separate source selectors for each result.
use super::{r, Enc, Insn, Opnd};

enum Operation {
    Add,
    Subtract,
    Multiply,
    ShiftLeft,
    ShiftRight,
    ArithmeticShiftRight,
    SignedMax,
    SignedMin,
}

pub(super) fn half(src: &Opnd, select: u32) -> Result<String, String> {
    let value = r(src)?;
    Ok(if select == 0 {
        format!("({value} & 0xffffu)")
    } else {
        format!("({value} >> 16)")
    })
}

pub(super) fn expression(ins: &Insn) -> Result<Option<String>, String> {
    if ins.enc != Enc::Vop3 {
        return Ok(None);
    }
    let op = match ins.mnemonic.as_str() {
        "v_add_nc_u16" | "v_pk_add_u16" => Operation::Add,
        "v_sub_nc_u16" | "v_pk_sub_i16" | "v_pk_sub_u16" => Operation::Subtract,
        "v_pk_mul_lo_u16" => Operation::Multiply,
        "v_lshlrev_b16" | "v_pk_lshlrev_b16" => Operation::ShiftLeft,
        "v_lshrrev_b16" | "v_pk_lshrrev_b16" => Operation::ShiftRight,
        "v_ashrrev_i16" | "v_pk_ashrrev_i16" => Operation::ArithmeticShiftRight,
        "v_max_i16" | "v_pk_max_i16" => Operation::SignedMax,
        "v_min_i16" | "v_pk_min_i16" => Operation::SignedMin,
        _ => return Ok(None),
    };
    let packed = ins.op >= 0x400;
    let (w0, w1) = (ins.raw[0], ins.raw[1]);
    if w0 & 0x8700 != 0
        || w1 >> (if packed { 29 } else { 27 }) != 0
        || ins.src[..2].iter().any(|s| matches!(s, Opnd::ConstF32(_)))
    {
        return Err("unsupported 16-bit integer modifiers or inline float".into());
    }
    let result = |selectors: u32| -> Result<String, String> {
        let a = half(&ins.src[0], selectors & 1)?;
        let b = half(&ins.src[1], (selectors >> 1) & 1)?;
        let signed = |x: &str| format!("(asint({x} << 16) >> 16)");
        Ok(match op {
            Operation::Add => format!("({a} + {b})"),
            Operation::Subtract => format!("({a} - {b})"),
            Operation::Multiply => format!("({a} * {b})"),
            Operation::ShiftLeft => format!("({b} << ({a} & 15u))"),
            Operation::ShiftRight => format!("({b} >> ({a} & 15u))"),
            Operation::ArithmeticShiftRight => format!("asuint({} >> ({a} & 15u))", signed(&b)),
            Operation::SignedMax => format!("asuint(max({}, {}))", signed(&a), signed(&b)),
            Operation::SignedMin => format!("asuint(min({}, {}))", signed(&a), signed(&b)),
        })
    };
    let lo = result(w0 >> 11)?;
    Ok(Some(if packed {
        let hi = result(w1 >> 27)?;
        format!("(({lo} & 0xffffu) | ({hi} << 16))")
    } else {
        let dst = r(&ins.dst[0])?;
        if (w0 & (1 << 14)) != 0 {
            format!("(({dst} & 0xffffu) | ({lo} << 16))")
        } else {
            format!("(({dst} & 0xffff0000u) | ({lo} & 0xffffu))")
        }
    }))
}

/// binary32 -> binary16 bits, round to nearest even, for constants folded at
/// translation time (HLSL f32tof16's rounding is the driver's choice). A
/// rounding carry out of the mantissa is added into the exponent, so
/// 2 - 2^-12 gives 2.0 and the largest finite values round to infinity.
pub(super) fn f16_rne_bits(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let a = bits & 0x7fff_ffff;
    if a > 0x7f80_0000 {
        return sign | 0x7e00 | ((a >> 13) & 0x3ff) as u16;
    }
    if a >= 0x4780_0000 {
        return sign | 0x7c00; // 65536 and up, infinity
    }
    if a >= 0x3880_0000 {
        // Normal: rebias the exponent, then add the rounding increment.
        let r = (a >> 13) - 0x1c000;
        let rem = a & 0x1fff;
        let up = rem > 0x1000 || (rem == 0x1000 && r & 1 != 0);
        return sign | (r + u32::from(up)).min(0x7c00) as u16;
    }
    if a < 0x3300_0000 {
        return sign; // below half the smallest subnormal
    }
    let e = a >> 23;
    let mant = (a & 0x7f_ffff) | 0x80_0000;
    let shift = 126 - e;
    let q = mant >> shift;
    let rem = mant & ((1 << shift) - 1);
    let half = 1 << (shift - 1);
    let up = rem > half || (rem == half && q & 1 != 0);
    sign | (q + u32::from(up)) as u16
}

#[cfg(test)]
mod half_rounding {
    use super::f16_rne_bits;

    #[test]
    fn carry_reaches_the_exponent() {
        assert_eq!(f16_rne_bits(2.0 - 2f32.powi(-12)), 0x4000);
        assert_eq!(f16_rne_bits(1.0 - 2f32.powi(-12)), 0x3c00);
        // The largest subnormal rounds up into the smallest normal.
        assert_eq!(f16_rne_bits(f32::from_bits(0x387f_f000)), 0x0400);
        assert_eq!(f16_rne_bits(65519.0), 0x7bff);
        assert_eq!(f16_rne_bits(65520.0), 0x7c00);
    }

    #[test]
    fn exact_ties_and_specials() {
        assert_eq!(f16_rne_bits(1.0), 0x3c00);
        assert_eq!(f16_rne_bits(-2.0), 0xc000);
        assert_eq!(f16_rne_bits(0.5), 0x3800);
        assert_eq!(f16_rne_bits(1.0 / (2.0 * std::f32::consts::PI)), 0x3118);
        assert_eq!(f16_rne_bits(2f32.powi(-24)), 0x0001);
        assert_eq!(f16_rne_bits(2f32.powi(-25)), 0x0000);
        assert_eq!(f16_rne_bits(3.0 * 2f32.powi(-25)), 0x0002);
        assert_eq!(f16_rne_bits(1.0 + 2f32.powi(-11)), 0x3c00);
        assert_eq!(f16_rne_bits(1.0 + 3.0 * 2f32.powi(-11)), 0x3c02);
        assert_eq!(f16_rne_bits(f32::INFINITY), 0x7c00);
        assert_eq!(f16_rne_bits(-0.0), 0x8000);
        assert_eq!(f16_rne_bits(f32::NAN) & 0x7e00, 0x7e00);
    }
}
