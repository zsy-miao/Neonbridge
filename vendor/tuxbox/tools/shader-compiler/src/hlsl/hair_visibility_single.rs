//! Single-sample hair visibility compaction. Local lane zero is the only
//! reservation participant; the group barriers publish the shared counts.
use super::{
    compute_group, packed_root::matches_program, Emitter, Enc, Insn, Opnd, Res, ShaderExtract,
    Stage,
};
pub(super) fn prepare(ex: &ShaderExtract, insns: &mut [Insn]) -> bool {
    if ex.stage != Stage::Compute
        || ex.entry.as_deref() != Some("MainCS")
        || !((compute_group(ex) == [8, 4, 1] && matches_program(&ex.isa, 816, 0x17f7cf03fb1cfbd5))
            || (compute_group(ex) == [8, 8, 1]
                && matches_program(&ex.isa, 816, 0x01cb72bde582d06c)))
    {
        return false;
    }
    for i in insns {
        let p = i.offset;
        if [0x084, 0x148, 0x200].contains(&p) {
            i.mnemonic = "tb_subsurface_group_sync".into();
        } else if [0x16c, 0x17c, 0x1c4, 0x1d8].contains(&p) {
            i.enc = Enc::Vop1;
            i.op = 1;
            i.mnemonic = "v_mov_b32".into();
            i.src = vec![Opnd::ConstInt(0)];
        } else if p == 0x174 {
            i.enc = Enc::Sop1;
            i.op = 3;
            i.mnemonic = "s_mov_b32".into();
            i.src = vec![Opnd::ConstInt(0)];
        } else if p == 0x1d0 {
            i.enc = Enc::Sop1;
            i.op = 3;
            i.mnemonic = "s_mov_b32".into();
        }
    }
    true
}

pub(super) fn resources(em: &mut Emitter<'_>) -> Result<(), String> {
    for (offset, expected) in [
        (0x0c4, Res::Tex(1, 1)),
        (0x0f4, Res::Tex(0, 1)),
        (0x0fc, Res::Tex(2, 1)),
        (0x2d8, Res::Tex(3, 1)),
        (0x2b0, Res::TBuf(3, true)),
        (0x31c, Res::Uav(1, 1)),
    ] {
        let Some((idx, _)) = em
            .insns
            .iter()
            .enumerate()
            .find(|(_, op)| op.offset == offset)
        else {
            return Err("missing hair visibility binding".into());
        };
        if em.res.get(&idx).and_then(|r| r.first()).copied() != Some(expected) {
            return Err(format!("unexpected hair visibility binding at {offset:#x}"));
        }
    }
    em.tbuf_ty.insert((true, 3), 1);
    em.img_ty.insert((true, 1), 1);
    Ok(())
}
