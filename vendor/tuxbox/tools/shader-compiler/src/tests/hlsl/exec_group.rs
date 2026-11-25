use super::group;
use crate::{ShaderExtract, Stage};

fn compute_extract(words: &[u32]) -> ShaderExtract {
    let isa: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    ShaderExtract { flat_inputs: Vec::new(), flat_outputs: Vec::new(),
        stage: Stage::Compute, shader_model: (6, 0), required_features: None,
        bitcode_stripped: true, fspx_config: None, ps09_pixel_inputs: None,
        fssm: Vec::new(), entry: Some("main".into()), isa_data: isa.clone(),
        isa, isa_offset_in_fssm: 0, psv_threads: None, isa_preamble: [0, 64, 1, 1],
        has_input_sig: false, has_output_sig: false, has_psv0: false, has_root_sig: false,
        input_sig: Vec::new(), output_sig: Vec::new(), resources: Vec::new(),
        gs_info: None, mesh_info: None, primitive_sig: Vec::new(), root_sig: None,
    }
}

#[test]
fn bitwise_quad_exchange_combines_the_selected_source_with_the_local_operand() {
    for (op, symbol) in [(0x3808_04fa, "|"), (0x3608_04fa, "&"), (0x3a08_04fa, "^")] {
        for control in [0xff0c_b101, 0xff04_b101] {
            let mut ex = compute_extract(&[op, control, 0xbf81_0000]);
            let hlsl = super::super::translate(&ex).unwrap().hlsl;
            assert!(hlsl.contains("WaveReadLaneAt(v1,"), "{hlsl}");
            assert!(hlsl.contains(&format!("v4 = q0 {symbol} v2;")), "{hlsl}");

            ex.psv_threads = Some([1, 1, 1]);
            assert!(super::super::translate(&ex).is_err());
        }
    }
}

#[test]
fn bitwise_quad_exchange_with_inactive_source_obeys_bound_control() {
    // gfx1030: v_{or,and,xor}_b32_dpp v4, v1, v2 quad_perm:[1,0,3,2], FI=0.
    for (op, symbol) in [(0x3808_04fa, "|"), (0x3608_04fa, "&"), (0x3a08_04fa, "^")] {
        let ex = compute_extract(&[op, 0xff00_b101, 0xbf81_0000]);
        let keep = super::super::translate(&ex).unwrap().hlsl;
        assert!(keep.contains("WaveReadLaneAt(uint2(v1, exec_lo & 1u),"), "{keep}");
        assert!(keep.contains(&format!("q0active ? (q0 {symbol} v2) : v4")), "{keep}");

        let ex = compute_extract(&[op, 0xff08_b101, 0xbf81_0000]);
        let zero = super::super::translate(&ex).unwrap().hlsl;
        assert!(zero.contains("WaveReadLaneAt((exec_lo & 1u) != 0u ? v1 : 0u,"), "{zero}");
        assert!(zero.contains(&format!("v4 = q0 {symbol} v2;")), "{zero}");
    }
}

#[test]
fn quad_subtractions_permute_only_the_first_operand() {
    // gfx1030 quad_perm:[1,0,3,2], full row and bank masks.
    for (op, expression) in [(0x4c08_04fa, "q0 - v2"), (0x4e08_04fa, "v2 - q0")] {
        for control in [0xff00_b101, 0xff04_b101, 0xff08_b101, 0xff0c_b101] {
            let ex = compute_extract(&[op, control, 0xbf81_0000]);
            let hlsl = super::super::translate(&ex).unwrap().hlsl;
            assert!(hlsl.contains("WaveReadLaneAt("), "{hlsl}");
            let write = if control == 0xff00_b101 {
                format!("v4 = q0active ? ({expression}) : v4;")
            } else {
                format!("v4 = {expression};")
            };
            assert!(hlsl.contains(&write), "{hlsl}");
        }
    }
}

#[test]
fn quad_shifts_take_the_count_from_the_selected_lane() {
    // gfx1030 quad_perm:[1,0,3,2], full row and bank masks.
    for (opcode, value) in [
        (0x3408_04fa, "v2 << (q0 & 31u)"),
        (0x2c08_04fa, "v2 >> (q0 & 31u)"),
        (0x3008_04fa, "asuint(asint(v2) >> (q0 & 31u))"),
    ] {
        for control in [0xff00_b101, 0xff04_b101, 0xff08_b101, 0xff0c_b101] {
            let shader = compute_extract(&[opcode, control, 0xbf81_0000]);
            let hlsl = super::super::translate(&shader).unwrap().hlsl;
            let expected = if control == 0xff00_b101 {
                format!("v4 = q0active ? ({value}) : v4;")
            } else {
                format!("v4 = {value};")
            };
            assert!(hlsl.contains("WaveReadLaneAt(") && hlsl.contains(&expected), "{hlsl}");
        }
    }
}

