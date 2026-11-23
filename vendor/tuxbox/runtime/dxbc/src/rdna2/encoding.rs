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
mod tests {
    use super::*;

    #[test]
    fn short_formats_handle_embedded_immediates_and_extended_operands() {
        // Base words from assembling individual instructions on gfx1010 and
        // gfx1030. The two mad*k_f32 encodings belong to gfx1010.
        for (word, kind, opcode, len) in [
            (0x8003_01ff, Enc::Sop2, 0, 2),    // s_add_u32 with literal src0
            (0xbf01_ff02, Enc::Sopc, 1, 2),    // s_cmp_lg_i32 with literal src1
            (0xbe82_03ff, Enc::Sop1, 3, 2),    // s_mov_b32
            (0xba80_0001, Enc::Sopk, 21, 2),   // s_setreg_imm32_b32
            (0xbf81_0000, Enc::Sopp, 1, 1),    // s_endpgm
            (0x7e02_03ff, Enc::Vop1, 1, 1),    // v_mov_b32 v1, v255
            (0x7e02_02ff, Enc::Vop1, 1, 2),    // v_mov_b32 v1, literal
            (0x7d84_02ff, Enc::Vopc, 194, 2),  // v_cmp_eq_u32
            (0x7e00_02fa, Enc::Vop1, 1, 2),    // DPP16
            (0x7e00_02e9, Enc::Vop1, 1, 2),    // DPP8
            (0x7e00_02ea, Enc::Vop1, 1, 2),    // DPP8 with fetch inactive
            (0x7e08_02f9, Enc::Vop1, 1, 2),    // SDWA
            (0x4002_0702, Enc::Vop2, 32, 2),   // v_madmk_f32
            (0x4202_0702, Enc::Vop2, 33, 2),   // v_madak_f32
            (0x5a04_0903, Enc::Vop2, 45, 2),   // v_fmaak_f32
            (0xc805_0d02, Enc::Vinterp, 1, 1), // v_interp_p2_f32
        ] {
            assert_eq!(classify(word), kind);
            assert_eq!(opcode_field(kind, word, None), opcode);
            assert_eq!(instr_len(kind, word, None), len);
        }
    }

    #[test]
    fn packed_and_unpacked_valu_share_one_literal_between_three_sources() {
        for (word, opcode) in [(0xcc02_4001, 0x402), (0xd54b_0000, 0x14b)] {
            let enc = classify(word);
            assert_eq!(enc, Enc::Vop3);
            assert_eq!(opcode_field(enc, word, None), opcode);
            assert_eq!(instr_len(enc, word, None), 2);
            assert_eq!(instr_len(enc, word, Some(0x07ff_ffff)), 2); // v255 in each slot
            for sources in [255, 255 << 9, 255 << 18, 255 | (255 << 18)] {
                assert_eq!(instr_len(enc, word, Some(sources)), 3);
            }
        }
    }

    #[test]
    fn address_word_count_is_independent_of_split_image_opcode() {
        for (word, opcode, len) in [
            (0xf000_0108, 0, 2),   // contiguous image_load
            (0xf000_010a, 0, 3),   // image_load with scattered v7,v9
            (0xf000_0133, 128, 3), // image_msaa_load on gfx1030
            (0xf088_0114, 34, 4),  // image_sample_d (9 address registers)
            (0xf0ec_0116, 59, 5),  // image_sample_c_d_cl_o (12 registers)
        ] {
            assert_eq!(classify(word), Enc::Mimg);
            assert_eq!(opcode_field(Enc::Mimg, word, None), opcode);
            assert_eq!(instr_len(Enc::Mimg, word, None), len);
        }
    }

    #[test]
    fn typed_buffer_opcode_uses_both_words() {
        assert_eq!(classify(0xe830_0000), Enc::Mtbuf);
        // tbuffer_load_format_d16_x has the D16 opcode bit in word 1.
        assert_eq!(opcode_field(Enc::Mtbuf, 0xe830_0000, Some(0x8020_0000)), 8);
        assert_eq!(opcode_field(Enc::Mtbuf, 0xe830_0000, Some(0x8000_0000)), 0);
        assert_eq!(opcode_field(Enc::Mubuf, 0xe200_0000, None), 128);
    }
}
