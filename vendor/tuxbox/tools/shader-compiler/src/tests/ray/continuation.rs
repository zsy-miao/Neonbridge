use super::{continuation, resources};
use crate::rootsig::{RootParam, RootParamKind, RootRange, RootSig};

const RETURN: u32 = 0xbe80_201e;
const NOP: u32 = 0xbf80_0000;

fn lift(words: &[u32]) -> Result<String, String> {
    let bytes: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let mut out = String::new();
    continuation(
        &bytes,
        0,
        bytes.len(),
        &mut out,
        &resources::Bindings::default(),
        None,
        false,
    )?;
    Ok(out)
}

#[test]
fn nested_back_edges_keep_the_first_entry_to_the_inner_loop() {
    let lowered = lift(&[
        0xbf84_0001,
        NOP, // 0 -> 8 or 4
        0xbf82_0001,
        NOP,         // 8 -> 16, skipping 12 on the first iteration
        0xbf85_fffe, // 16 -> 12 or 20
        0xbf85_fffb, // 20 -> 4 or 24
        RETURN,
    ])
    .unwrap();
    assert!(!lowered.contains("switch"));
    assert_eq!(lowered.matches("[loop] do").count(), 2);
    let edges = [
        "if (scc == 0u) nextBlock = 8u;",
        "if (nextBlock <= 20u) { [loop] do",
        "nextBlock = 16u;\n}",
        "if (nextBlock <= 16u) { [loop] do",
        "if (scc != 0u) nextBlock = 12u;",
        "while (nextBlock >= 12u && nextBlock <= 16u)",
        "if (scc != 0u) nextBlock = 4u;",
        "while (nextBlock >= 4u && nextBlock <= 20u)",
    ];
    let mut rest = lowered.as_str();
    for edge in edges {
        rest = rest.split_once(edge).expect(edge).1;
    }
}

#[test]
fn crossing_back_edges_dispatch_and_invalid_edges_fail() {
    // Reachable back edges [0,8] and [4,12] overlap without nesting.
    let crossing = lift(&[NOP, NOP, 0xbf85_fffd, 0xbf85_fffd, RETURN]).unwrap();
    assert!(crossing.contains("switch (nextBlock)"));
    assert_eq!(crossing.matches("case ").count(), 4);
    assert!(lift(&[]).is_err());
    assert!(lift(&[0xbf82_0001, RETURN]).is_err()); // beyond final instruction
    assert!(lift(&[0xbf82_0001, 0x8036_ff36, 0x1234, RETURN]).is_err()); // literal operand
    assert!(lift(&[NOP]).is_err()); // fallthrough
}

#[test]
fn compact_f32_to_f16_preserves_the_upper_word() {
    // v_cvt_f16_f32 v4,v1; v_cvt_f16_f32 v4,v4. Even with aliased
    // operands, conversion reads the old word before replacing its low half.
    let hlsl = lift(&[0x7e08_1501, 0x7e08_1504, RETURN]).unwrap();
    assert!(hlsl.contains("v[4] = (v[4] & 0xffff0000u) | tb_ray_half(asfloat(v[1]));"));
    assert!(hlsl.contains("v[4] = (v[4] & 0xffff0000u) | tb_ray_half(asfloat(v[4]));"));
}

#[test]
fn sdwa_f32_to_f16_places_the_converted_bits_in_the_destination() {
    let convert = |extension| lift(&[0x7e08_14f9, extension, RETURN]);
    // BYTE_1 PRESERVE, WORD_0 SEXT, and DWORD PAD.
    let preserve = convert(0x0006_1101).unwrap();
    assert!(preserve.contains("(v[4] & 0xffff00ffu)"));
    assert!(preserve.contains("(tb_ray_half(asfloat(v[1])) & 255u) << 8u"));
    let extend = convert(0x0006_0c01).unwrap();
    assert!(extend.contains("asint(tb_ray_half(asfloat(v[1])) << 16u) >> 16u"));
    assert!(convert(0x0006_0601).unwrap().contains("v[4] = tb_ray_half(asfloat(v[1]));"));
    // Reserved selector, reserved unused mode, and unhandled output modifiers.
    for extension in [0x0006_0701, 0x0006_1801, 0x0006_2001, 0x0006_4001] {
        assert!(convert(extension).is_err());
    }
}

