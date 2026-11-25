use super::*;
fn op(name: &str, enc: Enc, dst: Vec<Opnd>, src: Vec<Opnd>, offset: usize) -> Insn {
    Insn { offset, dwords: 1, enc, op: 0, raw: vec![0], mnemonic: name.into(), dst, src, aux: Default::default() }
}
#[test]
fn election_ignores_independent_single_word_moves() {
    let mut ops = vec![
        op("v_mbcnt_lo_u32_b32", Enc::Vop3, vec![Opnd::Vgpr(2)], vec![Opnd::ConstInt(-1), Opnd::ConstInt(0)], 0),
        op("s_ff1_i32_b32", Enc::Sop1, vec![Opnd::Sgpr(7)], vec![Opnd::ExecLo], 4),
        op("s_mov_b32", Enc::Sop1, vec![Opnd::Sgpr(8)], vec![Opnd::Sgpr(6)], 8),
        op("v_cmp_eq_u32", Enc::Vop3, vec![Opnd::VccLo], vec![Opnd::Vgpr(2), Opnd::Sgpr(7)], 12),
    ];
    assert!(verify_lane_election(&mut ops, 1, true).is_ok());
    ops[2].mnemonic = "s_mov_b64".into(); // s[6:7] reads the elected lane.
    assert!(verify_lane_election(&mut ops, 1, true).is_err());
    ops[2].mnemonic = "s_mov_b32".into();
    ops[2].dst = vec![Opnd::Sgpr(7)];
    assert!(verify_lane_election(&mut ops, 1, true).is_err());
}

#[test]
fn later_saveexec_uses_the_redefined_vcc() {
    let mut ops = vec![
        op("v_mbcnt_lo_u32_b32", Enc::Vop3, vec![Opnd::Vgpr(2)], vec![Opnd::ConstInt(-1), Opnd::ConstInt(0)], 0),
        op("s_ff1_i32_b32", Enc::Sop1, vec![Opnd::Sgpr(7)], vec![Opnd::ExecLo], 4),
        op("v_cmp_eq_u32", Enc::Vopc, vec![Opnd::VccLo], vec![Opnd::Sgpr(7), Opnd::Vgpr(2)], 8),
        op("v_cmp_eq_u32", Enc::Vopc, vec![Opnd::VccLo], vec![Opnd::ConstInt(0), Opnd::Vgpr(4)], 12),
        op("s_and_saveexec_b32", Enc::Sop1, vec![Opnd::Sgpr(20)], vec![Opnd::VccLo], 16),
    ];
    assert!(verify_lane_election(&mut ops, 1, true).is_ok());
    ops.remove(3);
    assert!(verify_lane_election(&mut ops, 1, true).is_err());
}

#[test]
fn symmetric_equality_still_requires_full_lane_count() {
    let mut ops = vec![
        op("v_mbcnt_lo_u32_b32", Enc::Vop3, vec![Opnd::Vgpr(2)], vec![Opnd::ConstInt(-1), Opnd::ConstInt(0)], 0),
        op("s_ff1_i32_b32", Enc::Sop1, vec![Opnd::Sgpr(7)], vec![Opnd::ExecLo], 4),
        op("v_cmp_eq_u32", Enc::Vop3, vec![Opnd::VccLo], vec![Opnd::Vgpr(2), Opnd::Sgpr(7)], 8),
    ];
    assert!(verify_lane_election(&mut ops, 1, true).is_ok());
    ops[2].src.swap(0, 1);
    assert!(verify_lane_election(&mut ops, 1, true).is_ok());
    ops[2].mnemonic = "v_cmp_ne_u32".into();
    assert!(verify_lane_election(&mut ops, 1, true).is_err());
    ops[2].mnemonic = "v_cmp_eq_u32".into();
    ops[0].src[0] = Opnd::ConstInt(0);
    assert!(verify_lane_election(&mut ops, 1, true).is_err());
}
