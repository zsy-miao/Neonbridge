use super::*;

fn insn(mnemonic: &str, enc: Enc, op: u32, dst: Vec<Opnd>, src: Vec<Opnd>) -> Insn {
    Insn { offset: 0, dwords: 1, enc, op, raw: vec![0, 125 << 25], mnemonic: mnemonic.into(), dst, src,
        aux: Default::default() }
}

fn program(mut ops: Vec<Insn>) -> Vec<Insn> {
    ops.push(insn("s_endpgm", Enc::Sopp, 1, vec![], vec![]));
    for (i, op) in ops.iter_mut().enumerate() {
        op.offset = i * 4;
    }
    ops
}

fn jump(op: u32, over: i64) -> Insn {
    let mut branch = insn("s_cbranch", Enc::Sopp, op, vec![], vec![]);
    branch.aux.imm = over;
    branch
}

fn compare() -> Insn {
    insn("v_cmp_gt_u32", Enc::Vopc, 0, vec![Opnd::VccLo], vec![Opnd::Vgpr(0), Opnd::Vgpr(1)])
}

fn scalar(mnemonic: &str, dst: u8, src: Vec<Opnd>) -> Insn {
    let enc = if src.len() == 1 { Enc::Sop1 } else { Enc::Sop2 };
    insn(mnemonic, enc, 0, vec![Opnd::Sgpr(dst)], src)
}

#[test]
fn mask_operations_vote_on_their_condition_code() {
    let ops = program(vec![
        compare(),
        scalar("s_and_b32", 4, vec![Opnd::VccLo, Opnd::ExecLo]),
        jump(4, 1),
        scalar("s_and_saveexec_b32", 5, vec![Opnd::VccLo]),
        insn("s_cmp_eq_u32", Enc::Sopc, 0, vec![Opnd::Scc], vec![Opnd::Sgpr(4), Opnd::ConstInt(0)]),
    ]);
    let votes = prove(&ops, 32).unwrap();
    assert_eq!(votes[&1], Vote { mask: Opnd::Sgpr(4), none: false });
    assert_eq!(votes[&3], Vote { mask: Opnd::ExecLo, none: false });
    assert_eq!(votes[&4], Vote { mask: Opnd::Sgpr(4), none: true });
}

#[test]
fn ordinary_scalar_work_needs_no_vote() {
    let ops = program(vec![
        scalar("s_add_u32", 4, vec![Opnd::Sgpr(0), Opnd::ConstInt(1)]),
        scalar("s_and_b32", 5, vec![Opnd::Sgpr(4), Opnd::ConstInt(-1)]),
        jump(5, 0),
    ]);
    assert!(prove(&ops, 32).unwrap().is_empty());
}

#[test]
fn masks_used_as_numbers_are_refused() {
    let mask = || scalar("s_mov_b32", 4, vec![Opnd::ExecLo]);
    for misuse in [
        scalar("s_add_u32", 5, vec![Opnd::Sgpr(4), Opnd::ConstInt(1)]),
        scalar("s_bcnt1_i32_b32", 5, vec![Opnd::Sgpr(4)]),
        scalar("s_and_b32", 5, vec![Opnd::Sgpr(4), Opnd::Sgpr(0)]),
        insn("v_mov_b32", Enc::Vop1, 0, vec![Opnd::Vgpr(2)], vec![Opnd::Sgpr(4)]),
        insn("s_cmp_gt_u32", Enc::Sopc, 0, vec![Opnd::Scc], vec![Opnd::Sgpr(4), Opnd::ConstInt(0)]),
    ] {
        let name = misuse.mnemonic.clone();
        assert_eq!(prove(&program(vec![mask(), misuse]), 32), None, "{name}");
    }
}

#[test]
fn numbers_used_as_masks_are_refused() {
    // A literal written to EXEC selects lanes by position.
    let write = scalar("s_mov_b32", 126, vec![Opnd::Literal(0xffff)]);
    let mut write = write;
    write.dst = vec![Opnd::ExecLo];
    assert_eq!(prove(&program(vec![write, jump(8, 0)]), 32), None);
    // A register that is a mask on only one path cannot be read after them.
    let ops = program(vec![
        jump(5, 1),
        scalar("s_mov_b32", 4, vec![Opnd::ExecLo]),
        scalar("s_mov_b32", 5, vec![Opnd::Sgpr(4)]),
    ]);
    assert_eq!(prove(&ops, 32), None);
}

#[test]
fn masks_survive_loops_and_saved_exec() {
    let mut restore = scalar("s_mov_b32", 0, vec![Opnd::Sgpr(10)]);
    restore.dst = vec![Opnd::ExecLo];
    let ops = program(vec![
        scalar("s_and_saveexec_b32", 10, vec![Opnd::ExecLo]),
        compare(),
        scalar("s_and_saveexec_b32", 11, vec![Opnd::VccLo]),
        jump(9, -3),
        restore,
    ]);
    assert_eq!(prove(&ops, 32).unwrap().len(), 2);
}

#[test]
fn unused_source_fields_are_not_reads() {
    // The long encoding's third field of a two-operand instruction names s0.
    let spare = |mnemonic: &str, enc, dst: Vec<Opnd>| {
        insn(mnemonic, enc, 0, dst, vec![Opnd::Vgpr(1), Opnd::Vgpr(2), Opnd::Sgpr(0)])
    };
    let mask = || scalar("s_mov_b32", 0, vec![Opnd::ExecLo]);
    for op in [
        spare("v_cmp_eq_u32", Enc::Vop3, vec![Opnd::Sgpr(4)]),
        spare("v_fmac_f32", Enc::Vop3, vec![Opnd::Vgpr(3)]),
        spare("image_load", Enc::Mimg, vec![Opnd::Vgpr(3)]),
    ] {
        let name = op.mnemonic.clone();
        assert!(prove(&program(vec![mask(), op]), 32).is_some(), "{name}");
    }
    for op in [
        spare("v_fma_f32", Enc::Vop3, vec![Opnd::Vgpr(3)]),
        spare("image_sample", Enc::Mimg, vec![Opnd::Vgpr(3)]),
    ] {
        let name = op.mnemonic.clone();
        assert_eq!(prove(&program(vec![mask(), op]), 32), None, "{name}");
    }
}

#[test]
fn branch_conditions_cover_both_masks_and_polarities() {
    assert_eq!(branch_condition(8).unwrap(), "!WaveActiveAnyTrue((exec_lo & 1u) != 0u)");
    assert_eq!(branch_condition(7).unwrap(), "WaveActiveAnyTrue((vcc_lo & 1u) != 0u)");
    assert_eq!(branch_condition(4), None);
}
