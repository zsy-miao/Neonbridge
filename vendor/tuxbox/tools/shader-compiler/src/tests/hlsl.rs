use super::*;

#[test]
fn vector_add_sub_can_discard_the_carry_destination() {
    use super::float64::tests::translate;

    // gfx1030 VOP3B add, sub and subrev with SDST=null; the _ci_ forms read VCC.
    for (opcode, sources) in [
        (0xd70f_7d04, 0x0002_0501), (0xd710_7d04, 0x0002_0501),
        (0xd719_7d04, 0x0002_0501), (0xd528_7d04, 0x01aa_0501),
        (0xd529_7d04, 0x01aa_0501), (0xd52a_7d04, 0x01aa_0501),
    ] {
        let hlsl = translate(Stage::Compute, &[opcode, sources]).unwrap();
        assert!(hlsl.contains("v4 = t"), "{hlsl}");
        assert!(!hlsl.lines().any(|line| {
            let stmt = line.trim_start();
            stmt.starts_with("vcc_lo =") || stmt.starts_with("vcc_hi =") || stmt.starts_with("nul =")
        }), "{hlsl}");
        assert!(translate(Stage::Compute, &[opcode, sources | (1 << 29)]).is_err());
    }
}

#[test]
fn vector_carry_clamp_saturates_the_result_and_keeps_the_overflow_flag() {
    use super::float64::tests::translate;

    for (opcode, limit) in [(0xd70f_ea04, "0xffffffffu"), (0xd710_ea04, "0u")] {
        // gfx1030: v_{add,sub}_co_u32 v4, vcc_lo, v1, v2 clamp
        let hlsl = translate(Stage::Compute, &[opcode, 0x0002_0501]).unwrap();
        assert!(hlsl.contains(&format!("if (t0o != 0u) t0 = {limit};")), "{hlsl}");
        assert!(hlsl.lines().any(|line| line.trim_start().starts_with("vcc_lo =") && line.contains("t0o")));
    }

    // In e32 the same bit belongs to the source register v66.
    let hlsl = translate(Stage::Compute, &[0x5008_8501]).unwrap();
    assert!(!hlsl.contains("if (t0o != 0u)"), "{hlsl}");
}

#[test]
fn unsigned_no_carry_clamp_is_specific_to_the_long_encoding() {
    use super::float64::tests::translate;
    // LLVM gfx1030: v_{add,sub,subrev}_nc_u32_e64 v4, v1, v2 clamp.
    for (opcode, saturated, wrapping) in [
        (0xd525_8004, "(min(v1, ~v2) + v2)", "(v1 + v2)"),
        (0xd526_8004, "(max(v1, v2) - v2)", "(v1 - v2)"),
        (0xd527_8004, "(max(v2, v1) - v1)", "(v2 - v1)"),
    ] {
        let hlsl = translate(Stage::Compute, &[opcode, 0x0002_0501]).unwrap();
        assert!(hlsl.contains(saturated), "{hlsl}");
        let hlsl = translate(Stage::Compute, &[opcode & !0x8000, 0x0002_0501]).unwrap();
        assert!(hlsl.contains(wrapping), "{hlsl}");
    }
    // In e32 the same bit is part of the second source register (v66).
    assert!(translate(Stage::Compute, &[0x4a08_8501]).unwrap().contains("(v1 + v66)"));
}

#[test]
fn mul24_clamps_the_full_product_plus_accumulator_before_packing() {
    for (words, args) in [
        ([0xd50b_8004, 0x0002_0501], "v1, v2, 0u, false"), // unsigned MUL
        ([0xd542_8004, 0x040e_0501], "v1, v2, v3, true"), // signed MAD
        ([0xd543_8004, 0x040e_0501], "v1, v2, v3, false"), // unsigned MAD
        ([0x1208_04f9, 0x0606_3201], "v1, v2, 0u, true"), // signed MUL, BYTE_2
    ] {
        let shader = translate_valu_probe(&words, 5).unwrap();
        assert!(shader.contains(&format!("= tb_mad24_sat({args});")));
        if words[0] == 0x1208_04f9 {
            assert!(shader.contains("0xff00ffffu"));
        }
    }
    let ordinary = translate_valu_probe(&[0xd50b_0004, 0x0002_0501], 5).unwrap();
    assert!(!ordinary.contains("tb_mad24_sat"));
    // Integer products have no floating output scale; SDWA DST_UNUSED=3 is reserved.
    for words in [
        [0xd50b_8004, 0x0802_0501],
        [0x1208_04f9, 0x0606_7201],
        [0x1208_04f9, 0x0606_3a01],
    ] {
        assert!(translate_valu_probe(&words, 5).is_err());
    }
}