#[test]
fn sdwa_integer_conversions_apply_float_modifiers_before_packing() {
    // Convert -abs(v1) to signed, replacing only WORD_0 in v4.
    let signed = lift(&[0x7e08_10f9, 0x0036_1401, RETURN]).unwrap();
    assert!(signed.contains("(v[4] & 0xffff0000u)"));
    assert!(signed.contains("tb_cvt_i32_f32(asfloat(((v[1] & 0x7fffffffu) ^ 0x80000000u)))"));
    // Convert v1 to unsigned, replacing only BYTE_3 in v4.
    let unsigned = lift(&[0x7e08_0ef9, 0x0006_1301, RETURN]).unwrap();
    assert!(unsigned.contains("(v[4] & 0x00ffffffu)"));
    assert!(unsigned.contains("(tb_cvt_u32_f32(asfloat(v[1])) & 255u) << 24u"));
}

#[test]
fn sdwa_integer_to_float_extends_before_conversion_and_clamps_before_writing() {
    // Signed WORD_1 source: a negative high half must convert to a negative float.
    let signed = lift(&[0x7e08_0af9, 0x000d_0601, RETURN]).unwrap();
    assert!(signed.contains("float(asint("));
    assert!(signed.contains("asint((v[1]) << 0u) >> 16u"));
    // Unsigned DWORD source: clamp the float, then copy its low byte to BYTE_3.
    let unsigned = lift(&[0x7e08_0cf9, 0x0006_3301, RETURN]).unwrap();
    assert!(unsigned.contains("(v[4] & 0x00ffffffu)"));
    assert!(unsigned.contains("(asuint(saturate(float(v[1]))) & 255u) << 24u"));
    assert!(lift(&[0x7e08_0cf9, 0x0006_4601, RETURN]).is_err()); // output scale
}

#[test]
fn widening_an_inline_half_uses_half_precision_before_subword_selection() {
    let compact = lift(&[0x7e08_16f2, RETURN]).unwrap(); // 1.0
    assert!(compact.contains("tb_ray_widen_half(f32tof16(asfloat(0x3f800000u)))"));
    let high = lift(&[0x7e08_16f9, 0x0085_06f2, RETURN]).unwrap();
    assert!(high.contains("(f32tof16(asfloat(0x3f800000u))) >> 16u"));
    let packed = lift(&[0x7e08_16f9, 0x0036_1501, RETURN]).unwrap();
    assert!(packed.contains("(v[4] & 0x0000ffffu)"));
    assert!(packed.contains("tb_ray_widen_half(((v[1] & 0x7fffu) ^ 0x8000u))"));
}

#[test]
fn sdwa_clamped_half_writes_only_launch_mode_independent_results() {
    let convert = |ext| lift(&[0x7e08_16f9, ext, RETURN]);
    // The low byte of any widened and saturated half is zero.
    let byte = convert(0x0006_3201).unwrap(); // BYTE_2, PRESERVE
    assert!(byte.contains("(v[4] & 0xff00ffffu)"));
    assert!(!byte.contains("tb_ray_widen_half"));
    let constant = convert(0x0086_26f0).unwrap(); // 0.5, DWORD
    assert!(constant.contains("saturate(tb_ray_widen_half("));
    // A register can contain a NaN whose clamped result depends on DX10_CLAMP.
    for ext in [0x0006_2601, 0x0015_3501, 0x0007_3201, 0x0006_7201] {
        assert!(convert(ext).is_err());
    }
}

