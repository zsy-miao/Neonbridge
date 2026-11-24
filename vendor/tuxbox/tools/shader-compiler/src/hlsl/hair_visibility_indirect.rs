//! Indirect-tile single-sample hair visibility compaction. Lane zero alone
//! reserves the group range; barriers publish the LDS count and base.
use super::{
    compute_group, packed_root::matches_program, Emitter, Enc, Insn, Opnd, Res, ShaderExtract,
    Stage,
};
pub(super) fn prepare(ex: &ShaderExtract, insns: &mut [Insn]) -> bool {
    if ex.stage != Stage::Compute
        || ex.entry.as_deref() != Some("MainCS")
        || !((compute_group(ex) == [8, 8, 1] && matches_program(&ex.isa, 916, 0x9477caca708f755b))
            || (compute_group(ex) == [8, 4, 1]
                && matches_program(&ex.isa, 916, 0xf4ee6f2691eb82b7)))
    {
        return false;
    }
    for i in insns {
        let p = i.offset;
        if [0x054, 0x190, 0x24c].contains(&p) {
            i.mnemonic = "tb_subsurface_group_sync".into();
        } else if [0x1b8, 0x1c8, 0x210, 0x224].contains(&p) {
            i.enc = Enc::Vop1;
            i.op = 1;
            i.mnemonic = "v_mov_b32".into();
            i.src = vec![Opnd::ConstInt(0)];
        } else if p == 0x1c0 {
            i.enc = Enc::Sop1;
            i.op = 3;
            i.mnemonic = "s_mov_b32".into();
            i.src = vec![Opnd::ConstInt(0)];
        } else if p == 0x21c {
            i.enc = Enc::Sop1;
            i.op = 3;
            i.mnemonic = "s_mov_b32".into();
        }
    }
    true
}

pub(super) fn resources(em: &mut Emitter<'_>) -> Result<(), String> {
    for (offset, expected) in [
        (0x058, Res::TBuf(3, false)),
        (0x0c0, Res::TBuf(4, false)),
        (0x10c, Res::Tex(1, 1)),
        (0x13c, Res::Tex(0, 1)),
        (0x144, Res::Tex(2, 1)),
        (0x328, Res::Tex(5, 1)),
        (0x300, Res::TBuf(3, true)),
        (0x380, Res::Uav(1, 1)),
    ] {
        let Some((idx, _)) = em
            .insns
            .iter()
            .enumerate()
            .find(|(_, op)| op.offset == offset)
        else {
            return Err("missing indirect hair visibility binding".into());
        };
        if em.res.get(&idx).and_then(|r| r.first()).copied() != Some(expected) {
            return Err(format!(
                "unexpected indirect hair visibility binding at {offset:#x}"
            ));
        }
    }
    em.tbuf_ty
        .extend([((false, 3), 1), ((false, 4), 1), ((true, 3), 1)]);
    em.img_ty.insert((true, 1), 1);
    Ok(())
}