#[test]
fn half_rounding_reads_the_entry_destination_and_obeys_fp_mode() {
    // s_denorm_mode 4; v_floor_f16_e64 v6,-|v1| mul:2
    let hlsl = float64::tests::translate(
        Stage::Compute, &[0xbfa5_0004, 0xd5db_0106, 0x2800_0101],
    ).unwrap();
    assert!(hlsl.lines().next().unwrap().split_whitespace().any(|word| word == "v6"));
    let write = hlsl.lines().find(|line| line.contains("v6 & 0xffff0000u")).unwrap();
    assert!(write.contains("tb_unary_f16(") && write.contains("tb_fp_mode, 0u)"));
    assert!(write.contains("tb_half_output(tb_unary_f16(") && write.contains("tb_fp_mode, 2.0, false"));
}

#[test]
fn half_conversion_only_reads_destination_for_partial_writes() {
    // Both directions, signed and unsigned: convert v1.h into v6.h.
    for opcode in [0x7e0c_a0f9, 0x7e0c_a2f9, 0x7e0c_a4f9, 0x7e0c_a6f9, 0x7e0c_c6f9, 0x7e0c_c8f9] {
        for (extension, preserve) in [(0x51501, true), (0x50501, false), (0x50d01, false), (0x51601, false)] {
            let hlsl = float64::tests::translate(Stage::Compute, &[opcode, extension]).unwrap();
            let inputs = hlsl.lines().next().unwrap();
            assert_eq!(inputs.split_whitespace().any(|word| word == "v6"), preserve);
        }
    }
}

#[test]
fn fract_f16_applies_the_mode_and_modifiers_to_the_low_half() {
    // s_denorm_mode 4; v_fract_f16_e64 v4,-|v1| mul:2 (gfx1030).
    let hlsl = float64::tests::translate(
        Stage::Compute, &[0xbfa5_0004, 0xd5df_0104, 0x2800_0101],
    ).unwrap();
    assert!(hlsl.contains("v4 & 0xffff0000u"));
    assert!(hlsl.contains("tb_fp_mode, 4u), tb_fp_mode, 2.0, false)"));
    // An unwritten v6 supplies the high half through the entry ABI.
    let first_write = float64::tests::translate(Stage::Compute, &[0x7e0c_bf04]).unwrap();
    assert!(first_write.lines().next().unwrap().split_whitespace().any(|word| word == "v6"));
}

#[test]
fn fract_f32_uses_the_guest_denormal_mode() {
    // gfx1030: v_fract_f32 v8, v4, optionally preceded by s_denorm_mode 3.
    let fract = 0x7e10_4104;
    for (code, mode) in [(vec![fract], "0u"), (vec![0xbfa5_0003, fract], "tb_fp_mode")] {
        let hlsl = float64::tests::translate(Stage::Compute, &code).unwrap();
        assert!(hlsl.contains(&format!("tb_fract_f32(asfloat(v4), {mode})")));
        assert!(hlsl.contains("float tb_fract_f32("));
    }
}

#[test]
fn conversion_overflow_bit_can_be_set_cleared_and_read() {
    // MODE[23] is FP16_OVFL. Reads including reserved bits remain unsupported.
    let get = 0xb91a_05c1;
    let hlsl = float64::tests::translate(Stage::Compute, &[
        0xba80_05c1, 1, get, 0xba80_05c1, 0, get,
    ]).unwrap();
    assert!(hlsl.contains("((tb_fp_mode >> 23) & 0x1u)"));
    assert!(hlsl.contains("((0x1u & 0x1u) << 23)") && hlsl.contains("((0x0u & 0x1u) << 23)"));
    assert!(float64::tests::translate(Stage::Compute, &[get | (1 << 11)]).is_err());
}

#[test]
fn single_to_half_reads_the_old_destination_only_for_partial_writes() {
    // v_cvt_f16_f32 v6,v1 and SDWA with DWORD source.
    for (words, preserves) in [
        (vec![0x7e0c_1501], true),
        (vec![0x7e0c_14f9, 0x0006_1501], true),  // WORD_1, PRESERVE
        (vec![0x7e0c_14f9, 0x0006_0501], false), // WORD_1, PAD
        (vec![0x7e0c_14f9, 0x0006_1601], false), // DWORD
    ] {
        let hlsl = float64::tests::translate(Stage::Compute, &words).unwrap();
        let incoming = hlsl.lines().next().unwrap().split_whitespace().any(|word| word == "v6");
        assert_eq!(incoming, preserves);
    }
}

#[test]
fn reads_all_mode_fields_through_ieee() {
    // s_getreg_b32 s26, hwreg(MODE, shift, width).
    for shift in 0..10 {
        for width in 1..=10 - shift {
            let get = 0xb91a_0001 | (shift << 6) | ((width - 1) << 11);
            let hlsl = float64::tests::translate(Stage::Compute, &[get]).unwrap();
            let mask = (1u32 << width) - 1;
            assert!(hlsl.contains(&format!("((tb_fp_mode >> {shift}) & {mask:#x}u)")));
        }
    }
    // LOD_CLAMPED (bit 10) is updated by texture instructions.
    for get in [0xb91a_0281, 0xb91a_0a41] {
        assert!(float64::tests::translate(Stage::Compute, &[get]).is_err());
    }
}

