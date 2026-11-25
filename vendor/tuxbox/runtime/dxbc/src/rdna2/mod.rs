//! Decode the GFX10 instructions stored in native Xbox shader programs.
//! Instruction lengths, opcode names and register fields are defined by the
//! encoding, opcodes and operands modules respectively.

mod encoding;
mod opcodes;
mod operands;

/// Encoding family identified by the instruction prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enc {
    Sop2,
    Sopk,
    Sop1,
    Sopc,
    Sopp,
    Smem,
    Vop2,
    Vop1,
    Vopc,
    Vop3,
    Vinterp,
    Ds,
    Mubuf,
    Mtbuf,
    Mimg,
    Flat,
    Exp,
    Unknown,
}

/// Encoded register or value. A register group is named by its first register.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Opnd {
    Sgpr(u8),
    Vgpr(u8),
    VccLo,
    VccHi,
    ExecLo,
    ExecHi,
    M0,
    Scc,
    Null,
    ConstInt(i64),
    ConstF32(f32),
    Literal(u32),
    Raw(u16),
    /// Temporary allocated by the HLSL translator.
    Temp(u16),
}

/// Encoding-specific controls, such as a branch offset, export target, memory
/// access mask or a sub-dword addressing control word.
#[derive(Debug, Clone, Default)]
pub struct InsnAux {
    pub imm: i64,
    pub mask: u8,
    pub imm2: i64,
    pub sdwa: Option<u32>,
}

/// One instruction and its position in the input byte stream.
#[derive(Debug, Clone)]
pub struct Insn {
    pub offset: usize,
    pub dwords: usize,
    pub enc: Enc,
    pub op: u32,
    pub raw: Vec<u32>,
    pub mnemonic: String,
    pub dst: Vec<Opnd>,
    pub src: Vec<Opnd>,
    pub aux: InsnAux,
}

impl Insn {
    /// Whether all dwords required by this instruction's encoding are present.
    pub fn is_complete(&self) -> bool {
        self.raw.first().is_some_and(|&first| {
            self.raw.len() == encoding::instr_len(self.enc, first, self.raw.get(1).copied())
        })
    }
}

// Scalar bit counts and searches read a pair, but only overwrite one SGPR.
pub fn wide_bit_result(mnemonic: &str) -> bool {
    mnemonic.ends_with("_b64") && !mnemonic.ends_with("_i32_b64")
}

pub fn writes_scalar(op: &Insn, register: u8) -> bool {
    let m = op.mnemonic.as_str();
    let width = if m.ends_with("dwordx16") {
        16
    } else if m.ends_with("dwordx8") {
        8
    } else if m.ends_with("dwordx4") {
        4
    } else if m.ends_with("dwordx2")
        || wide_bit_result(m)
        || (m.ends_with("_i64") && !m.ends_with("_i32_i64"))
        || m.ends_with("_u64")
        || m.starts_with("v_cmp")
        || matches!(m, "s_bitreplicate_b64_b32" | "s_memtime" | "s_memrealtime")
    {
        2
    } else {
        1
    };
    op.dst.iter().any(|dst| {
        let base = match *dst {
            Opnd::Sgpr(n) => n,
            Opnd::Vgpr(n) if m == "v_readlane_b32" => n, // scalar destination in the VOP3 encoding
            _ => return false,
        };
        (base..base.saturating_add(width)).contains(&register)
    })
}

/// Includes VOP3B secondary scalar destinations when tracking a reaching definition.
pub fn clobbers_scalar(op: &Insn, reg: u8) -> bool {
    if writes_scalar(op, reg) {
        return true;
    }
    if op.enc == Enc::Vop3
        && (op.mnemonic.contains("_co_")
            || matches!(
                op.mnemonic.as_str(),
                "v_mad_u64_u32" | "v_mad_i64_i32" | "v_div_scale_f32" | "v_div_scale_f64"
            ))
    {
        let dst = ((op.raw[0] >> 8) & 127) as u8;
        return reg == dst || reg == dst + 1;
    }
    op.enc == Enc::Vop3
        && op.mnemonic == "v_readfirstlane_b32"
        && op.raw[0] & 127 == u32::from(reg)
}


/// Iterate over complete dwords, including available words of a truncated final
/// instruction. Offsets are relative to the start of `isa`, in bytes.
pub fn instructions(isa: &[u8]) -> impl Iterator<Item = Insn> + '_ {
    let (words, _) = isa.as_chunks::<4>();
    let mut cursor = 0;
    std::iter::from_fn(move || {
        let rest = &words[cursor..];
        let first = u32::from_le_bytes(*rest.first()?);
        let next = rest.get(1).copied().map(u32::from_le_bytes);
        let enc = encoding::classify(first);
        let dwords = encoding::instr_len(enc, first, next).min(rest.len());
        let raw: Vec<_> = rest[..dwords]
            .iter()
            .copied()
            .map(u32::from_le_bytes)
            .collect();
        let op = encoding::opcode_field(enc, first, next);
        let mnemonic = opcodes::lookup(enc, op).map_or_else(
            || format!("{enc:?}<{op:#x}>").to_ascii_lowercase(),
            str::to_owned,
        );
        let (dst, src, aux) = operands::decode_operands(enc, &raw);
        let offset = cursor * 4;
        cursor += dwords;
        Some(Insn {
            offset,
            dwords,
            enc,
            op,
            raw,
            mnemonic,
            dst,
            src,
            aux,
        })
    })
}

/// Decode the full stream for consumers that need indexed instruction access.
pub fn disassemble(isa: &[u8]) -> Vec<Insn> {
    instructions(isa).collect()
}

#[cfg(test)]
#[path = "../tests/rdna2/mod.rs"]
mod tests;
