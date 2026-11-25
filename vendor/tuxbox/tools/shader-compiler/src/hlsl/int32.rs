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
#[path = "../tests/hlsl/int32.rs"]
mod tests;
