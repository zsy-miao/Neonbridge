//! Register names, operand expressions and signature lookups.
use super::*;

pub(in crate::hlsl) fn clamped_product24(ins: &Insn) -> bool {
    matches!(ins.mnemonic.as_str(), "v_mul_u32_u24" | "v_mul_i32_i24" | "v_mad_u32_u24" | "v_mad_i32_i24")
        && (ins.aux.imm != 0 || ins.aux.sdwa.is_some_and(|ext| ext & (1 << 13) != 0))
}

/// Prove the non-returning 2D, DMASK=3 UMAX encoding and return its X/Y VGPRs.
/// NSA accepts one extension word with zero unused bytes; A16/D16 stay rejected.
pub(in crate::hlsl) fn wide_atomic_coordinates(ins: &Insn) -> Result<[u8; 2], String> {
    let nsa = match ins.raw.as_slice() {
        [0xf05c1308, flags] if flags & 0xc000_0000 == 0 => false,
        [0xf05c130a, flags, coordinates]
            if flags & 0xc000_0000 == 0 && coordinates & 0xffff_ff00 == 0 => true,
        _ => return Err("unsupported 64-bit image access flags".into()),
    };
    if ins.enc != Enc::Mimg || ins.mnemonic != "image_atomic_umax"
        || ins.aux.imm != 1 || ins.aux.mask != 3 || ins.aux.imm2 != i64::from(nsa)
    {
        return Err("unsupported 64-bit image access flags".into());
    }
    let Some(Opnd::Vgpr(x)) = ins.src.first().copied() else {
        return Err("invalid image coordinates".into());
    };
    if nsa {
        let Some(Opnd::Vgpr(y)) = ins.src.get(1).copied() else {
            return Err("invalid image coordinates".into());
        };
        if x != ins.raw[1] as u8 || y != ins.raw[2] as u8 {
            return Err("invalid image coordinates".into());
        }
        Ok([x, y])
    } else {
        Ok([x, x.checked_add(1).ok_or("64-bit image coordinate register overflow")?])
    }
}

/// Number of VGPRs occupied by the MIMG data operand. Gather selects a channel
/// with DMASK but returns four texels. D16 packs two components per register.
pub(in crate::hlsl) fn image_data_registers(ins: &Insn) -> u8 {
    let components = if ins.mnemonic.starts_with("image_gather4") {
        4
    } else {
        (ins.aux.mask & 0xf).count_ones() as u8
    };
    if ins.raw.get(1).is_some_and(|word| word >> 31 != 0) {
        components.div_ceil(2)
    } else {
        components
    }
}

pub(in crate::hlsl) fn name(o: &Opnd) -> Result<String, String> {
    Ok(match o {
        Opnd::Sgpr(n) => format!("s{n}"),
        Opnd::Vgpr(n) => format!("v{n}"),
        Opnd::VccLo => "vcc_lo".into(),
        Opnd::VccHi => "vcc_hi".into(),
        Opnd::ExecLo => "exec_lo".into(),
        Opnd::ExecHi => "exec_hi".into(),
        Opnd::M0 => "m0".into(),
        Opnd::Scc => "scc".into(),
        Opnd::Null => "nul".into(),
        Opnd::Temp(n) => format!("tb_s{n}"),
        other => return Err(format!("operand {other:?} is not assignable")),
    })
}

// Return the bits of this encoded field that can be read back or updated,
// relative to its starting offset. MODE has a gap before FP16_OVFL (bit 23).
pub(in crate::hlsl) fn modeled_hwreg_mask(word: u32) -> u32 {
    let known = match word & 63 {
        1 => 0x0080_03ff,
        20 | 21 => u32::MAX, // FLAT_SCRATCH_LO/HI
        _ => 0,
    };
    let shift = (word >> 6) & 31;
    let encoded_width = (word >> 11) & 31;
    (known >> shift) & (u32::MAX >> (31 - encoded_width))
}

/// Operand as a `uint` rvalue expression.
pub(in crate::hlsl) fn r(o: &Opnd) -> Result<String, String> {
    Ok(match o {
        Opnd::ConstInt(v) => format!("{:#x}u", *v as i32 as u32),
        Opnd::ConstF32(f) => format!("{:#x}u", f.to_bits()),
        Opnd::Literal(v) => format!("{v:#x}u"),
        Opnd::Raw(c) => return Err(format!("unmodelled operand code {c}")),
        other => name(other)?,
    })
}