#[test]
fn sdwa_binary_ops_write_the_selected_byte_after_evaluating_both_sources() {
    // v4 = op(v1, v5), BYTE_1 with UNUSED_PRESERVE.
    for (opcode, value) in [
        (0x3608_0af9, "v[1] & v[5]"),
        (0x3808_0af9, "v[1] | v[5]"),
        (0x3a08_0af9, "v[1] ^ v[5]"),
        (0x4a08_0af9, "v[1] + v[5]"),
        (0x4c08_0af9, "v[1] - v[5]"),
        (0x3408_0af9, "v[5] << (v[1] & 31u)"),
        (0x2c08_0af9, "v[5] >> (v[1] & 31u)"),
        (0x3008_0af9, "asuint(asint(v[5]) >> (v[1] & 31u))"),
    ] {
        let hlsl = lift(&[opcode, 0x0606_1101, RETURN]).unwrap();
        assert!(hlsl.contains("(v[4] & 0xffff00ffu)"));
        assert!(hlsl.contains(&format!("(({value}) & 255u) << 8u")));
    }
    // Unsupported output scale and reserved source selector.
    assert!(lift(&[0x3608_0af9, 0x0606_5101, RETURN]).is_err());
    assert!(lift(&[0x3608_0af9, 0x0706_1101, RETURN]).is_err());
}

#[test]
fn mul24_selects_signedness_result_word_and_saturation() {
    for (encoding, flags) in [
        (vec![0x1208_0b01], "true, false, false"),
        (vec![0x1808_0b01], "false, true, false"),
        (vec![0xd50b_8004, 0x0002_0b01], "false, false, true"),
        (vec![0x1408_0af9, 0x0606_1101], "true, true, false"),
    ] {
        let mut words = encoding;
        words.push(RETURN);
        let shader = lift(&words).unwrap();
        assert!(shader.contains(&format!("tb_ray_mul24(v[1], v[5], {flags})")));
        if words[0] == 0x1408_0af9 {
            assert!(shader.contains("v[4] & 0xffff00ffu"));
        }
    }
    // VOP3 integer multiply has no output scaling.
    assert!(lift(&[0xd50b_0004, 0x0802_0b01, RETURN]).is_err());
}

#[test]
fn sdwa_minmax_compares_full_values_before_writing_the_selected_byte() {
    for (opcode, expr) in [
        (0x2208_0af9, "asuint(min(asint(v[1]), asint(v[5])))"),
        (0x2408_0af9, "asuint(max(asint(v[1]), asint(v[5])))"),
        (0x2608_0af9, "min(v[1], v[5])"),
        (0x2808_0af9, "max(v[1], v[5])"),
    ] {
        let hlsl = lift(&[opcode, 0x0606_1101, RETURN]).unwrap();
        assert!(hlsl.contains(&format!("(v[4] & 0xffff00ffu) | (({expr} & 255u) << 8u)")));
    }
}

#[test]
fn consecutive_vector_moves_share_the_guard_until_exec_changes() {
    let move_v29 = 0x7e3a_0280; // v_mov_b32 v29, 0
    let move_v45 = 0x7e5a_0280; // v_mov_b32 v45, 0
    let restore_exec = 0xbefe_0335; // s_mov_b32 exec_lo, s53
    let lowered = lift(&[move_v29, NOP, move_v45, restore_exec, move_v45, RETURN]).unwrap();
    assert_eq!(lowered.matches("if (exec != 0u)").count(), 2);
    let (before, after) = lowered.split_once("}\nexec = s[53];\n").unwrap();
    assert!(before.contains("v[29] =") && before.contains("v[45] ="));
    assert!(after.starts_with("if (exec != 0u) {\nv[45] ="));
}