#[test]
fn quad_min_max_compare_the_peer_with_the_local_operand() {
    for (opcode, result) in [
        (0x2208_04fa, "asuint(min(asint(q0), asint(v2)))"),
        (0x2408_04fa, "asuint(max(asint(q0), asint(v2)))"),
        (0x2608_04fa, "min(q0, v2)"),
        (0x2808_04fa, "max(q0, v2)"),
    ] {
        let shader = compute_extract(&[opcode, 0xff0c_b101, 0xbf81_0000]);
        let hlsl = super::super::translate(&shader).unwrap().hlsl;
        assert!(hlsl.contains("WaveReadLaneAt(v1,") && hlsl.contains(&format!("v4 = {result};")), "{hlsl}");
    }
}

#[test]
fn quad_mul24_uses_the_same_product_as_an_ordinary_multiply() {
    for opcode in [0x1208_04fa, 0x1408_04fa, 0x1608_04fa, 0x1808_04fa] {
        let plain = compute_extract(&[(opcode & !0x1ff) | 0x101, 0xbf81_0000]);
        let plain_hlsl = super::super::translate(&plain).unwrap().hlsl;
        let expression = plain_hlsl.lines()
            .filter_map(|line| line.split_once("v4 = ").map(|(_, rhs)| rhs))
            .find(|rhs| rhs.contains("v1") && rhs.contains("v2"))
            .unwrap_or_else(|| panic!("missing multiply in {plain_hlsl}"))
            .replace("v1", "q0");
        let quad = compute_extract(&[opcode, 0xff0c_b101, 0xbf81_0000]);
        let hlsl = super::super::translate(&quad).unwrap().hlsl;
        assert!(hlsl.contains("WaveReadLaneAt(v1,") && hlsl.contains(&format!("v4 = {expression}")), "{hlsl}");
    }
}

#[test]
fn quad_integer_operations_permute_only_the_first_operand() {
    // gfx1030 quad_perm:[1,0,3,2], full row and bank masks.
    for (op, expression) in [
        (0x4a08_04fa, "q0 + v2"),
        (0x4c08_04fa, "q0 - v2"),
        (0x4e08_04fa, "v2 - q0"),
        (0x3c08_04fa, "~(q0 ^ v2)"),
    ] {
        for control in [0xff00_b101, 0xff04_b101, 0xff08_b101, 0xff0c_b101] {
            let ex = compute_extract(&[op, control, 0xbf81_0000]);
            let hlsl = super::super::translate(&ex).unwrap().hlsl;
            assert!(hlsl.contains("WaveReadLaneAt("), "{hlsl}");
            let write = if control == 0xff00_b101 {
                format!("v4 = q0active ? ({expression}) : v4;")
            } else {
                format!("v4 = {expression};")
            };
            assert!(hlsl.contains(&write), "{hlsl}");
        }
    }
}

#[test]
fn quad_cndmask_selects_with_the_destination_lanes_vcc() {
    // v_cmp_ne_u32 vcc_lo, 0, v3; v_cndmask_b32_dpp v4, v1, v2.
    let quad = 0x0208_04fa;
    let compare = 0x7d8a_0680;
    for control in [0xff00_b101, 0xff04_b101, 0xff08_b101, 0xff0c_b101] {
        let shader = compute_extract(&[compare, quad, control, 0xbf81_0000]);
        let hlsl = super::super::translate(&shader).unwrap().hlsl;
        assert!(hlsl.contains("(vcc_lo & 1u) != 0u ? v2 : q1"), "{hlsl}");
    }
    // A branch can bypass the compare. A scalar write can also replace
    // the per-lane predicate with a full wave mask.
    for prefix in [vec![0xbf84_0001, compare], vec![compare, 0xbeea_0381]] {
        let mut words = prefix;
        words.extend([quad, 0xff0c_b101, 0xbf81_0000]);
        let err = super::super::translate(&compute_extract(&words)).err().expect("unproven VCC accepted");
        assert!(err.contains("requires a known per-lane VCC predicate"), "{err}");
    }
    // After a scalar mask, a new compare establishes a fresh lane predicate.
    let words = [0xbeea_0381, compare, quad, 0xff0c_b101, quad, 0xff0c_b101, 0xbf81_0000];
    let hlsl = super::super::translate(&compute_extract(&words)).unwrap().hlsl;
    assert_eq!(hlsl.matches("(vcc_lo & 1u) != 0u ? v2 : q").count(), 2);
}

#[test]
fn quad_fp32_arithmetic_keeps_operand_order_and_inactive_destinations() {
    for (op, expression) in [
        (0x0608_04fa, "asuint(asfloat(q0) + asfloat(v2))"),
        (0x0808_04fa, "asuint(asfloat(q0) - asfloat(v2))"),
        (0x0a08_04fa, "asuint(asfloat(v2) - asfloat(q0))"),
        (0x1008_04fa, "asuint(asfloat(q0) * asfloat(v2))"),
        (0x0e08_04fa, "asuint(tb_mull(asfloat(q0), asfloat(v2)))"),
        (0x1e08_04fa, "tb_quad_extrema(q0, v2, false, 0xc0u)"),
        (0x2008_04fa, "tb_quad_extrema(q0, v2, true, 0xc0u)"),
        (0x5e08_04fa, "(tb_f16_rtz(asfloat(q0)) | (tb_f16_rtz(asfloat(v2)) << 16))"),
    ] {
        for control in [0xff00_b101, 0xff04_b101, 0xff08_b101, 0xff0c_b101] {
            let shader = compute_extract(&[op, control, 0xbf81_0000]);
            let hlsl = super::super::translate(&shader).unwrap().hlsl;
            let value = if control == 0xff00_b101 {
                format!("q0active ? ({expression}) : v4")
            } else {
                expression.to_owned()
            };
            assert!(hlsl.contains("WaveReadLaneAt(") && hlsl.contains(&format!("v4 = {value};")), "{hlsl}");
        }
    }
}

