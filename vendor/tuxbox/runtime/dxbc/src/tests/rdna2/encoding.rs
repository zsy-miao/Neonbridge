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