#[test]
fn wide_mode_writes_update_readable_bits() {
    // Write MODE[9:0] from s5, overwrite MODE[10:6] with 9, read bit 8.
    let hlsl = float64::tests::translate(Stage::Compute,
        &[0xb985_4801, 0xba80_2181, 9, 0xb91a_0201]).unwrap();
    assert!(hlsl.lines().next().unwrap().split_whitespace().any(|reg| reg == "s5"));
    assert!(hlsl.contains("((s5 & 0x3ffu) << 0)"));
    assert!(hlsl.contains("((0x9u & 0xfu) << 6)"));
    // The immediate form's encoding has a zero in the SGPR field, but
    // leaves the entry value of s0 available to the following v_mov.
    let hlsl = float64::tests::translate(Stage::Compute,
        &[0xba80_2181, 9, 0x7e08_0200]).unwrap();
    assert!(hlsl.lines().next().unwrap().split_whitespace().any(|reg| reg == "s0"));
}

#[test]
fn sdwa_f32_applies_modifiers_before_preserving_unselected_bits() {
    use super::float64::tests::translate;
    // gfx1030: v_add_f32_sdwa v4, -|v1|, v2 clamp mul:2 dst_sel:WORD_1
    // dst_unused:UNUSED_PRESERVE src0_sel:DWORD src1_sel:DWORD
    let hlsl = translate(Stage::Compute, &[0x0608_04f9, 0x0636_7501]).unwrap();
    assert!(hlsl.contains("saturate((((-abs(asfloat(v1))) + asfloat(v2)) * 2.0))"), "{hlsl}");
    assert!(hlsl.contains("(v4) & 0xffffu"), "{hlsl}");
}

#[test]
fn sdwa_f32_scales_a_full_word_without_source_modifiers() {
    use super::float64::tests::translate;
    // gfx1030: v_mul_f32_sdwa v4, v1, v2 div:2 (all selectors DWORD).
    let hlsl = translate(Stage::Compute, &[0x1008_04f9, 0x0606_d601]).unwrap();
    assert!(hlsl.contains("v4 = asuint(((asfloat(v1) * asfloat(v2)) * 0.5))"), "{hlsl}");
}

#[test]
fn sdwa_f32_rounding_applies_input_and_output_modifiers() {
    use super::float64::tests::translate;
    // gfx1030: v_<op>_f32_sdwa v4, -|v1| mul:2, both selectors DWORD.
    for (opcode, intrinsic) in [
        (0x7e08_40f9, "tb_fract_f32"), (0x7e08_42f9, "trunc"), (0x7e08_44f9, "ceil"),
        (0x7e08_46f9, "round"), (0x7e08_48f9, "floor"),
    ] {
        let hlsl = translate(Stage::Compute, &[opcode, 0x0036_5601]).unwrap();
        let mode = if intrinsic == "tb_fract_f32" { ", 0u" } else { "" };
        let result = format!("asuint((({intrinsic}((-abs(asfloat(v1))){mode})) * 2.0))");
        assert!(hlsl.contains(&result), "{hlsl}");
    }
}

#[test]
fn sdwa_integer_to_float_has_float_output_and_integer_input_modifiers() {
    use super::float64::tests::translate;
    // gfx1030: signed/unsigned dword, signed nibble offset, then unsigned bytes 0..3.
    // Each writes v4 from v1 with clamp mul:4 and both selectors DWORD.
    let sdwa = 0x0006_b601;
    for opcode in [0x7e08_0af9, 0x7e08_0cf9, 0x7e08_1cf9, 0x7e08_22f9,
                   0x7e08_24f9, 0x7e08_26f9, 0x7e08_28f9] {
        let hlsl = translate(Stage::Compute, &[opcode, sdwa]).unwrap();
        assert!(hlsl.lines().any(|line| line.contains("v4 = asuint(saturate(")
            && line.ends_with(" * 4.0)));")), "{hlsl}");
        // SRC0_NEG is invalid for an integer input.
        let error = translate(Stage::Compute, &[opcode, sdwa | (1 << 20)]).unwrap_err();
        assert!(error.contains("unsupported SDWA neg/abs"), "{error}");
    }
}

#[test]
fn widen_f16_reads_a_half_precision_source_before_applying_modifiers() {
    use super::float64::tests::translate;
    // gfx1030: v_cvt_f32_f16_e32 v4, 1/(2*pi) (inline selector 248).
    let hlsl = translate(Stage::Compute, &[0x7e08_16f8]).unwrap();
    assert!(hlsl.contains("asfloat(0x3e230000u)"), "{hlsl}");
    // v_cvt_f32_f16_sdwa v4, -|v1| clamp mul:2 dst_sel:DWORD src0_sel:WORD_1
    let hlsl = translate(Stage::Compute, &[0x7e08_16f9, 0x0035_7601]).unwrap();
    assert!(hlsl.contains("((v1 >> 16) & 0xffffu)"), "{hlsl}");
    assert!(hlsl.contains("saturate((((-abs(f16tof32("), "{hlsl}");
}