#[test]
fn extrema_quad_can_start_a_saved_exec_reduction_or_stand_alone() {
    // Save EXEC, materialize +infinity, then select the input for active lanes.
    let prologue = [0xbe94_2980, 0x7e0c_02ff, 0x7f80_0000, 0xd501_0004, 0x0052_0306];
    let quad = [0x1e08_08fa, 0xff04_b104];
    let mut words = prologue.to_vec();
    words.extend(quad);
    words.extend([0xd760_0017, 0x0001_0104, 0xbefe_0414, 0xbf81_0000]);
    let mut insns = crate::rdna2::disassemble(&compute_extract(&words).isa);
    super::super::lower_wave_reductions(&mut insns).unwrap();
    assert!(insns.iter().any(super::super::is_quad_exchange));

    words.truncate(prologue.len() + quad.len());
    // Fold the remaining pairs, octets, rows and the other half-wave.
    words.extend([0x1e08_08fa, 0xff04_4e04, 0x1e08_08fa, 0xff05_2404,
        0x1e08_08fa, 0xff05_2804, 0xd778_0005, 0x0201_0104, 0x1e08_0905,
        0xd760_0016, 0x0001_4104, 0x1e08_0816, 0xd760_0017, 0x0001_0104,
        0xbefe_0414, 0xbf81_0000]);
    let hlsl = super::super::translate(&compute_extract(&words)).unwrap().hlsl;
    assert!(hlsl.contains("tb_swizzle[(tb_lane & ~63u) | (0x20u & 63u)]"), "{hlsl}");
    assert!(hlsl.contains("tb_swizzle[(tb_lane & ~63u) | (0x0u & 63u)]"), "{hlsl}");
    assert!(!hlsl.contains("WaveActiveMin("), "{hlsl}");
    // Rotating right four lanes reads from the previous quartet.
    assert!(hlsl.contains("lane - 4u) & 15u)"), "{hlsl}");
    // Only selected destination rows and banks consume the rotated value.
    let mut masked = words.clone();
    masked[prologue.len() + 5] = 0xa505_2404;
    let hlsl = super::super::translate(&compute_extract(&masked)).unwrap().hlsl;
    assert!(hlsl.contains("0xau >> ((tb_lane >> 4u) & 3u)"), "{hlsl}");
    assert!(hlsl.contains("0x5u >> ((tb_lane >> 2u) & 3u)"), "{hlsl}");
    // The same two broadcasts end a tree that starts with DPP8 exchanges.
    words.splice(prologue.len()..prologue.len() + 8, [
        0x1e08_08e9, 0xde54_c104, 0x1e08_08e9, 0xb3e2_1a04,
        0x1e08_08e9, 0x688f_ac04, 0x1e08_08fa, 0xff05_4004]);
    let hlsl = super::super::translate(&compute_extract(&words)).unwrap().hlsl;
    assert!(hlsl.contains("tb_swizzle[(tb_lane & ~63u) | (0x20u & 63u)]"), "{hlsl}");
    assert!(!hlsl.contains("WaveActiveMin("), "{hlsl}");
    // A branch skipping NAND_SAVEEXEC could leave the DPP8 source inactive.
    words.insert(0, 0xbf85_0005);
    let error = super::super::translate(&compute_extract(&words)).err().unwrap();
    assert!(error.contains("branch enters a saved-EXEC extrema exchange"), "{error}");
}

#[test]
fn quad_fp32_arithmetic_modifies_each_input_before_combining() {
    // DPP16 quad_perm:[1,0,3,2], with negative absolute source 0
    // and negated source 1. FI=BC=0 preserves the previous destination.
    let peer = "asfloat(((q0 & 0x7fffffffu) ^ 0x80000000u))";
    let local = "asfloat((v2 ^ 0x80000000u))";
    for (opcode, arithmetic) in [
        (0x0608_04fa, format!("{peer} + {local}")),
        (0x0808_04fa, format!("{peer} - {local}")),
        (0x0a08_04fa, format!("{local} - {peer}")),
        (0x1008_04fa, format!("{peer} * {local}")),
        (0x0e08_04fa, format!("tb_mull({peer}, {local})")),
    ] {
        let shader = compute_extract(&[opcode, 0xff70_b101, 0xbf81_0000]);
        let hlsl = super::super::translate(&shader).unwrap().hlsl;
        assert!(hlsl.contains(&format!("v4 = q0active ? (asuint({arithmetic})) : v4;")), "{hlsl}");
    }
}