pub(in crate::hlsl) fn f(o: &Opnd) -> Result<String, String> {
    Ok(format!("asfloat({})", r(o)?))
}

// Selector for tb_unary_f16, constant at each translated instruction.
pub(in crate::hlsl) fn half_unary_op(mnemonic: &str) -> Option<u8> {
    match mnemonic {
        "v_floor_f16" => Some(0),
        "v_ceil_f16" => Some(1),
        "v_trunc_f16" => Some(2),
        "v_rndne_f16" => Some(3),
        "v_fract_f16" => Some(4),
        _ => None,
    }
}

pub(in crate::hlsl) fn integer_to_half(mnemonic: &str) -> bool {
    matches!(mnemonic, "v_cvt_f16_u16" | "v_cvt_f16_i16")
}

pub(in crate::hlsl) fn normalized_half_to_integer(mnemonic: &str) -> bool {
    matches!(mnemonic, "v_cvt_norm_u16_f16" | "v_cvt_norm_i16_f16")
}

pub(in crate::hlsl) fn half_round_or_conversion(m: &str) -> bool {
    half_unary_op(m).is_some() || integer_to_half(m) || half_to_integer(m)
}

// Selector 248 is the only inline float whose binary16 value differs from its binary32 value.
pub(in crate::hlsl) fn inline_half_value(value: f32) -> f32 {
    if value.to_bits() == 0x3e22_f983 { f32::from_bits(0x3e23_0000) } else { value }
}

pub(in crate::hlsl) fn half_operand_bits(o: &Opnd) -> Result<String, String> {
    match o {
        Opnd::ConstF32(_) => Ok(format!("tb_f16_rne({})", f(o)?)),
        _ => r(o),
    }
}

pub(in crate::hlsl) fn half_to_integer(mnemonic: &str) -> bool {
    matches!(mnemonic, "v_cvt_u16_f16" | "v_cvt_i16_f16") || normalized_half_to_integer(mnemonic)
}

pub(in crate::hlsl) fn mul24_expr(op: &str, a: &str, b: &str) -> Option<String> {
    Some(match op {
        "v_mul_u32_u24" => format!("(({a} & 0xffffffu) * ({b} & 0xffffffu))"),
        "v_mul_i32_i24" => format!("asuint((asint({a} << 8) >> 8) * (asint({b} << 8) >> 8))"),
        "v_mul_hi_u32_u24" => format!("tb_mulhi_u32({a} & 0xffffffu, {b} & 0xffffffu)"),
        "v_mul_hi_i32_i24" => format!("tb_mulhi_i32(asuint(asint({a} << 8) >> 8), asuint(asint({b} << 8) >> 8))"),
        _ => return None,
    })
}

/// Low `width` bits set, saturated to one word. Keep full-width shifts out of the generated code.
pub(in crate::hlsl) fn low_mask(width: &str) -> String {
    format!("(({width}) >= 32u ? 0xffffffffu : ((1u << (({width}) & 31u)) - 1u))")
}

/// Signed addition overflows when both operands have the same sign and the sum changes it.
/// SCC of `s_lshl<n>_add_u32`: set when `(a << shift) + b` exceeds 32 bits.
pub(in crate::hlsl) fn shift_add_carry(a: &str, b: &str, result: &str, shift: u32) -> String {
    format!("((({a}) >> {}) != 0u || {result} < {b}) ? 1u : 0u", 32 - shift)
}

pub(in crate::hlsl) fn signed_add_overflow(a: &str, b: &str, sum: &str) -> String {
    format!("((~({a} ^ {b}) & ({a} ^ {sum})) >> 31)")
}

/// Scalar register number of an operand (SGPRs, plus vcc_lo/vcc_hi = 106/107, which SMEM
/// loads can target directly: `s_buffer_load_dwordx2 vcc_lo, ...`).
pub(in crate::hlsl) fn sreg_num(o: &Opnd) -> Option<u8> {
    match o {
        Opnd::Sgpr(n) => Some(*n),
        Opnd::VccLo => Some(106),
        Opnd::VccHi => Some(107),
        _ => None,
    }
}

pub(in crate::hlsl) fn sreg_opnd(n: u8) -> Opnd {
    match n {
        106 => Opnd::VccLo,
        107 => Opnd::VccHi,
        n => Opnd::Sgpr(n),
    }
}