#[test]
fn byte_pack_uses_the_current_single_precision_round_mode() {
    // gfx1030: v_cvt_pk_u8_f32 v4, v1, v0, v4, with and without s_round_mode.
    let pack = [0xd55e_0004, 0x0412_0101];
    let translate = |words: &[u32]| float64::tests::translate(Stage::Compute, words).unwrap();
    let default = translate(&pack);
    assert!(default.contains("tb_round_u8(asfloat(v1), 0u)"));
    let mut words = vec![0xbfa4_0001];
    words.extend(pack);
    assert!(translate(&words).contains("tb_round_u8(asfloat(v1), tb_fp_mode)"));
}

fn dpp16_word(src0_reg: u32, ctrl: u32, fi: u32, bc: u32, bank_m: u32, row_m: u32) -> u32 {
    // [src0_reg:8][ctrl:9][fi:1][bc:1][bank_m:4][row_m:4]
    src0_reg | (ctrl << 8) | (fi << 18) | (bc << 19) | (bank_m << 24) | (row_m << 28)
}

fn dpp16_move_insn(ctrl: u32) -> Insn {
    Insn {
        offset: 0x062c,
        dwords: 2,
        enc: Enc::Vop1,
        op: 0,
        raw: vec![0x7e0802fa, ctrl],
        mnemonic: "v_mov_b32".to_string(),
        dst: vec![Opnd::Vgpr(4)],
        src: vec![Opnd::Raw(250)],
        aux: Default::default(),
    }
}

#[test]
fn dpp16_row_moves_select_the_source_lane_by_xor() {
    let mirrors = [(0x141, 7), (0x140, 15)];
    for (ctrl, mask) in mirrors.into_iter().chain((0..16).map(|mask| (0x160 + mask, mask))) {
        let mut insns = [dpp16_move_insn(dpp16_word(2, ctrl, 0, 1, 0xf, 0xf))];
        lower_wave_reductions(&mut insns).unwrap();
        assert_eq!(insns[0].op, OP_ROW_XOR_MOVE);
        assert_eq!(insns[0].src, [Opnd::Vgpr(2)]);
        assert_eq!(insns[0].aux.imm, i64::from(mask));
    }
}

#[test]
fn dpp16_quad_perm_fi1_is_accepted_and_keeps_the_perm_in_aux_imm() {
    let mut insns = vec![dpp16_move_insn(dpp16_word(0, 0x0, 1, 1, 0xf, 0xf))];
    lower_wave_reductions(&mut insns).expect("quad_perm accepted");
    assert_eq!(insns[0].op, OP_QUAD_MOVE);
    assert_eq!(insns[0].src, vec![Opnd::Vgpr(0)]);
    assert_eq!(insns[0].aux.imm, 0);
}

#[test]
fn dpp16_unimplemented_control_is_refused() {
    // Zero is outside the row_ror range. Quad moves with negation are also unsupported.
    let mut insns = vec![dpp16_move_insn(dpp16_word(2, 0x120, 0, 1, 0xf, 0xf))];
    assert!(lower_wave_reductions(&mut insns).is_err());
    let mut insns = vec![dpp16_move_insn(dpp16_word(2, 0x0, 1, 1, 0xf, 0xf) | (1u32 << 20))];
    assert!(lower_wave_reductions(&mut insns).is_err());
}

#[test]
fn dpp16_mov_missing_control_word_is_refused() {
    let mut insns = vec![Insn { raw: vec![0x7e0802fa], ..dpp16_move_insn(0) }];
    assert!(lower_wave_reductions(&mut insns).is_err());
}

#[test]
fn pixel_heap_base_accepts_rooted_resource_heap_without_heap_map() {
    let mut fssm = vec![0u8; 0x124];
    fssm[..8].copy_from_slice(b"FSSM\x01\x01\0\0");
    let len = (fssm.len() - 16) as u32;
    fssm[8..12].copy_from_slice(&len.to_le_bytes());
    fssm[0xe8..0xf0].copy_from_slice(&0x100u64.to_le_bytes());
    fssm[0xf8..0xfc].copy_from_slice(&1u32.to_le_bytes());
    for (k, v) in [5u32, 0, 1, 0, 10].into_iter().enumerate() {
        fssm[0x110 + k * 4..0x114 + k * 4].copy_from_slice(&v.to_le_bytes());
    }
    let mut ex = ShaderExtract {
        stage: Stage::Pixel,
        shader_model: (6, 6),
        required_features: Some(1 << 25 | 1 << 26),
        bitcode_stripped: true,
        fspx_config: None,
        ps09_pixel_inputs: None,
        fssm,
        entry: Some("main".into()),
        isa_data: Vec::new(),
        isa: Vec::new(),
        isa_offset_in_fssm: 0,
        psv_threads: None,
        flat_inputs: Vec::new(),
        flat_outputs: Vec::new(),
        isa_preamble: [0, 64, 1, 1],
        has_input_sig: false,
        has_output_sig: false,
        has_psv0: false,
        has_root_sig: false,
        input_sig: Vec::new(),
        output_sig: Vec::new(),
        resources: Vec::new(),
        gs_info: None,
        mesh_info: None,
        primitive_sig: vec![],
        root_sig: None,
    };
    assert_eq!(pixel_heap_base(&ex, 5), Some(10));
    ex.root_sig = Some(crate::meta::RootSig { version: 2, flags: 0x401, params: vec![], num_static_samplers: 0, static_samplers: vec![] });
    assert_eq!(pixel_heap_base(&ex, 5), Some(10));
    assert_eq!(pixel_heap_base(&ex, 6), None);
    ex.root_sig.as_mut().unwrap().flags = 0x1;
    assert_eq!(pixel_heap_base(&ex, 5), None);
}