#[test]
fn quad_pack_applies_source_signs_before_rounding_each_half() {
    // v_cvt_pkrtz_f16_f32 v4, -|v1|, |v2| quad_perm:[1,0,3,2] fi:1
    let shader = compute_extract(&[0x5e08_04fa, 0xffb4_b101, 0xbf81_0000]);
    let hlsl = super::super::translate(&shader).unwrap().hlsl;
    let low = "tb_f16_rtz(asfloat(((q0 & 0x7fffffffu) ^ 0x80000000u)))";
    let high = "tb_f16_rtz(asfloat((v2 & 0x7fffffffu)))";
    assert!(hlsl.contains(&format!("v4 = ({low} | ({high} << 16));")), "{hlsl}");
}

#[test]
fn dpp16_extrema_apply_each_source_sign_after_selecting_its_lane() {
    // v_max_f32 v4, |v8|, -v2 quad_perm:[1,0,3,2] fi:1
    let shader = compute_extract(&[0x2008_04fa, 0xff64_b108, 0xbf81_0000]);
    let hlsl = super::super::translate(&shader).unwrap().hlsl;
    assert!(hlsl.contains("tb_quad_extrema((q0 & 0x7fffffffu), (v2 ^ 0x80000000u), true"), "{hlsl}");
}

#[test]
fn masked_quad_bitwise_writes_only_to_selected_compute_lanes() {
    for (op, symbol) in [(0x3808_04fa, "|"), (0x3a08_04fa, "^")] {
        let mut shader = compute_extract(&[op, 0x2500_b101, 0xbf81_0000]);
        shader.isa_preamble = [0, 4, 4, 2];
        let hlsl = super::super::translate(&shader).unwrap().hlsl;
        assert!(hlsl.contains("gtid.x + 4u * (gtid.y + 4u * gtid.z)"), "{hlsl}");
        assert!(hlsl.contains("0x2u >> ((tb_lane >> 4u) & 1u)"), "{hlsl}");
        assert!(hlsl.contains("0x5u >> ((tb_lane >> 2u) & 3u)"), "{hlsl}");
        assert!(hlsl.contains(&format!("&& q0active ? (q0 {symbol} v2) : v4")), "{hlsl}");

        shader.isa_preamble = [0, 64, 1, 1];
        let error = super::super::translate(&shader).err().unwrap();
        assert!(error.contains("known guest wave width"), "{error}");
    }
}

#[test]
fn asymmetric_row_masks_require_a_logical_lane_number() {
    // An identity select, a masked row rotation and the EXEC restore.
    let words = [0xbe94_2980, 0x7e0c_02ff, 0x7f80_0000, 0xd501_0004, 0x0052_0306,
        0x1e08_08fa, 0x8f05_2404, 0xbefe_0414, 0xbf81_0000];
    let mut ex = compute_extract(&words);
    ex.isa_preamble = [0, 8, 4, 1];
    let hlsl = super::super::translate(&ex).unwrap().hlsl;
    assert!(hlsl.contains("uint tb_lane = gtid.x + 8u * (gtid.y + 4u * gtid.z)"), "{hlsl}");
    ex.isa_preamble = [0, 8, 8, 1];
    let error = super::super::translate(&ex).err().unwrap();
    assert!(error.contains("known guest wave width"), "{error}");
}

#[test]
fn signed_quad_extrema_preserve_both_lane_reads() {
    // A single quad maximum of two negative absolute values precedes reads
    // from lanes 32 and 0. Other quads cannot contribute to either result.
    let shader = compute_extract(&[
        0xbe94_2980, 0x7e0c_02ff, 0x7f80_0000, 0xd501_0004, 0x0052_0906,
        0x2008_04fa, 0xfff8_9301, 0xd760_0016, 0x0001_4104, 0xd760_0017,
        0x0001_0104, 0xbefe_0414, 0xbf81_0000,
    ]);
    let hlsl = super::super::translate(&shader).unwrap().hlsl;
    assert!(hlsl.contains("tb_swizzle[(tb_lane & ~63u) | (0x20u & 63u)]"), "{hlsl}");
    assert!(hlsl.contains("tb_swizzle[(tb_lane & ~63u) | (0x0u & 63u)]"), "{hlsl}");
    assert!(!hlsl.contains("WaveActiveMax("), "{hlsl}");
}

#[test]
fn dpp_row_moves_mask_the_source_value_before_the_lane_read() {
    // gfx1030: v_mov_b32_dpp v4, v1 with row_mirror, row_half_mirror, or row_xmask:3.
    for (control, mask) in [(0xff09_4001, 15), (0xff09_4101, 7), (0xff09_6301, 3)] {
        let ex = compute_extract(&[0x7e08_02fa, control, 0xbf81_0000]);
        let hlsl = super::super::translate(&ex).unwrap().hlsl;
        assert!(hlsl.contains(&format!("WaveGetLaneIndex() ^ {mask}u")), "{hlsl}");
        assert!(hlsl.contains("WaveReadLaneAt((exec_lo & 1u) != 0u ? v1 : 0u,"), "{hlsl}");
    }
}