#[test]
fn scalar_bit_operations_preserve_their_native_condition_codes() {
    let reverse = lift(&[3199470388, RETURN]).unwrap();
    assert!(reverse.contains("s[52] = reversebits(s[52]);"));
    assert!(!reverse.contains("scc ="));
    let shift = lift(&[2436071731, RETURN]).unwrap();
    assert!(shift.contains("asint(s[51]) >>"));
    assert!(shift.contains("scc = (s[51] != 0u)"));
    // S_BFE_U32 accepts a seven-bit width. V_BFE_U32 takes five bits.
    let extract = lift(&[0x938b_0300, 0xd548_0001, 0x040a_0300, RETURN]).unwrap();
    assert!(extract.contains("tb_ray_bfe(s[0], s[3], ((s[3] >> 16) & 127u))"));
    assert!(extract.contains("scc = (s[11] != 0u)"));
    assert!(extract.contains("tb_ray_bfe(v[0], v[1], (v[2] & 31u))"));
    assert!(lift(&[3128952961, 0, RETURN]).is_ok());
    assert!(lift(&[3128952961, 1, RETURN]).is_err());
    let count = lift(&[2268364394, 3199471412, 2234810497, RETURN]).unwrap();
    assert!(count.contains("WaveActiveBallot(s[52] != 0u)"));
    assert!(count.contains("votes[WaveGetLaneIndex() >> 5u]"));
    assert!(lift(&[3199471412, 2234810497, RETURN]).is_err());
    assert!(lift(&[2268364394, 3199471412, RETURN]).is_err());
}

#[test]
fn scratch_writes_preserve_partial_words_and_private_spills_are_separate() {
    let half = lift(&[3764916284, 67121959, RETURN]).unwrap();
    assert!(half.contains("spill[i] & ~(0xffffu << shift)"));
    assert!(half.contains("if (shift == 24u) spill[i + 1u] = (spill[i + 1u] & ~255u)"));
    let save = [3765436420, 83888128]; // private scratch[4] = v8
    assert!(lift(&[save[0], save[1], RETURN]).is_err());
    let restored = lift(&[3196388128, save[0], save[1], 3761242116, 83892736, RETURN]).unwrap();
    assert!(restored.contains("privateWord1 = v[8];"));
    assert!(restored.contains("v[26] = privateWord1;"));
}

#[test]
fn cold_branch_islands_keep_spills_on_the_taken_edge() {
    let hlsl = lift(&[
        3196388128,  // s5 = s32, private scratch base
        0xbf85_0002, // branch on SCC to the island at 16
        NOP,
        RETURN,
        3765436420,
        83888128,    // private scratch[4] = v8
        0xbf82_fffc, // jump back to RETURN at 12
    ])
    .unwrap();
    assert!(!hlsl.contains("switch") && !hlsl.contains("[loop]"));
    let taken = hlsl.split_once("if (scc != 0u) {").unwrap().1;
    assert!(taken.starts_with(" if (exec != 0u) { privateWord1 = v[8];"));
    assert!(taken.contains("} nextBlock = 12u; }"));
    assert_eq!(hlsl.matches("privateWord1 = v[8]").count(), 1);
}

#[test]
fn ray_export_uses_declared_register_space_and_table_offsets() {
    let table = RootParam {
        visibility: 0,
        kind: RootParamKind::Table(vec![
            RootRange {
                range_type: 0,
                num: 2,
                base_register: 7,
                space: 3,
                offset_in_table: 2,
            },
            RootRange {
                range_type: 1,
                num: 1,
                base_register: 5,
                space: 6,
                offset_in_table: u32::MAX,
            },
        ]),
    };
    let root = RootSig {
        version: 2,
        flags: 0,
        params: vec![table],
        num_static_samplers: 0,
        static_samplers: vec![],
    };
    let binding = super::super::binding;
    assert_eq!(
        binding(&root, 0, 3, 0).as_deref(),
        Some("register(t8, space3)")
    );
    assert_eq!(
        binding(&root, 0, 4, 1).as_deref(),
        Some("register(u5, space6)")
    );
    assert!(binding(&root, 0, 4, 0).is_none());
    assert!(binding(&root, 0, 0, 0).is_none());
}