fn pixel_wave_test_extract() -> ShaderExtract {
    ShaderExtract {
        stage: Stage::Pixel,
        shader_model: (6, 6),
        required_features: None,
        bitcode_stripped: true,
        fspx_config: None,
        ps09_pixel_inputs: None,
        fssm: Vec::new(),
        entry: Some("main".into()),
        isa_data: Vec::new(),
        isa: Vec::new(),
        isa_offset_in_fssm: 0,
        psv_threads: None,
        flat_inputs: Vec::new(),
        flat_outputs: Vec::new(),
        isa_preamble: [0, 64, 1, 1],
        has_input_sig: false,
        has_output_sig: false,
        has_psv0: false,
        has_root_sig: false,
        input_sig: Vec::new(),
        output_sig: Vec::new(),
        resources: Vec::new(),
        gs_info: None,
        mesh_info: None,
        primitive_sig: vec![],
        root_sig: None,
    }
}

#[test]
fn pixel_fi0_quad_exchange_requires_a_proven_exec_mask() {
    let ex = pixel_wave_test_extract();
    let emit = |prefix: &[u32], bound: u32| {
        // GFX1030: v_or_b32_dpp v7, v5, v6 quad_perm:[1,0,3,2].
        let mut words = prefix.to_vec();
        words.extend([0x380e_0cfa, dpp16_word(5, 0xb1, 0, bound, 15, 15)]);
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let mut insns = tuxbox_dxbc::rdna2::disassemble(&bytes);
        lower_wave_reductions(&mut insns)?;
        let mut em = Emitter::new(&ex, insns);
        em.pixel_wave = true;
        em.emit_body(true)
    };
    let full = 0xbefe_04c1; // s_mov_b64 exec, -1
    let cmpx = 0x7daa_1081; // v_cmpx_ne_u32 1, v8
    for bound in [0, 1] {
        emit(&[full], bound).unwrap();
        emit(&[full, cmpx], bound).unwrap();
        for prefix in [&[][..], &[cmpx], &[0xbf84_0001, full, cmpx]] {
            let error = emit(prefix, bound).unwrap_err();
            assert!(error.contains("requires a proven source EXEC mask"), "{error}");
        }
    }
}

#[test]
fn pixel_wave_quad_exchange_works_in_block_dispatch() {
    let ex = pixel_wave_test_extract();
    let mut insns = vec![Insn { offset: 0x0, ..dpp16_move_insn(dpp16_word(0, 0xb1, 1, 1, 0xf, 0xf)) }];
    lower_wave_reductions(&mut insns).unwrap();
    let mut em = Emitter::new(&ex, insns.clone());
    em.pixel_wave = true;
    em.emit_body_dispatch(true, 0).expect("pixel_wave block dispatch quad exchange");
    assert!(em.body.contains("QuadReadAcrossX("));

    let mut em_non_pw = Emitter::new(&ex, insns);
    let err = em_non_pw.emit_body_dispatch(true, 0).expect_err("non-pixel_wave refused");
    assert!(err.contains("quad exchange in a block-dispatched program"), "{err}");
}

#[test]
fn dpp16_quad_exchange_in_loop_allowed_only_for_pixel_wave_quad_ops() {
    let ex = ShaderExtract {
        stage: Stage::Pixel,
        shader_model: (6, 6),
        required_features: None,
        bitcode_stripped: true,
        fspx_config: None,
        ps09_pixel_inputs: None,
        fssm: Vec::new(),
        entry: Some("main".into()),
        isa_data: Vec::new(),
        isa: Vec::new(),
        isa_offset_in_fssm: 0,
        psv_threads: None,
        flat_inputs: Vec::new(),
        flat_outputs: Vec::new(),
        isa_preamble: [0, 64, 1, 1],
        has_input_sig: false,
        has_output_sig: false,
        has_psv0: false,
        has_root_sig: false,
        input_sig: Vec::new(),
        output_sig: Vec::new(),
        resources: Vec::new(),
        gs_info: None,
        mesh_info: None,
        primitive_sig: vec![],
        root_sig: None,
    };
    let loop_insns = |dpp_word: u32| {
        let mut insns = vec![
            Insn { offset: 0x0, ..dpp16_move_insn(dpp_word) },
            Insn {
                offset: 0x8,
                dwords: 1,
                enc: Enc::Sopp,
                op: 4, // s_cbranch_scc0
                raw: vec![0xbf84fffd],
                mnemonic: "s_cbranch_scc0".into(),
                dst: vec![],
                src: vec![],
                aux: tuxbox_dxbc::rdna2::InsnAux { imm: -3, ..Default::default() },
            },
        ];
        lower_wave_reductions(&mut insns).unwrap();
        insns
    };
    let quad_insns = loop_insns(dpp16_word(0, 0xb1, 1, 1, 0xf, 0xf));
    let mut em = Emitter::new(&ex, quad_insns.clone());
    em.pixel_wave = true;
    em.emit_body(true).expect("pixel_wave quad exchange in loop");
    assert!(em.body.contains("QuadReadAcrossX("));

    let mut em_non_pw = Emitter::new(&ex, quad_insns);
    let err = em_non_pw.emit_body(true).expect_err("non-pixel_wave refused");
    assert!(err.contains("DPP16 exchange inside backward control flow"), "{err}");

    let mirror_insns = loop_insns(dpp16_word(2, 0x140, 0, 1, 0xf, 0xf));
    let mut em_mirror = Emitter::new(&ex, mirror_insns);
    em_mirror.pixel_wave = true;
    let err = em_mirror.emit_body(true).expect_err("row_mirror refused");
    assert!(err.contains("DPP16 exchange inside backward control flow"), "{err}");
}