#[test]
fn dpp_row_share_reads_a_fixed_lane_in_each_row() {
    // gfx1030: v_mov_b32_dpp v4, v1 row_share:N, FI=0 and BC=1.
    for lane in 0..16 {
        let control = 0xff09_5001 | (lane << 8);
        let ex = compute_extract(&[0x7e08_02fa, control, 0xbf81_0000]);
        let hlsl = super::super::translate(&ex).unwrap().hlsl;
        assert!(hlsl.contains(&format!("(WaveGetLaneIndex() & ~15u) | {lane}u")), "{hlsl}");
        assert!(hlsl.contains("WaveReadLaneAt((exec_lo & 1u) != 0u ? v1 : 0u,"), "{hlsl}");
    }
}

#[test]
fn dpp_row_moves_fetch_preexisting_values_when_fi_is_set() {
    for op in [0x140, 0x141, 0x150, 0x167] {
        for bound in [0, 1] {
            let control = 0xff04_0001 | (op << 8) | (bound << 19);
            let ex = compute_extract(&[0x7e08_02fa, control, 0xbf81_0000]);
            let hlsl = super::super::translate(&ex).unwrap().hlsl;
            assert!(hlsl.contains("WaveReadLaneAt(v1, q0src)"), "{hlsl}");
        }
    }
}

#[test]
fn dpp_row_moves_keep_the_destination_when_the_source_is_inactive_by_default() {
    for control in [0xff01_4001, 0xff01_4101, 0xff01_5001, 0xff01_6701] {
        let ex = compute_extract(&[0x7e08_02fa, control, 0xbf81_0000]);
        let hlsl = super::super::translate(&ex).unwrap().hlsl;
        assert!(hlsl.contains("WaveReadLaneAt(uint2(v1, exec_lo & 1u), q0src)"), "{hlsl}");
        assert!(hlsl.contains("q0pair.y != 0u ? q0pair.x : v4"), "{hlsl}");
    }
}

#[test]
fn dpp_row_ror_wraps_the_source_index_within_its_row() {
    // gfx1030: v_mov_b32_dpp v4, v1 row_ror:N, FI=0 and BC=0.
    for distance in 1..16 {
        let control = 0xff01_2001 | (distance << 8);
        let ex = compute_extract(&[0x7e08_02fa, control, 0xbf81_0000]);
        let hlsl = super::super::translate(&ex).unwrap().hlsl;
        let lane = format!("(WaveGetLaneIndex() & ~15u) | ((WaveGetLaneIndex() - {distance}u) & 15u)");
        assert!(hlsl.contains(&lane), "{hlsl}");
    }
}

#[test]
fn dpp_row_shifts_apply_bound_control_outside_each_row() {
    for (control, offset, fallback) in [
        (0xff01_0101, "+ 1u", "v4"),
        (0xff09_0f01, "+ 15u", "0u"),
        (0xff0d_1101, "- 1u", "0u"),
        (0xff05_1f01, "- 15u", "v4"),
    ] {
        let ex = compute_extract(&[0x7e08_02fa, control, 0xbf81_0000]);
        let hlsl = super::super::translate(&ex).unwrap().hlsl;
        assert!(hlsl.contains(&format!("(WaveGetLaneIndex() & 15u) {offset}")), "{hlsl}");
        assert!(hlsl.contains(&format!("if (q0offset >= 16u) q0 = {fallback};")), "{hlsl}");
    }
}

#[test]
fn dpp_masks_select_destinations_after_the_lane_read() {
    // Row mask 5, bank mask 9: read v1 even from a masked bank, then update v4.
    for control in [0x5909_4001, 0x590c_1b01] { // row_mirror, quad_perm:[3,2,1,0]
        let ex = compute_extract(&[0x7e08_02fa, control, 0xbf81_0000]);
        let hlsl = super::super::translate(&ex).unwrap().hlsl;
        let (_, write) = hlsl.split_once("WaveReadLaneAt(").unwrap();
        assert!(write.contains("0x5u >> (WaveGetLaneIndex() >> 4u)"), "{hlsl}");
        assert!(write.contains("0x9u >> ((WaveGetLaneIndex() >> 2u) & 3u)"), "{hlsl}");
        assert!(write.contains("? q0 : v4;"), "{hlsl}");
    }
}

#[test]
fn asymmetric_row_mask_requires_a_known_guest_width_for_large_groups() {
    let mov = [0x7e08_02fa, 0x1f09_4001, 0xbf81_0000];
    let mut ex = compute_extract(&mov);
    assert!(matches!(super::super::translate(&ex), Err(msg) if msg.contains("guest wave width")));
    ex.psv_threads = Some([32, 1, 1]);
    assert!(super::super::translate(&ex).is_ok());

    // v_cmp_gt_u32 s3,v0,v1 establishes a single-word comparison result (wave32).
    let words = [&[0xd4c4_0003, 0x0202_0300][..], &mov].concat();
    assert!(super::super::translate(&compute_extract(&words)).is_ok());
}

