//! Instruction boundaries and opcode fields for AMD GFX10.1 and GFX10.3.
//!
//! Derived from the RDNA 1 and RDNA 2 encoding and instruction tables in
//! [AMD's machine-readable ISA specifications](https://gpuopen.com/machine-readable-isa/).
//! The [RDNA 2 ISA manual](https://gpuopen.com/download/rdna2-shader-instruction-set-architecture.pdf)
//! describes the formats in chapter 13. Classification identifies the wire
//! format even when an opcode within that format is reserved.

use super::Enc;

pub(super) fn classify(word: u32) -> Enc {
    match word >> 26 {
        0x00..=0x1e => Enc::Vop2,
        0x1f if word & (1 << 25) == 0 => Enc::Vopc,
        0x1f => Enc::Vop1,
        0x20..=0x2b => Enc::Sop2,
        0x2c..=0x2f => match word >> 23 {
            0x17d => Enc::Sop1,
            0x17e => Enc::Sopc,
            0x17f => Enc::Sopp,
            _ => Enc::Sopk,
        },
        0x32 => Enc::Vinterp,
        0x33 | 0x35 => Enc::Vop3,
        0x36 => Enc::Ds,
        0x37 => Enc::Flat,
        0x38 => Enc::Mubuf,
        0x3a => Enc::Mtbuf,
        0x3c => Enc::Mimg,
        0x3d => Enc::Smem,
        0x3e => Enc::Exp,
        _ => Enc::Unknown,
    }
}

/// Returns the opcode used by the instruction dispatcher. Packed VOP3
/// operations occupy a separate range above the ten-bit VOP3 opcodes.
pub(super) fn opcode_field(enc: Enc, word: u32, next: Option<u32>) -> u32 {
    match enc {
        Enc::Sop2 => (word >> 23) & 0x7f,
        Enc::Sopk => (word >> 23) & 0x1f,
        Enc::Sop1 => (word >> 8) & 0xff,
        Enc::Sopc | Enc::Sopp => (word >> 16) & 0x7f,
        Enc::Smem | Enc::Ds | Enc::Mubuf => (word >> 18) & 0xff,
        Enc::Vop2 => (word >> 25) & 0x3f,
        Enc::Vop1 => (word >> 9) & 0xff,
        Enc::Vopc => (word >> 17) & 0xff,
        Enc::Vop3 if word >> 26 == 0x33 => 0x400 | ((word >> 16) & 0x7f),
        Enc::Vop3 => (word >> 16) & 0x3ff,
        Enc::Vinterp => (word >> 16) & 3,
        Enc::Flat => (word >> 18) & 0x7f,
        Enc::Mtbuf => ((next.unwrap_or_default() >> 18) & 8) | ((word >> 16) & 7),
        Enc::Mimg => ((word & 1) << 7) | ((word >> 18) & 0x7f),
        Enc::Exp | Enc::Unknown => (word >> 18) & 0xff,
    }
}

/// Number of 32-bit words occupied by an instruction, including its control,
/// literal, or extra vector address words. A caller walking a bounded shader
/// must check that the indicated words are present before advancing.
pub(super) fn instr_len(enc: Enc, word: u32, next: Option<u32>) -> usize {
    match enc {
        Enc::Sop1 => 1 + usize::from(word & 0xff == 255),
        Enc::Sop2 | Enc::Sopc => 1 + usize::from(word & 0xff == 255 || (word >> 8) & 0xff == 255),
        Enc::Sopk => 1 + usize::from(opcode_field(enc, word, next) == 21), // s_setreg_imm32_b32
        Enc::Vop2 | Enc::Vop1 | Enc::Vopc => {
            // Source zero selects DPP8, DPP8FI, SDWA, DPP16, or a literal.
            let extended = matches!(word & 0x1ff, 233 | 234 | 249 | 250 | 255);
            // MADAK/MADMK (RDNA 1) and the FMAAK/FMAMK family encode K in
            // the following word regardless of the source-zero selector.
            let has_k = matches!(enc, Enc::Vop2)
                && matches!(opcode_field(enc, word, next), 32 | 33 | 44 | 45 | 55 | 56);
            1 + usize::from(extended || has_k)
        }
        Enc::Vop3 => {
            let literal = next.is_some_and(|sources| {
                [0, 9, 18]
                    .iter()
                    .any(|shift| (sources >> shift) & 0x1ff == 255)
            });
            2 + usize::from(literal)
        }
        Enc::Mimg => 2 + ((word >> 1) & 3) as usize,
        Enc::Smem | Enc::Ds | Enc::Flat | Enc::Mubuf | Enc::Mtbuf | Enc::Exp => 2,
        Enc::Sopp | Enc::Vinterp | Enc::Unknown => 1,
    }
}

#[cfg(test)]
#[path = "../tests/rdna2/encoding.rs"]
mod tests;