#[test]
fn f16_interp_descriptor_decodes_attr_channel_and_high() {
    // attr=1, chan=0, high=1 (0b00000001 01_00 00_01)
    assert_eq!(Emitter::f16_interp_descriptor(0x00020101), (1, 0, true));
    // attr=1, chan=1, high=0
    assert_eq!(Emitter::f16_interp_descriptor(0x00020041), (1, 1, false));
    // attr=1, chan=0, high=0
    assert_eq!(Emitter::f16_interp_descriptor(0x00020001), (1, 0, false));
}

#[test]
fn typed_buffer_16_16_16_16_uint_channels_read_16_bit_words_from_two_dwords() {
    // 8-byte element: [lo: ch0, ch1] [hi: ch2, ch3].
    assert_eq!(tbuf_16_16_16_16_uint_channel(0, "packed", "packed1"), "((packed >> 0u) & 65535u)");
    assert_eq!(tbuf_16_16_16_16_uint_channel(1, "packed", "packed1"), "((packed >> 16u) & 65535u)");
    assert_eq!(tbuf_16_16_16_16_uint_channel(2, "packed", "packed1"), "((packed1 >> 0u) & 65535u)");
    assert_eq!(tbuf_16_16_16_16_uint_channel(3, "packed", "packed1"), "((packed1 >> 16u) & 65535u)");
}

#[test]
fn ff1_b64_points_into_low_word_when_it_is_set() {
    assert_eq!(
        ff1_b64_expr("s2", "s3"),
        "(s2 != 0u ? uint(firstbitlow(s2)) : s3 != 0u ? 32u + uint(firstbitlow(s3)) : 0xffffffffu)"
    );
}

#[test]
fn flbit_b64_checks_high_word_before_low_word() {
    assert_eq!(
        flbit_b64_expr("s2", "s3"),
        "(s3 != 0u ? 31u - firstbithigh(s3) : s2 != 0u ? 63u - firstbithigh(s2) : ~0u)"
    );
}

#[test]
fn typed_buffer_extraction_masks_to_16_bits() {
    // Each channel is masked with 0xffff: no channel may leak into a wider one.
    for channel in 0..4u32 {
        let expr = tbuf_16_16_16_16_uint_channel(channel, "a", "b");
        assert!(expr.contains("& 65535u"), "channel {channel}: {expr}");
    }
}

#[test]
fn ff0_b32_inverts_operand_for_firstbitlow() {
    assert_eq!(ff0_b32_expr("s2"), "firstbitlow(~s2)");
}

#[test]
fn ff0_b64_checks_low_word_before_high_word() {
    assert_eq!(
        ff0_b64_expr("s2", "s3"),
        "(~s2 != 0u ? uint(firstbitlow(~s2)) : ~s3 != 0u ? 32u + uint(firstbitlow(~s3)) : 0xffffffffu)"
    );
}

// --- Masked prologue decode paths (int and floating) -------------------

fn masked_insn(mnemonic: &str, enc: Enc, dst: Vec<Opnd>, src: Vec<Opnd>) -> Insn {
    Insn {
        offset: 0x100,
        dwords: 1,
        enc,
        op: 0,
        raw: vec![0],
        mnemonic: mnemonic.to_string(),
        dst,
        src,
        aux: Default::default(),
    }
}

fn masked_shuffle(mnemonic: &str, selector: u32, acc: u8, ctrl: u32) -> Insn {
    let mut ins = masked_insn(mnemonic, Enc::Vop2, vec![Opnd::Vgpr(acc)], vec![Opnd::Raw(selector as u16), Opnd::Vgpr(acc)]);
    ins.raw = vec![0x4a0606e9, ctrl << 8 | u32::from(acc)];
    ins
}