#[test]
fn quad_move_with_fi_clear_obeys_bound_control_for_inactive_sources() {
    // v_mov_b32_dpp v4, v1 quad_perm:[1,2,3,0], with each BOUND_CTRL setting.
    for (control, read) in [
        (0xff00_3901, "WaveReadLaneAt(uint2(v1, exec_lo & 1u), q0src)"),
        (0xff08_3901, "WaveReadLaneAt((exec_lo & 1u) != 0u ? v1 : 0u, q0src)"),
    ] {
        let ex = compute_extract(&[0x7e08_02fa, control, 0xbf81_0000]);
        let hlsl = super::super::translate(&ex).unwrap().hlsl;
        assert!(hlsl.contains(read), "{hlsl}");
        if control == 0xff00_3901 {
            assert!(hlsl.contains("q0pair.y != 0u ? q0pair.x : v4"), "{hlsl}");
        }
    }
}

#[test]
fn first_active_election_uses_the_guest_wave_boundary() {
    // Full lane index, FF1 of EXEC, independent adjacent move, equality comparison.
    let mut ex = compute_extract(&[0xd765_0006, 0x0001_00c1, 0xd766_0006, 0x0002_0cc1,
        0xbe98_147e, 0xbe99_0387, 0x7d84_0c18, 0xbf81_0000]);
    let hlsl = super::super::translate(&ex).unwrap().hlsl;
    assert!(hlsl.contains("InterlockedMin(") && hlsl.contains("tb_lane & 63u"), "{hlsl}");
    ex.isa_preamble = [0, 8, 4, 1];
    let translated = super::super::translate(&ex).unwrap();
    assert_eq!(translated.required_wave_size, Some(32));
    assert!(translated.hlsl.contains("WaveActiveMin(") && !translated.hlsl.contains("InterlockedMin("));
}

#[test]
fn first_active_lane_survives_restoring_the_predicate() {
    // CMP, SAVEEXEC, FF1, restore, broadcast the saved index.
    let mut words = [0x7d86_00a3, 0xbe94_246a, 0xbe98_147e, 0xbefe_0414, 0x7e08_0218, 0xbf81_0000];
    let ex = compute_extract(&words);
    let shader = super::super::translate(&ex).unwrap();
    assert!(shader.hlsl.contains("InterlockedMin("));
    words[4] = 0x7e08_0214; // exposing the saved mask as scalar data requires a ballot
    let scalar_mask = super::super::translate(&compute_extract(&words)).unwrap();
    assert!(scalar_mask.hlsl.contains("tb_page_ballot(tb_lane,") && scalar_mask.hlsl.contains("saved_exec1.x"));
    words[4] = 0x7e08_0218;
    words[3] = 0xbefe_0416; // restore from s[22:23] instead of the saved s[20:21]
    assert!(super::super::translate(&compute_extract(&words)).is_err());
    words[3] = 0xbefe_0414;
    words[2] = 0xbe95_147e; // FF1 clobbers the high half of the saved EXEC.
    words[4] = 0x7e08_0215;
    assert!(super::super::translate(&compute_extract(&words)).is_err());
}

#[test]
fn numeric_inputs_and_lane_predicate_keep_their_distinct_exec_views() {
    // Save EXEC. Add its two words, use the saved pair as a lane predicate,
    // overwrite the upper word and repeat the same addition.
    let mut ex = compute_extract(&[0xbe9c_047e, 0xd525_0004, 0x0000_381d,
        0xd501_0005, 0x0072_0880, 0xbe9d_0391, 0xd525_0004, 0x0000_381d, 0xbf81_0000]);
    ex.isa_preamble = [0, 21, 1, 1];
    let hlsl = super::super::translate(&ex).unwrap().hlsl;
    assert!(hlsl.contains("(saved_exec0.y + saved_exec0.x)"), "{hlsl}");
    assert!(hlsl.contains("((s28 & 1u) != 0u)"), "{hlsl}");
    assert!(hlsl.contains("(s29 + saved_exec0.x)"), "{hlsl}");
    assert_eq!(hlsl.matches("WaveActiveBallot(").count(), 1);
}

#[test]
fn vector_comparisons_read_the_saved_word_before_overwriting_the_mask() {
    // Save EXEC, compare a signed VGPR against its low word, then write
    // an equality predicate to the same pair that holds the saved mask.
    let mut ex = compute_extract(&[0xbe9c_047e, 0x7d12_121c, 0xd481_006a, 0x0000_3909,
        0xd4c2_001c, 0x0002_121c, 0xd501_0005, 0x0072_0880, 0xbf81_0000]);
    ex.isa_preamble = [0, 21, 1, 1];
    let hlsl = super::super::translate(&ex).unwrap().hlsl;
    assert!(hlsl.contains("(asint(saved_exec0.x << 16) >> 16)"), "{hlsl}");
    assert!(hlsl.contains("(asint(v9) < asint(saved_exec0.x))"), "{hlsl}");
    assert!(hlsl.contains("(saved_exec0.x == v9)"), "{hlsl}");
    assert!(hlsl.contains("((s28 & 1u) != 0u)"), "{hlsl}");
    assert_eq!(hlsl.matches("WaveActiveBallot(").count(), 1);
}

