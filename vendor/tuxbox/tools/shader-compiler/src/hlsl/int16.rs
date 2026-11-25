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
    let new_packed = matches!(ins.mnemonic.as_str(),
        "v_pk_add_i16" | "v_pk_mad_i16" | "v_pk_mad_u16" | "v_pk_max_u16" | "v_pk_min_u16");
    let arity = if matches!(op, Operation::MultiplyAdd | Operation::UnsignedMin3 | Operation::UnsignedMax3
        | Operation::UnsignedMed3 | Operation::SignedMin3 | Operation::SignedMax3 | Operation::SignedMed3) { 3 } else { 2 };
    if !ins.is_complete() || ins.raw.len() < 2 || ins.src.len() < arity {
        return Err("incomplete 16-bit integer instruction".into());
    }
    // Do not interpret an EXEC/VCC selector as packed integer data.
    if new_packed && (!packed || !ins.src[..arity].iter().all(|s| matches!(s, Opnd::Vgpr(_)))) {
        return Err("unsupported packed integer source or encoding".into());
    }
    let (w0, w1) = (ins.raw[0], ins.raw[1]);
    let allow_immediates = matches!(ins.mnemonic.as_str(), "v_add_nc_i16" | "v_sub_nc_i16");
    // gfx1030 accepts inline integers for ordinary multiply without op_sel.
    let inline_mul = ins.mnemonic == "v_mul_lo_u16" && w0 & 0x7800 == 0;
    if new_half(ins) && (packed || !ins.src[..arity].iter().enumerate().all(|(i, src)| {
        matches!(src, Opnd::Vgpr(_))
            || (inline_mul && matches!(src, Opnd::ConstInt(_)))
            || (allow_immediates
                && matches!(src, Opnd::ConstInt(_) | Opnd::Literal(_))
                && w0 & (1 << (11 + i)) == 0)
    })) {
        return Err("unsupported ordinary 16-bit integer source or encoding".into());
    }
    // RDNA2 ISA sections 6.5 and 12.10: clamp arithmetic to the
    // representable signed/unsigned 16-bit interval, independently per half.
    let clamped = w0 & 0x8000 != 0;
    let saturating_arithmetic = packed && matches!(op,
        Operation::Add | Operation::Subtract | Operation::Multiply | Operation::MultiplyAdd);
    if clamped && (!saturating_arithmetic
        || !ins.src[..arity].iter().all(|s| matches!(s, Opnd::Vgpr(_)))) {
        return Err("unsupported packed integer clamp form or source".into());
    }
    if w0 & 0x0700 != 0
        || w1 >> (if packed { 29 } else { 27 }) != 0
        || ins.src[..arity].iter().any(|s| matches!(s, Opnd::ConstF32(_)))
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

#[cfg(test)]
#[path = "../tests/hlsl/int16.rs"]
mod tests;
