//! Ordinary VOP3 integer forms with a 32-bit result: the signed no-carry
//! add/subtract, 16x16+32 multiply-add, saturating integer packs and sums of
//! absolute differences over bytes, halves and words. Add/subtract and unsigned MAD
//! accept integer immediates, and byte SAD accepts an inline zero accumulator.
//! Signed add/subtract also support saturating arithmetic with clamp.
use super::{int16::half, r, signed_add_overflow, Enc, Insn, Opnd};

pub(super) fn handles(ins: &Insn) -> bool {
    ins.enc == Enc::Vop3 && matches!(ins.mnemonic.as_str(),
        "v_add_nc_i32" | "v_sub_nc_i32" | "v_mad_u32_u16" | "v_mad_i32_i16"
        | "v_cvt_pk_u16_u32" | "v_cvt_pk_i16_i32"
        | "v_sad_u8" | "v_sad_hi_u8" | "v_sad_u16" | "v_sad_u32")
}

pub(super) fn expression(ins: &Insn) -> Result<Option<String>, String> {
    if !handles(ins) {
        return Ok(None);
    }
    let mad = ins.mnemonic.starts_with("v_mad_");
    let add_sub = matches!(ins.mnemonic.as_str(), "v_add_nc_i32" | "v_sub_nc_i32");
    let pack = matches!(ins.mnemonic.as_str(), "v_cvt_pk_u16_u32" | "v_cvt_pk_i16_i32");
    let arity = if add_sub || pack { 2 } else { 3 };
    if !ins.is_complete() || ins.raw.len() < 2 || ins.src.len() < arity
        || !ins.src[..arity].iter().enumerate().all(|(index, s)| {
            matches!(s, Opnd::Vgpr(_))
                || (add_sub && matches!(s, Opnd::ConstInt(_) | Opnd::Literal(_)))
                || (index == 2 && ins.mnemonic == "v_sad_u8" && matches!(s, Opnd::ConstInt(0)))
                || (index == 2 && ins.mnemonic == "v_mad_u32_u16" && matches!(s, Opnd::ConstInt(_)))
        }) {
        return Err("unsupported ordinary 32-bit integer source or encoding".into());
    }
    let (w0, w1) = (ins.raw[0], ins.raw[1]);
    // RDNA2 ISA 6.4: op_sel bits 11 and 12 pick the 16-bit half of the two
    // multiplicands. The 32-bit addend and result take no half selection.
    let allowed_select = if mad { 0x1800 } else { 0 };
    let clamped = w0 & 0x8000 != 0;
    if (clamped && !add_sub) || w0 & 0x0700 != 0 || w0 & 0x7800 & !allowed_select != 0 || w1 >> 27 != 0 {
        return Err("unsupported ordinary 32-bit integer modifiers".into());
    }
    let (a, b) = (r(&ins.src[0])?, r(&ins.src[1])?);
    if clamped {
        // Use uint arithmetic so even overflowing inputs have a defined result.
        let (result, overflow) = if ins.mnemonic == "v_add_nc_i32" {
            let sum = format!("({a} + {b})");
            let overflow = signed_add_overflow(&a, &b, &sum);
            (sum, overflow)
        } else {
            let diff = format!("({a} - {b})");
            let overflow = format!("((({a} ^ {b}) & ({a} ^ {diff})) >> 31)");
            (diff, overflow)
        };
        // On signed overflow the first operand's sign determines which limit to use.
        return Ok(Some(format!("({overflow} != 0u ? (0x7fffffffu + ({a} >> 31)) : {result})")));
    }
    // |x - y| of unsigned fields, then the 32-bit accumulator S2 (modular).
    let sad = |width: u32| -> String {
        let mask = (1u64 << width) - 1;
        (0..32 / width).map(|i| {
            let x = format!("(({a} >> {}u) & {mask:#x}u)", i * width);
            let y = format!("(({b} >> {}u) & {mask:#x}u)", i * width);
            format!("(max({x}, {y}) - min({x}, {y}))")
        }).collect::<Vec<_>>().join(" + ")
    };
    Ok(Some(match ins.mnemonic.as_str() {
        // Convert both full-width inputs with saturation; S0 occupies the low half.
        "v_cvt_pk_u16_u32" => format!("(min({a}, 65535u) | (min({b}, 65535u) << 16))"),
        "v_cvt_pk_i16_i32" => format!(
            "((asuint(clamp(asint({a}), -32768, 32767)) & 0xffffu) | (asuint(clamp(asint({b}), -32768, 32767)) << 16))"
        ),
        // RDNA2 ISA: SAD_U8 sums four byte differences, SAD_HI_U8 places
        // that sum in the high half, SAD_U16 sums two half differences.
        "v_sad_u8" => format!("({} + {})", sad(8), r(&ins.src[2])?),
        "v_sad_hi_u8" => format!("((({}) << 16) + {})", sad(8), r(&ins.src[2])?),
        "v_sad_u16" => format!("({} + {})", sad(16), r(&ins.src[2])?),
        "v_sad_u32" => format!("((max({a}, {b}) - min({a}, {b})) + {})", r(&ins.src[2])?),
        // Without clamp these wrap exactly like the unsigned forms.
        "v_add_nc_i32" => format!("({a} + {b})"),
        "v_sub_nc_i32" => format!("({a} - {b})"),
        _ => {
            let x = half(&ins.src[0], (w0 >> 11) & 1)?;
            let y = half(&ins.src[1], (w0 >> 12) & 1)?;
            let c = r(&ins.src[2])?;
            if ins.mnemonic == "v_mad_u32_u16" {
                format!("({x} * {y} + {c})")
            } else {
                let s = |v: &str| format!("(asint({v} << 16) >> 16)");
                format!("asuint({} * {} + asint({c}))", s(&x), s(&y))
            }
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rdna2::disassemble;

    fn native(a: u32, b: u32) -> Insn {
        let bytes: Vec<u8> = [a, b].iter().flat_map(|v| v.to_le_bytes()).collect();
        disassemble(&bytes).remove(0)
    }

    #[test]
    fn ordinary_32_bit_forms_accept_vgprs_without_modifiers() {
        // LLVM gfx1030 words for `v7, v4, v5[, v6]`.
        for (first, second, name, want) in [
            (0xd77f_0007u32, 0x0202_0b04u32, "v_add_nc_i32", "(v4 + v5)"),
            (0xd776_0007, 0x0202_0b04, "v_sub_nc_i32", "(v4 - v5)"),
            (0xd773_0007, 0x041a_0b04, "v_mad_u32_u16", "((v4 & 0xffffu) * (v5 & 0xffffu) + v6)"),
            (0xd775_0007, 0x041a_0b04, "v_mad_i32_i16", "asint(v6)"),
        ] {
            let ins = native(first, second);
            assert_eq!(ins.mnemonic, name);
            assert!(expression(&ins).unwrap().unwrap().contains(want), "{name}");
            if name.starts_with("v_mad_") {
                assert!(expression(&native(first | 0x8000, second)).is_err(), "{name} clamp");
            }
            assert!(expression(&native(first | 0x100, second)).is_err(), "{name} abs");
            assert!(expression(&native(first, (second & !511) | 4)).is_err(), "{name} sgpr");
            assert!(expression(&native(first, second | 1 << 29)).is_err(), "{name} neg");
            // Destination or addend half selection has no meaning here.
            assert!(expression(&native(first | 0x4000, second)).is_err(), "{name} dst sel");
        }
        // op_sel:[1,1,0,0] selects the high multiplicand halves.
        let expr = expression(&native(0xd773_1807, 0x041a_0b04)).unwrap().unwrap();
        assert_eq!(expr, "((v4 >> 16) * (v5 >> 16) + v6)");
        assert!(expression(&native(0xd77f_0807, 0x0202_0b04)).is_err());
    }

    #[test]
    fn unsigned_mad_accepts_a_full_width_inline_integer_accumulator() {
        // LLVM gfx1030: v_mad_u32_u16 v4, v1, v2, 7 (then -1).
        for (sources, addend) in [(0x021e_0501, "0x7u"), (0x0306_0501, "0xffffffffu")] {
            let expr = expression(&native(0xd773_0004, sources)).unwrap().unwrap();
            assert_eq!(expr, format!("((v1 & 0xffffu) * (v2 & 0xffffu) + {addend})"));
        }
        // v_mad_u32_u16 v4, 7, v2, v3 and v_mad_i32_i16 v4, v1, v2, 7
        assert!(expression(&native(0xd773_0004, 0x040e_0487)).is_err());
        assert!(expression(&native(0xd775_0004, 0x021e_0501)).is_err());
    }

    #[test]
    fn integer_packs_clamp_each_word_before_combining_halves() {
        use crate::hlsl::float64::tests::translate;
        use crate::Stage;

        // LLVM gfx1030: v_cvt_pk_{u16_u32,i16_i32} v4, v1, v2
        for (opcode, low, high) in [
            (0xd76a_0004, "min(v1, 65535u)", "min(v2, 65535u)"),
            (0xd76b_0004, "(asuint(clamp(asint(v1), -32768, 32767)) & 0xffffu)",
                          "asuint(clamp(asint(v2), -32768, 32767))"),
        ] {
            let body = [opcode, 0x0002_0501];
            let hlsl = translate(Stage::Compute, &body).unwrap();
            assert!(hlsl.contains(&format!("({low} | ({high} << 16))")), "{hlsl}");
            assert!(translate(Stage::Compute, &[opcode | 0x0800, body[1]]).is_err());
            assert!(translate(Stage::Compute, &[opcode, body[1] | (1 << 29)]).is_err());
        }
    }

    #[test]
    fn signed_add_and_subtract_accept_clamp() {
        use crate::hlsl::float64::tests::translate;
        use crate::Stage;

        // LLVM gfx1030: v_add_nc_i32 / v_sub_nc_i32 v4, v1, v2 clamp
        for (opcode, result) in [(0xd77f_8004, "(v1 + v2)"), (0xd776_8004, "(v1 - v2)")] {
            let hlsl = translate(Stage::Compute, &[opcode, 0x0002_0501]).unwrap();
            assert!(hlsl.contains(&format!("? (0x7fffffffu + (v1 >> 31)) : {result}")), "{hlsl}");
            assert!(translate(Stage::Compute, &[opcode | 0x0100, 0x0002_0501]).is_err());
        }
    }

    #[test]
    fn signed_add_and_subtract_accept_immediate_operands() {
        use crate::hlsl::float64::tests::translate;
        use crate::Stage;

        // LLVM gfx1030: v_add_nc_i32 v7, v4, -1
        //              v_sub_nc_i32 v8, 0x80000000, v7
        let hlsl = translate(
            Stage::Compute,
            &[0xd77f_0007, 0x0001_8304, 0xd776_0008, 0x0002_0eff, 0x8000_0000],
        )
        .unwrap();
        assert!(hlsl.contains("v4 + 0xffffffffu"), "{hlsl}");
        assert!(hlsl.contains("0x80000000u - v7"), "{hlsl}");
    }

    #[test]
    fn byte_sad_accepts_inline_zero_only_as_the_accumulator() {
        // LLVM gfx1030: v_sad_u8 v4, v1, v2, 0
        let expr = expression(&native(0xd55a_0004, 0x0202_0501)).unwrap().unwrap();
        assert_eq!(expr.matches("max(").count(), 4);
        assert!(expr.ends_with("+ 0x0u)"), "{expr}");

        for (opcode, sources) in [
            (0xd55a_0004, 0x0206_0501), // v_sad_u8 v4, v1, v2, 1
            (0xd55a_0004, 0x040e_0480), // v_sad_u8 v4, 0, v2, v3
            (0xd55b_0004, 0x0202_0501), // v_sad_hi_u8 v4, v1, v2, 0
        ] {
            assert!(expression(&native(opcode, sources)).is_err());
        }
    }

    #[test]
    fn sums_of_absolute_differences_accumulate_into_source_2() {
        // LLVM gfx1030 words for `v7, v4, v5, v6`.
        for (first, name, fields) in [
            (0xd55a_0007u32, "v_sad_u8", 4), (0xd55b_0007, "v_sad_hi_u8", 4),
            (0xd55c_0007, "v_sad_u16", 2), (0xd55d_0007, "v_sad_u32", 1),
        ] {
            let ins = native(first, 0x041a_0b04);
            assert_eq!(ins.mnemonic, name);
            let expr = expression(&ins).unwrap().unwrap();
            assert_eq!(expr.matches("max(").count(), fields, "{name}: {expr}");
            assert!(expr.ends_with("+ v6)"), "{name}: {expr}");
            assert_eq!(name == "v_sad_hi_u8", expr.contains("<< 16"), "{name}");
            assert!(expression(&native(first | 0x8000, 0x041a_0b04)).is_err(), "{name} clamp");
            assert!(expression(&native(first | 0x0800, 0x041a_0b04)).is_err(), "{name} op_sel");
            assert!(expression(&native(first, 0x041a_0b04 & !511 | 4)).is_err(), "{name} sgpr");
        }
        // Masked and quad SADs remain refused.
        assert!(!handles(&native(0xd571_0007, 0x041a_0b04)));
    }

    #[test]
    fn signed_64_bit_multiply_add_needs_a_null_carry() {
        use crate::hlsl::float64::tests::translate;
        use crate::Stage;
        // v_mad_i64_i32 v[10:11], null, v4, v5, v[8:9]
        let hlsl = translate(Stage::Compute, &[0xd577_7d0a, 0x0422_0b04]).unwrap();
        assert!(hlsl.contains("tb_mulhi_i32(v4, v5) + v9"), "{hlsl}");
        assert!(hlsl.contains("v10 = t") && hlsl.contains("v11 = t"), "{hlsl}");
        // A real carry destination (vcc_lo) is not modelled for the signed form.
        assert!(translate(Stage::Compute, &[0xd577_6a0a, 0x0422_0b04]).is_err());
        // The unsigned form now also accepts the null carry LLVM emits.
        let hlsl = translate(Stage::Compute, &[0xd576_7d0a, 0x0422_0b04]).unwrap();
        assert!(hlsl.contains("tb_mulhi_u32(v4, v5) + v9"), "{hlsl}");
    }
}