#[test]
fn pair_comparisons_resolve_each_half_at_its_last_write() {
    // Overwrite the first saved pair's low word and the second pair's high
    // word. Both comparisons must combine ordinary SGPRs with saved bits.
    let mut ex = compute_extract(&[0xbe9c_047e, 0xbe9c_0393, 0xd4a1_006a, 0x0002_141c,
        0xbe9e_047e, 0xbe9f_0391, 0xd4e2_006a, 0x0000_3c1c, 0xbf81_0000]);
    ex.isa_preamble = [0, 21, 1, 1];
    let hlsl = super::super::translate(&ex).unwrap().hlsl;
    assert!(hlsl.contains("asint(saved_exec0.y) < asint(v11)"), "{hlsl}");
    assert!(hlsl.contains("saved_exec0.y == v11 && s28 < v10"), "{hlsl}");
    assert!(hlsl.contains("saved_exec0.y == s31 && s28 == saved_exec3.x"), "{hlsl}");
    assert_eq!(hlsl.matches("WaveActiveBallot(").count(), 2);
}

#[test]
fn restore_uses_the_bits_of_the_reassembled_exec_pair() {
    // Swap both halves. A small group's lanes now take their bits from
    // the original upper word, which contains no active lanes.
    let mut ex = compute_extract(&[0xbe9c_047e, 0xbe9e_031d, 0xbe9f_031c, 0xbefe_041e,
        0x7e08_0281, 0xbf81_0000]);
    ex.isa_preamble = [0, 21, 1, 1];
    let hlsl = super::super::translate(&ex).unwrap().hlsl;
    assert!(hlsl.contains("exec_lo = ((saved_exec0.y >> (WaveGetLaneIndex() & 31u)) & 1u)"), "{hlsl}");
}

#[test]
fn conditional_select_reads_the_predicate_bit_and_the_full_numeric_operand() {
    // Swap the EXEC words and use s30 as both the false value and mask base.
    let mut ex = compute_extract(&[0xbe9c_047e, 0xbe9e_031d, 0xbe9f_031c,
        0xd501_0004, 0x007a_121e, 0xbf81_0000]);
    ex.isa_preamble = [0, 21, 1, 1];
    let hlsl = super::super::translate(&ex).unwrap().hlsl;
    assert!(hlsl.contains("uint select_mask3 = ((saved_exec0.y >> (WaveGetLaneIndex() & 31u)) & 1u)"), "{hlsl}");
    assert!(hlsl.contains("((select_mask3 & 1u) != 0u) ? v9 : saved_exec0.y"), "{hlsl}");
}

#[test]
fn conditional_select_after_election_requires_a_known_saved_mask_pair() {
    // Restore EXEC after FF1, keep the saved low word, replace the high word.
    let mut words = vec![0x7d86_0087, 0xbe94_246a, 0xbe98_147e, 0xbefe_0414,
        0xbe9e_0414, 0xbe94_0480, 0xbe9f_03ff, 0x8000_0001, 0xd501_0009,
        0x0079_0280, 0x4a08_1218, 0xbf81_0000];
    let hlsl = super::super::translate(&compute_extract(&words)).unwrap().hlsl;
    assert!(hlsl.contains("((tb_lane & 32u) == 0u ? saved_lane1 : ((s31 >> (tb_lane & 31u)) & 1u))"), "{hlsl}");
    // Copy the elected lane into the high word. FF1 produces an ordinary
    // integer even though its source was the EXEC predicate.
    let mut dynamic = words.clone();
    dynamic.splice(6..8, [0xbe9f_0318]);
    for group_size in [21, 64] {
        let mut ex = compute_extract(&dynamic);
        ex.isa_preamble = [0, group_size, 1, 1];
        let hlsl = super::super::translate(&ex).unwrap().hlsl;
        assert!(hlsl.contains("uint select_mask7 = "), "{hlsl}");
        if group_size == 64 {
            assert!(hlsl.contains("((s31 >> (tb_lane & 31u)) & 1u)"), "{hlsl}");
        }
    }
    words.splice(6..8, [0xbe9f_0311]); // copy the unknown s17 instead of the literal
    assert!(super::super::translate(&compute_extract(&words)).is_err());
}

#[test]
fn aligned_halves_from_different_saves_reuse_each_lanes_predicate() {
    let ex = compute_extract(&[0xd765_0006, 0x0001_00c1, 0xd766_0006, 0x0002_0cc1,
        0xbe98_147e, 0x7d84_0c18, 0xbe9c_047e, 0xbe94_246a, 0xbea0_047e, 0xbefe_0414,
        0xbe9d_0321, 0xbefe_041c, 0x7e08_0281, 0xbf81_0000]);
    let hlsl = super::super::translate(&ex).unwrap().hlsl;
    assert!(hlsl.contains("exec_lo = ((tb_lane & 32u) == 0u ? saved_lane4 : saved_lane6)"), "{hlsl}");
    assert!(!hlsl.contains("tb_page_ballot("));
}