fn masked_readlane(acc: u8, lane: u32, dst: u8) -> Insn {
    let mut ins = masked_insn(
        "v_readlane_b32",
        Enc::Vop3,
        vec![Opnd::Vgpr(dst)],
        vec![Opnd::Vgpr(acc), Opnd::ConstInt(lane as i64), Opnd::Sgpr(0)],
    );
    ins.raw = vec![0xd760_0000 | u32::from(dst), ((128 + lane) << 9) | 256 | u32::from(acc)];
    ins
}

fn masked_cross_row(mnemonic: &str, acc: u8, tmp: u8) -> Insn {
    masked_insn(mnemonic, Enc::Vop2, vec![Opnd::Vgpr(acc)], vec![Opnd::Vgpr(tmp), Opnd::Vgpr(acc)])
}

fn masked_perm(acc: u8, tmp: u8) -> Insn {
    masked_insn("v_permlanex16_b32", Enc::Vop3, vec![Opnd::Vgpr(tmp)], vec![Opnd::Vgpr(acc), Opnd::ConstInt(0), Opnd::ConstInt(0)])
}

fn masked_reduction_insns(
    width: u32,
    op: &str,
    data_reg: u8,
    float_bits: Option<u32>,
    ident_reg: u8,
) -> Vec<Insn> {
    let is_float = float_bits.is_some();
    let acc = 1u8;
    let ctrl = [0xde54c1u32, 0x936012, 0x000924, 0xff0940];
    let selector = [233u32, 233, 233, 250];
    let mut insns = Vec::new();
    // Prologue: saveexec + (literal identity move | AND) + CNDMASK.
    if width == 32 {
        insns.push(masked_insn("s_or_saveexec_b32", Enc::Sop1, vec![Opnd::Sgpr(0)], vec![Opnd::ConstInt(-1)]));
    } else {
        insns.push(masked_insn("s_or_saveexec_b64", Enc::Sop1, vec![Opnd::Sgpr(0)], vec![Opnd::ConstInt(-1)]));
    }
    if let Some(bits) = float_bits {
        insns.push(masked_insn("v_mov_b32", Enc::Vop1, vec![Opnd::Vgpr(ident_reg)], vec![Opnd::Literal(bits)]));
    }
    if width == 32 {
        insns.push(masked_insn("s_and_b32", Enc::Sopc, vec![Opnd::Sgpr(2)], vec![Opnd::Sgpr(26), Opnd::Sgpr(0)]));
    } else {
        insns.push(masked_insn("s_and_b64", Enc::Sopc, vec![Opnd::Sgpr(2)], vec![Opnd::Sgpr(80), Opnd::Sgpr(0)]));
    }
    let cndmask_src = if is_float {
        vec![Opnd::Vgpr(ident_reg), Opnd::Vgpr(data_reg), Opnd::Sgpr(2)]
    } else {
        vec![Opnd::ConstInt(0), Opnd::Vgpr(data_reg), Opnd::Sgpr(2)]
    };
    insns.push(masked_insn("v_cndmask_b32", Enc::Vop2, vec![Opnd::Vgpr(acc)], cndmask_src));
    for (i, (&c, &s)) in ctrl.iter().zip(selector.iter()).enumerate() {
        let _ = i;
        insns.push(masked_shuffle(op, s, acc, c));
    }
    insns.push(masked_perm(acc, ident_reg + 4));
    insns.push(masked_cross_row(op, acc, ident_reg + 4));
    if width == 64 {
        insns.push(masked_readlane(acc, 32, ident_reg + 3));
        insns.push(masked_insn(op, Enc::Vop2, vec![Opnd::Vgpr(acc)], vec![Opnd::Sgpr(ident_reg + 3), Opnd::Vgpr(acc)]));
    }
    insns.push(masked_readlane(acc, 0, ident_reg + 2));
    if width == 32 {
        insns.push(masked_insn("s_mov_b32", Enc::Sop1, vec![Opnd::ExecLo], vec![Opnd::Sgpr(0)]));
    } else {
        insns.push(masked_insn("s_mov_b64", Enc::Sop1, vec![Opnd::ExecLo], vec![Opnd::Sgpr(0)]));
    }
    insns
}

#[test]
fn int_masked_add_prologue_lowers_full_block() {
    let mut insns = masked_reduction_insns(32, "v_add_nc_u32", 3, None, 1);
    lower_wave_reductions(&mut insns).expect("int masked prologue lowered");
    for ins in insns.iter().skip(3).take(4) {
        assert_eq!(ins.op, wave_sum::DPP_REDUCE, "shuffled instruction {ins:?}");
    }
    assert_eq!(insns[9].op, wave_sum::READ_LANE);
    assert_eq!(insns[9].mnemonic, "v_readlane_b32");
    assert_eq!(insns[9].dst, vec![Opnd::Sgpr(3)]);
}

#[test]
fn floating_min_masked_prologue_with_register_identity_lowers() {
    // +inf held in a VGPR materialized by a literal move.
    let mut insns = masked_reduction_insns(64, "v_min_f32", 4, Some(f32::INFINITY.to_bits()), 2);
    lower_wave_reductions(&mut insns).expect("floating masked prologue lowered");
    assert_eq!(insns[4].op, wave_sum::DPP_REDUCE);
    // Both readlanes became READ_LANE and returned scalars.
    assert_eq!(insns[10].op, wave_sum::READ_LANE);
    assert_eq!(insns[10].dst, vec![Opnd::Sgpr(5)]);
    assert_eq!(insns[12].op, wave_sum::READ_LANE);
    assert_eq!(insns[12].dst, vec![Opnd::Sgpr(4)]);
}