/// High half of a 64-bit scalar operand.
pub(in crate::hlsl) fn hi(o: &Opnd) -> Opnd {
    match o {
        Opnd::Sgpr(n) => Opnd::Sgpr(n + 1),
        Opnd::VccLo => Opnd::VccHi,
        Opnd::ExecLo => Opnd::ExecHi,
        Opnd::ConstInt(v) => Opnd::ConstInt(if *v < 0 { -1 } else { 0 }),
        Opnd::Null => Opnd::Null,
        _ => Opnd::ConstInt(0),
    }
}

/// Name of the SGPR used as an SMEM dynamic offset (SOFFSET field).
pub(in crate::hlsl) fn ssrc_name(code: u32) -> Result<String, String> {
    match code {
        0..=107 => r(&sreg_opnd(code as u8)),
        _ => Err(format!("unsupported SOFFSET register code {code}")),
    }
}

pub(in crate::hlsl) fn sig_type(e: &SigElement) -> (String, usize) {
    // Component count = number of mask bits (elements can be packed into a shared register,
    // e.g. TEXCOORD4.xy + TEXCOORD5.zw, so the highest bit is not the count).
    let comps = (e.mask.count_ones() as usize).max(1);
    let base = match e.comp_type {
        1 => "uint",
        2 => "int",
        _ => "float",
    };
    (if comps == 1 { base.to_string() } else { format!("{base}{comps}") }, comps)
}

/// Map a hardware (register, component) to (signature element index, component within
/// that element). RDNA2 attributes / parameter exports are whole registers; a register may
/// hold several packed signature elements.
pub(in crate::hlsl) fn sig_lookup(sig: &[SigElement], register: u32, chan: usize) -> Option<(usize, usize)> {
    sig.iter().enumerate().find_map(|(idx, e)| {
        if e.sysval == 0 && e.register == register && (e.mask >> chan) & 1 != 0 {
            Some((idx, (e.mask & ((1u8 << chan) - 1)).count_ones() as usize))
        } else {
            None
        }
    })
}

/// Distinct registers of the given elements, in signature order.
pub(in crate::hlsl) fn distinct_regs<'s>(it: impl Iterator<Item = &'s SigElement>) -> Vec<u32> {
    let mut v: Vec<u32> = Vec::new();
    for e in it {
        if !v.contains(&e.register) {
            v.push(e.register);
        }
    }
    v
}

pub(in crate::hlsl) fn semantic(e: &SigElement) -> String {
    format!("{}{}", e.name, e.index)
}

/// HLSL expression for one channel of a typed buffer element in format
/// 16_16_16_16_UINT (69): 8 bytes, 4 x 16-bit unsigned channels.
pub(in crate::hlsl) fn tbuf_16_16_16_16_uint_channel(channel: u32, packed: &str, packed1: &str) -> String {
    let word = if channel >= 2 { packed1 } else { packed };
    format!("(({word} >> {}u) & 65535u)", (channel % 2) * 16)
}

/// HLSL expression for `s_ff1_i32_b64`: first set bit of a 64-bit value read
/// through its two 32-bit words.
pub(in crate::hlsl) fn ff1_b64_expr(lo: &str, hi: &str) -> String {
    format!(
        "({lo} != 0u ? uint(firstbitlow({lo})) : {hi} != 0u ? 32u + uint(firstbitlow({hi})) : 0xffffffffu)"
    )
}

/// HLSL expression for `s_flbit_i32_b64`: first set bit from MSB to LSB over a 64-bit value.
pub(in crate::hlsl) fn flbit_b64_expr(lo: &str, hi: &str) -> String {
    format!("({hi} != 0u ? 31u - firstbithigh({hi}) : {lo} != 0u ? 63u - firstbithigh({lo}) : ~0u)")
}

/// HLSL expression for `s_ff0_i32_b32`: first zero bit from LSB to MSB over a 32-bit value.
pub(in crate::hlsl) fn ff0_b32_expr(val: &str) -> String {
    format!("firstbitlow(~{val})")
}

/// HLSL expression for `s_ff0_i32_b64`: first zero bit from LSB to MSB over a 64-bit value.
pub(in crate::hlsl) fn ff0_b64_expr(lo: &str, hi: &str) -> String {
    format!("(~{lo} != 0u ? uint(firstbitlow(~{lo})) : ~{hi} != 0u ? 32u + uint(firstbitlow(~{hi})) : 0xffffffffu)")
}