#[test]
fn restored_exec_pair_can_be_copied_and_used_as_numeric_data() {
    // Save and restore EXEC around FF1. The copy of the saved pair remains
    // live after its original registers are cleared.
    let ex = compute_extract(&[0x7d86_0087, 0xbe94_246a, 0xbe98_147e, 0xbefe_0414,
        0xbe9e_0414, 0xbe94_0480, 0xd525_0009, 0x0000_3e1e, 0x4a08_1218, 0xbf81_0000]);
    let hlsl = super::super::translate(&ex).unwrap().hlsl;
    assert!(hlsl.contains("(saved_exec1.x + saved_exec1.y)"), "{hlsl}");
}

#[test]
fn scalar_copies_retain_the_original_mask_words_until_overwritten() {
    // Copy and swap the mask words, clear the source, and copy the pair
    // onto itself. Replacing the low word later ends just that alias.
    let mut ex = compute_extract(&[0xbe9c_047e, 0xbe9e_041c, 0xbe9f_031e, 0xbe9e_031d,
        0xbe9c_0480, 0xbe9e_041e, 0xd525_0004, 0x0000_3e1e, 0xbe9e_0311,
        0xd525_0004, 0x0000_3e1e, 0xbf81_0000]);
    ex.isa_preamble = [0, 21, 1, 1];
    let hlsl = super::super::translate(&ex).unwrap().hlsl;
    assert!(hlsl.contains("(saved_exec0.y + saved_exec0.x)"), "{hlsl}");
    assert!(hlsl.contains("(s30 + saved_exec0.x)"), "{hlsl}");
    assert_eq!(hlsl.matches("WaveActiveBallot(").count(), 1);
}

#[test]
fn partial_register_write_preserves_only_the_other_snapshot_word() {
    // Copy EXEC to s[28:29], overwrite s29, then broadcast both registers.
    let mut ex = compute_extract(&[0xbe9c_047e, 0xbe9d_0391, 0x7e08_021c, 0x7e0a_021d, 0xbf81_0000]);
    ex.isa_preamble = [0, 7, 3, 1];
    let shader = super::super::translate(&ex).unwrap();
    assert_eq!(shader.required_wave_size, Some(32));
    assert!(shader.hlsl.contains("v4 = saved_exec0.x;"));
    assert!(shader.hlsl.contains("v5 = s29;"));
    assert!(!shader.hlsl.contains("saved_exec0.y;"));
}

#[test]
fn compute_register_writes_share_one_exec_region() {
    // v_add_nc_u32 v1, v0, v0; v_add_nc_u32 v2, v1, v1; s_endpgm
    let ex = compute_extract(&[0x4a02_0100, 0x4a04_0301, 0xbf81_0000]);
    let hlsl = super::super::translate(&ex).unwrap().hlsl;
    assert_eq!(hlsl.matches("if (exec_lo & 1u)").count(), 1, "{hlsl}");
    assert!(hlsl.contains("    if (exec_lo & 1u) {\n        v1 = "), "{hlsl}");
    assert!(hlsl.contains("\n        v2 = "), "{hlsl}");
}

#[test]
fn adjacent_pixel_register_writes_keep_order_and_exec_boundaries() {
    const GUARD: &str = "    if (exec_lo & 1u) ";
    let first = format!("{GUARD}v3 = asuint(asfloat(v2) + 1.0);");
    let second = format!("{GUARD}v2 = asuint(max(asfloat(v3), 0.0));");
    let grouped = "    if (exec_lo & 1u) {\n        v3 = asuint(asfloat(v2) + 1.0);\n        v2 = asuint(max(asfloat(v3), 0.0));\n    }\n";
    // Each instruction separates two runs of pure ALU. In particular,
    // derivatives and quad/wave operations must see their original EXEC.
    for boundary in [
        "    exec_lo = s4;",
        "    if (!exec_lo) { discard; }",
        "    if (jT) {",
        "    }",
        "    if (exec_lo & 1u) v3 = QuadReadLaneAt(v2, 1u);",
        "    if (exec_lo & 1u) v3 = WaveActiveSum(v2);",
        "    if (exec_lo & 1u) v3 = asuint(ddx(asfloat(v2)));",
        "    if (exec_lo & 1u) v3 = tex.Load(v2);",
        "    if (exec_lo & 1u) v3 = cache[v2];",
        "    if (exec_lo & 1u) InterlockedAdd(cache[v2], v3);",
        "    if (exec_lo & 1u) target.x = v3;",
    ] {
        let body = format!("{first}\n{second}\n{boundary}\n{first}\n{second}\n");
        assert_eq!(group(&body), format!("{grouped}{boundary}\n{grouped}"));
    }
}