#[test]
fn floating_max_masked_prologue_with_register_identity_lowers() {
    // -inf held in a VGPR materialized by a literal move.
    let mut insns = masked_reduction_insns(32, "v_max_f32", 4, Some(f32::NEG_INFINITY.to_bits()), 2);
    lower_wave_reductions(&mut insns).expect("floating masked prologue lowered");
    assert_eq!(insns[4].op, wave_sum::DPP_REDUCE);
    assert_eq!(insns[10].op, wave_sum::READ_LANE);
    assert_eq!(insns[10].dst, vec![Opnd::Sgpr(4)]);
}

#[test]
fn floating_masked_prologue_rejects_unrelated_identity_register() {
    // The identity move lives in a different register than the CNDMASK
    // operand: must refuse, not guess.
    let mut insns = masked_reduction_insns(32, "v_min_f32", 4, Some(f32::INFINITY.to_bits()), 2);
    insns[1].dst = vec![Opnd::Vgpr(9)]; // identity materialized in the wrong register
    assert!(lower_wave_reductions(&mut insns).is_err());
}

fn lane_insn(mnemonic: &str, op: u32, dst: Vec<Opnd>, src: Vec<Opnd>) -> Insn {
    Insn {
        offset: 0x100,
        dwords: 1,
        enc: Enc::Vop3,
        op,
        raw: vec![0],
        mnemonic: mnemonic.to_string(),
        dst,
        src,
        aux: Default::default(),
    }
}

fn sgpr_lane_readlane(v: u8, lane: u8, dst: u8) -> Insn {
    lane_insn("v_readlane_b32", 0x360,
              vec![Opnd::Vgpr(dst)],
              vec![Opnd::Vgpr(v), Opnd::Sgpr(lane), Opnd::Sgpr(0)])
}

fn const_lane_readlane(v: u8, lane: i64, dst: u8) -> Insn {
    lane_insn("v_readlane_b32", 0x360,
              vec![Opnd::Vgpr(dst)],
              vec![Opnd::Vgpr(v), Opnd::ConstInt(lane), Opnd::Sgpr(0)])
}

fn spill_writelane(v: u8, lane: i64, value: u8) -> Insn {
    lane_insn("v_writelane_b32", 0x361,
              vec![Opnd::Vgpr(v)],
              vec![Opnd::Sgpr(value), Opnd::ConstInt(lane), Opnd::Sgpr(0)])
}

#[test]
fn sgpr_lane_readlane_becomes_group_read_when_group_reads() {
    let mut insns = vec![sgpr_lane_readlane(4, 2, 7)];
    lower_lane_spills(&mut insns, false, true).expect("group read");
    assert_eq!(insns[0].mnemonic, "tb_group_readlane");
    assert_eq!(insns[0].dst, vec![Opnd::Sgpr(7)]);
    assert_eq!(insns[0].src, vec![Opnd::Vgpr(4), Opnd::Sgpr(2)]);
}

#[test]
fn sgpr_lane_readlane_refused_without_group_reads() {
    let mut insns = vec![sgpr_lane_readlane(4, 2, 7)];
    let err = lower_lane_spills(&mut insns, false, false).expect_err("refused");
    assert!(err.contains("non-constant lane operand"));
}

#[test]
fn sgpr_lane_readlane_refused_on_spill_area() {
    // A vreg written by v_writelane is slotted per constant lane; a dynamic
    // lane cannot address that slot, so it must stay refused.
    let mut insns = vec![spill_writelane(4, 0, 2), sgpr_lane_readlane(4, 2, 7)];
    let err = lower_lane_spills(&mut insns, false, true).expect_err("refused");
    assert!(err.contains("non-constant lane operand"));
}

#[test]
fn const_lane_readlane_still_group_read_when_group_reads() {
    let mut insns = vec![const_lane_readlane(4, 3, 7)];
    lower_lane_spills(&mut insns, false, true).expect("group read");
    assert_eq!(insns[0].mnemonic, "tb_group_readlane");
    assert_eq!(insns[0].dst, vec![Opnd::Sgpr(7)]);
    assert_eq!(insns[0].src, vec![Opnd::Vgpr(4), Opnd::ConstInt(3)]);
}

#[test]
fn floating_max_masked_prologue_rejects_immediate_select_identity() {
    // A select with an immediate identity does not match the register-held
    // floating shape.
    let mut insns = masked_reduction_insns(32, "v_max_f32", 4, Some(f32::NEG_INFINITY.to_bits()), 2);
    insns[3].src[0] = Opnd::ConstInt(f32::NEG_INFINITY.to_bits() as i64);
    assert!(lower_wave_reductions(&mut insns).is_err());
}
