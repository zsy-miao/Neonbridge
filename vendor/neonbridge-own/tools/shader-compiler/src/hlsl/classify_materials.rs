//! The material classifier sets one LDS bit per visible material in a tile.
use super::{compute_group, fssm_bindings, packed_root::matches_program, Emitter, Insn, Res, ShaderExtract, Stage};

pub(super) fn prepare(ex: &ShaderExtract, insns: &mut Vec<Insn>) -> bool {
    if ex.stage != Stage::Compute || ex.entry.as_deref() != Some("ClassifyMaterials")
        || compute_group(ex) != [16, 16, 1]
        || !matches_program(&ex.isa, 1608, 0x24035fbd859390eb)
    {
        return false;
    }
    // For a uniform material the guest elects one active lane to OR its bit.
    // For mixed materials it ORs each active lane's bit. The return value is
    // unused, so the second path gives the same LDS contents in either case.
    // Keep the t5 lookup, the per-lane OR and the EXEC restore at 0x498.
    insns.retain(|op| op.offset != 0x3f8 && !(0x408..0x478).contains(&op.offset));
    true
}

pub(super) fn resources(em: &mut Emitter<'_>) -> Result<(), String> {
    for (&(uav, slot), words) in &mut em.sbuf {
        *words = fssm_bindings::structured_words(em.ex, slot, uav)?;
    }
    let image = em.insns.iter().position(|op| op.offset == 0x130);
    if image.and_then(|i| em.res.get(&i)).map(Vec::as_slice) != Some(&[Res::Tex(4, 1)][..]) {
        return Err("unexpected material visibility binding".into());
    }
    em.img_ty.insert((false, 4), 1);
    Ok(())
}
