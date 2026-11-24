//! VOP3 operates on one selected half of a register. VOP3P computes both
//! halves independently, with separate source selectors for each result.
use super::{r, Enc, Insn, Opnd};

enum Operation {
    Add,
    Subtract,
    Multiply,
    MultiplyAdd,
    UnsignedMax,
    UnsignedMin,
    ShiftLeft,
    ShiftRight,
    ArithmeticShiftRight,
    SignedMax,
    SignedMin,
}

const NEW_PACKED: [&str; 5] = ["v_pk_add_i16", "v_pk_mad_i16", "v_pk_mad_u16", "v_pk_max_u16", "v_pk_min_u16"];

fn more() -> bool {
    std::env::var("TB_PK_INT16_MORE").map_or(true, |v| v.trim() != "0")
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
        // TuxBox fae402f/34ba13a: more packed forms and the clamp bit.
        // TB_PK_INT16_MORE=0 leaves them untranslated as before.
        name if !more() && NEW_PACKED.contains(&name) => return Ok(None),
        "v_add_nc_u16" | "v_pk_add_u16" | "v_pk_add_i16" => Operation::Add,
        "v_sub_nc_u16" | "v_pk_sub_i16" | "v_pk_sub_u16" => Operation::Subtract,
        "v_pk_mul_lo_u16" => Operation::Multiply,
        "v_pk_mad_i16" | "v_pk_mad_u16" => Operation::MultiplyAdd,
        "v_pk_max_u16" => Operation::UnsignedMax,
        "v_pk_min_u16" => Operation::UnsignedMin,
        "v_lshlrev_b16" | "v_pk_lshlrev_b16" => Operation::ShiftLeft,
        "v_lshrrev_b16" | "v_pk_lshrrev_b16" => Operation::ShiftRight,
        "v_ashrrev_i16" | "v_pk_ashrrev_i16" => Operation::ArithmeticShiftRight,
        "v_max_i16" | "v_pk_max_i16" => Operation::SignedMax,
        "v_min_i16" | "v_pk_min_i16" => Operation::SignedMin,
        _ => return Ok(None),
    };
    let packed = ins.op >= 0x400;
    let new_packed = NEW_PACKED.contains(&ins.mnemonic.as_str());
    let arity = if matches!(op, Operation::MultiplyAdd) { 3 } else { 2 };
    if !ins.is_complete() || ins.raw.len() < 2 || ins.src.len() < arity {
        return Err("incomplete 16-bit integer instruction".into());
    }
    // New forms are limited to ordinary VGPR inputs: an EXEC/VCC selector is
    // not read as packed integer data.
    if new_packed && (!packed || !ins.src[..arity].iter().all(|s| matches!(s, Opnd::Vgpr(_)))) {
        return Err("unsupported packed integer source or encoding".into());
    }
    let (w0, w1) = (ins.raw[0], ins.raw[1]);
    // RDNA2 ISA sections 6.5 and 12.10: clamp arithmetic to the
    // representable signed/unsigned 16-bit interval, independently per half.
    let clamped = w0 & 0x8000 != 0 && more();
    let saturating_arithmetic = packed && matches!(op,
        Operation::Add | Operation::Subtract | Operation::Multiply | Operation::MultiplyAdd);
    if clamped && (!saturating_arithmetic
        || !ins.src[..arity].iter().all(|s| matches!(s, Opnd::Vgpr(_)))) {
        return Err("unsupported packed integer clamp form or source".into());
    }
    let modifiers = if more() { 0x0700 } else { 0x8700 };
    // A packed integer op reads an inline float constant as its 32-bit pattern
    // (1.0 is 0x3f800000), and op_sel / op_sel_hi pick halves of that, as `r`
    // already spells it (VIctory a0efd2d). TB_PK_INLINE_STRICT=1 refuses them;
    // the non-packed 16-bit forms still do.
    let float_inline_ok = packed && std::env::var("TB_PK_INLINE_STRICT").map_or(true, |v| v.trim() != "1");
    if w0 & modifiers != 0
        || w1 >> (if packed { 29 } else { 27 }) != 0
        || (!float_inline_ok && ins.src[..arity].iter().any(|s| matches!(s, Opnd::ConstF32(_))))
    {
        return Err("unsupported 16-bit integer modifiers or inline float".into());
    }
    let result = |selectors: u32| -> Result<String, String> {
        let a = half(&ins.src[0], selectors & 1)?;
        let b = half(&ins.src[1], (selectors >> 1) & 1)?;
        let signed = |x: &str| format!("(asint({x} << 16) >> 16)");
        if clamped {
            let signed_op = matches!(ins.mnemonic.as_str(),
                "v_pk_add_i16" | "v_pk_sub_i16" | "v_pk_mad_i16");
            if signed_op {
                let (a, b) = (signed(&a), signed(&b));
                let value = match op {
                    Operation::Add => format!("({a} + {b})"),
                    Operation::Subtract => format!("({a} - {b})"),
                    Operation::MultiplyAdd => {
                        let c = signed(&half(&ins.src[2], (selectors >> 2) & 1)?);
                        format!("({a} * {b} + {c})")
                    }
                    _ => return Err("unsupported signed packed integer clamp".into()),
                };
                return Ok(format!("asuint(clamp({value}, -32768, 32767))"));
            }
            // 65535*65535+65535 fits in uint32, and unsigned subtract
            // must clamp at zero before any modular underflow.
            let value = match op {
                Operation::Add => format!("({a} + {b})"),
                Operation::Subtract => format!("(max({a}, {b}) - {b})"),
                Operation::Multiply => format!("({a} * {b})"),
                Operation::MultiplyAdd => {
                    let c = half(&ins.src[2], (selectors >> 2) & 1)?;
                    format!("({a} * {b} + {c})")
                }
                _ => return Err("unsupported unsigned packed integer clamp".into()),
            };
            return Ok(format!("min({value}, 65535u)"));
        }
        Ok(match op {
            Operation::Add => format!("({a} + {b})"),
            Operation::Subtract => format!("({a} - {b})"),
            Operation::Multiply => format!("({a} * {b})"),
            // Without clamp, signed and unsigned multiply-add produce the
            // same low 16 bits; the uint expression is modular.
            Operation::MultiplyAdd => {
                let c = half(&ins.src[2], (selectors >> 2) & 1)?;
                format!("({a} * {b} + {c})")
            }
            Operation::UnsignedMax => format!("max({a}, {b})"),
            Operation::UnsignedMin => format!("min({a}, {b})"),
            Operation::ShiftLeft => format!("({b} << ({a} & 15u))"),
            Operation::ShiftRight => format!("({b} >> ({a} & 15u))"),
            Operation::ArithmeticShiftRight => format!("asuint({} >> ({a} & 15u))", signed(&b)),
            Operation::SignedMax => format!("asuint(max({}, {}))", signed(&a), signed(&b)),
            Operation::SignedMin => format!("asuint(min({}, {}))", signed(&a), signed(&b)),
        })
    };
    let lo = result(w0 >> 11)?;
    Ok(Some(if packed {
        // op_sel_hi of source 2 is dword 0 bit 14.
        let hi = result(((w1 >> 27) & 3) | (((w0 >> 14) & 1) << 2))?;
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

#[cfg(test)]
mod packed_forms {
    use super::*;
    use crate::rdna2::disassemble;

    // From TuxBox fae402f/34ba13a (LLVM gfx1030 native words).
    #[test]
    fn new_packed_integer_forms_accept_ordinary_sources_only() {
        for (first, second, name) in [
            (0xcc02_4004, 0x1a02_0b04, "v_pk_add_i16"),
            (0xcc00_2804, 0x141a_0b04, "v_pk_mad_i16"),
            (0xcc09_4004, 0x1c1a_0b04, "v_pk_mad_u16"),
            (0xcc0c_4804, 0x1202_0b04, "v_pk_max_u16"),
            (0xcc0d_4004, 0x1a02_0b04, "v_pk_min_u16"),
        ] {
            let native = |a: u32, b: u32| {
                let bytes: Vec<u8> = [a, b].iter().flat_map(|v| v.to_le_bytes()).collect();
                disassemble(&bytes).remove(0)
            };
            let ins = native(first, second);
            assert_eq!(ins.mnemonic, name);
            assert!(expression(&ins).unwrap().is_some());
            if matches!(name, "v_pk_max_u16" | "v_pk_min_u16") {
                assert!(expression(&native(first | 0x8000, second)).is_err());
            } else {
                assert!(expression(&native(first | 0x8000, second)).unwrap().is_some());
            }
            assert!(expression(&native(first | 0x8000, (second & !511) | 4)).is_err());
            assert!(expression(&native(first, (second & !511) | 4)).is_err());
            assert!(expression(&native(first | 0x100, second)).is_err());
        }
        let bytes: Vec<u8> = [0xcc09_4004u32, 0x1c1a_0b04].iter().flat_map(|v| v.to_le_bytes()).collect();
        let expr = expression(&disassemble(&bytes)[0]).unwrap().unwrap();
        assert!(expr.contains("(v6 & 0xffffu)"));
        assert!(expr.contains("(v6 >> 16)"));
    }
}
