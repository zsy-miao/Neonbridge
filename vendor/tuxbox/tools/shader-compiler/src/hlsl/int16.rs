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
    UnsignedMin3,
    UnsignedMax3,
    UnsignedMed3,
    SignedMin3,
    SignedMax3,
    SignedMed3,
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

pub(super) fn new_half(ins: &Insn) -> bool {
    ins.enc == Enc::Vop3 && matches!(ins.mnemonic.as_str(),
        "v_mul_lo_u16" | "v_max_u16" | "v_min_u16" | "v_add_nc_i16" | "v_sub_nc_i16"
        | "v_mad_u16" | "v_mad_i16" | "v_min3_u16" | "v_max3_u16" | "v_med3_u16"
        | "v_min3_i16" | "v_max3_i16" | "v_med3_i16")
}

pub(super) fn expression(ins: &Insn) -> Result<Option<String>, String> {
    if ins.enc != Enc::Vop3 {
        return Ok(None);
    }
    let op = match ins.mnemonic.as_str() {
        // TuxBox fae402f/34ba13a/63343eb: more packed and ordinary 16-bit forms
        // and the clamp bit. TB_PK_INT16_MORE=0 leaves them untranslated as before.
        _ if !more() && (NEW_PACKED.contains(&ins.mnemonic.as_str()) || new_half(ins)) => return Ok(None),
        // Without clamp the signed and unsigned forms have the same low 16 bits.
        "v_add_nc_u16" | "v_add_nc_i16" | "v_pk_add_u16" | "v_pk_add_i16" => Operation::Add,
        "v_sub_nc_u16" | "v_sub_nc_i16" | "v_pk_sub_i16" | "v_pk_sub_u16" => Operation::Subtract,
        "v_mul_lo_u16" | "v_pk_mul_lo_u16" => Operation::Multiply,
        "v_mad_u16" | "v_mad_i16" | "v_pk_mad_i16" | "v_pk_mad_u16" => Operation::MultiplyAdd,
        "v_max_u16" | "v_pk_max_u16" => Operation::UnsignedMax,
        "v_min_u16" | "v_pk_min_u16" => Operation::UnsignedMin,
        "v_lshlrev_b16" | "v_pk_lshlrev_b16" => Operation::ShiftLeft,
        "v_lshrrev_b16" | "v_pk_lshrrev_b16" => Operation::ShiftRight,
        "v_ashrrev_i16" | "v_pk_ashrrev_i16" => Operation::ArithmeticShiftRight,
        "v_max_i16" | "v_pk_max_i16" => Operation::SignedMax,
        "v_min_i16" | "v_pk_min_i16" => Operation::SignedMin,
        "v_min3_u16" => Operation::UnsignedMin3,
        "v_max3_u16" => Operation::UnsignedMax3,
        "v_med3_u16" => Operation::UnsignedMed3,
        "v_min3_i16" => Operation::SignedMin3,
        "v_max3_i16" => Operation::SignedMax3,
        "v_med3_i16" => Operation::SignedMed3,
        _ => return Ok(None),
    };
    let packed = ins.op >= 0x400;
    let new_packed = NEW_PACKED.contains(&ins.mnemonic.as_str());
    let arity = if matches!(op, Operation::MultiplyAdd | Operation::UnsignedMin3 | Operation::UnsignedMax3
        | Operation::UnsignedMed3 | Operation::SignedMin3 | Operation::SignedMax3 | Operation::SignedMed3) { 3 } else { 2 };
    if !ins.is_complete() || ins.raw.len() < 2 || ins.src.len() < arity {
        return Err("incomplete 16-bit integer instruction".into());
    }
    // New forms are initially limited to ordinary VGPR inputs. In particular
    // do not interpret an EXEC/VCC selector as packed integer data.
    if new_packed && (!packed || !ins.src[..arity].iter().all(|s| matches!(s, Opnd::Vgpr(_)))) {
        return Err("unsupported packed integer source or encoding".into());
    }
    if new_half(ins) && (packed || !ins.src[..arity].iter().all(|s| matches!(s, Opnd::Vgpr(_)))) {
        return Err("unsupported ordinary 16-bit integer source or encoding".into());
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
                    _ => unreachable!("signed clamp arithmetic"),
                };
                // Keep the saturated representation unsigned before packing. The
                // sign-biased ordering is exactly the signed ordering, but
                // avoids narrow signed intermediates in the host optimizer.
                return Ok(format!("(clamp((asuint({value}) ^ 0x80000000u), 0x7fff8000u, 0x80007fffu) ^ 0x80000000u)"));
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
                _ => unreachable!("unsigned clamp arithmetic"),
            };
            return Ok(format!("min({value}, 65535u)"));
        }
        Ok(match op {
            Operation::Add => format!("({a} + {b})"),
            Operation::Subtract => format!("({a} - {b})"),
            Operation::Multiply => format!("({a} * {b})"),
            // Without clamp, signed and unsigned multiply-add produce the
            // same low 16 bits. The uint expression is modular even if the
            // intermediate wraps at 32 bits.
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
            Operation::UnsignedMin3 | Operation::UnsignedMax3 | Operation::UnsignedMed3
            | Operation::SignedMin3 | Operation::SignedMax3 | Operation::SignedMed3 => {
                let c = half(&ins.src[2], (selectors >> 2) & 1)?;
                let signed_op = matches!(op, Operation::SignedMin3 | Operation::SignedMax3 | Operation::SignedMed3);
                let (a, b, c) = if signed_op { (signed(&a), signed(&b), signed(&c)) } else { (a, b, c) };
                let value = match op {
                    Operation::UnsignedMin3 | Operation::SignedMin3 => format!("min(min({a}, {b}), {c})"),
                    Operation::UnsignedMax3 | Operation::SignedMax3 => format!("max(max({a}, {b}), {c})"),
                    _ => format!("max(min({a}, {b}), min(max({a}, {b}), {c}))"),
                };
                if signed_op { format!("asuint({value})") } else { value }
            }
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
mod tests {
    use super::*;
    use crate::rdna2::disassemble;

    #[test]
    fn ordinary_half_forms_preserve_the_other_half() {
        for (first, mnemonic) in [
            (0xd705_4804u32, "v_mul_lo_u16"),
            (0xd709_4804u32, "v_max_u16"),
            (0xd70b_4804u32, "v_min_u16"),
        ] {
            let decode = |a: u32, b: u32| {
                let bytes: Vec<u8> = [a,b].iter().flat_map(|v| v.to_le_bytes()).collect();
                disassemble(&bytes).remove(0)
            };
            let ins=decode(first,0x0202_0b04);
            assert_eq!(ins.mnemonic,mnemonic);
            assert!(new_half(&ins));
            let expr=expression(&ins).unwrap().unwrap();
            assert!(expr.contains("v4 & 0xffffu"));
            let expr=expression(&decode(first & !0x4000,0x0202_0b04)).unwrap().unwrap();
            assert!(expr.contains("v4 & 0xffff0000u"));
            assert!(expression(&decode(first | 0x8000,0x0202_0b04)).is_err());
            assert!(expression(&decode(first,0x0202_0a04)).is_err());
        }
    }

    #[test]
    fn new_16_bit_forms_select_halves_and_preserve_the_other_half() {
        let decode = |a: u32, b: u32| {
            let bytes: Vec<u8> = [a, b].iter().flat_map(|v| v.to_le_bytes()).collect();
            disassemble(&bytes).remove(0)
        };
        // LLVM gfx1030 words for `v7, v4, v5[, v6]` without op_sel.
        for (first, second, name) in [
            (0xd70d_0007u32, 0x0202_0b04u32, "v_add_nc_i16"),
            (0xd70e_0007, 0x0202_0b04, "v_sub_nc_i16"),
            (0xd740_0007, 0x041a_0b04, "v_mad_u16"),
            (0xd75e_0007, 0x041a_0b04, "v_mad_i16"),
            (0xd752_0007, 0x041a_0b04, "v_min3_i16"),
            (0xd753_0007, 0x041a_0b04, "v_min3_u16"),
            (0xd755_0007, 0x041a_0b04, "v_max3_i16"),
            (0xd756_0007, 0x041a_0b04, "v_max3_u16"),
            (0xd758_0007, 0x041a_0b04, "v_med3_i16"),
            (0xd759_0007, 0x041a_0b04, "v_med3_u16"),
        ] {
            let ins = decode(first, second);
            assert_eq!(ins.mnemonic, name);
            assert!(new_half(&ins), "{name}");
            let low = expression(&ins).unwrap().unwrap();
            assert!(low.contains("v7 & 0xffff0000u") && low.contains("(v4 & 0xffffu)"), "{name}: {low}");
            // op_sel source 0 high and destination high.
            let high = expression(&decode(first | 0x4800, second)).unwrap().unwrap();
            assert!(high.contains("v7 & 0xffffu") && high.contains("(v4 >> 16)"), "{name}: {high}");
            if name.ends_with("_i16") && !name.contains("_nc_") && !name.starts_with("v_mad") {
                assert!(low.contains("asint("), "{name} compares signed");
            }
            assert!(expression(&decode(first | 0x8000, second)).is_err(), "{name} clamp");
            assert!(expression(&decode(first, (second & !511) | 4)).is_err(), "{name} sgpr");
            assert!(expression(&decode(first | 0x100, second)).is_err(), "{name} abs");
        }
        // Source 2 selection for three-operand forms.
        let med = expression(&decode(0xd759_2007, 0x041a_0b04)).unwrap().unwrap();
        assert!(med.contains("(v6 >> 16)"));
    }

    #[test]
    fn new_packed_integer_forms_accept_ordinary_sources_only() {
        // LLVM gfx1030 native words. The default high selectors include
        // source 2 via dword0 bit 14 for the three-operand forms.
        for (first, second, name) in [
            (0xcc02_4004, 0x1a02_0b04, "v_pk_add_i16"),
            (0xcc00_2804, 0x141a_0b04, "v_pk_mad_i16"),
            (0xcc09_4004, 0x1c1a_0b04, "v_pk_mad_u16"),
            (0xcc0c_4804, 0x1202_0b04, "v_pk_max_u16"),
            (0xcc0d_4004, 0x1a02_0b04, "v_pk_min_u16"),
        ] {
            let native = |a: u32, b: u32| {
                let bytes: Vec<u8> = [a,b].iter().flat_map(|v| v.to_le_bytes()).collect();
                disassemble(&bytes).remove(0)
            };
            let ins = native(first,second);
            assert_eq!(ins.mnemonic, name);
            assert!(expression(&ins).unwrap().is_some());
            if matches!(name, "v_pk_max_u16" | "v_pk_min_u16") {
                assert!(expression(&native(first | 0x8000, second)).is_err());
            } else {
                let clamped = expression(&native(first | 0x8000, second)).unwrap().unwrap();
                if name.ends_with("_i16") {
                    assert!(clamped.contains("0x7fff8000u, 0x80007fffu"));
                    assert!(clamped.contains("^ 0x80000000u"));
                }
            }
            assert!(expression(&native(first | 0x8000, (second & !511) | 4)).is_err());
            assert!(expression(&native(first, (second & !511) | 4)).is_err());
            assert!(expression(&native(first | 0x100, second)).is_err());
        }
        let bytes: Vec<u8> = [0xcc09_4004u32,0x1c1a_0b04].iter().flat_map(|v| v.to_le_bytes()).collect();
        let expr = expression(&disassemble(&bytes)[0]).unwrap().unwrap();
        assert!(expr.contains("(v6 & 0xffffu)"));
        assert!(expr.contains("(v6 >> 16)"));
    }
}
