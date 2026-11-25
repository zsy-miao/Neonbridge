//! MSAA strand visibility compaction uses one guest wave per workgroup. The
//! native sample loop merges matching IDs and depth-tests each sample before
//! allocating its nodes. Keep the texture reads, mask loops and atomics intact.
use super::{compute_group, packed_root::matches_program, Emitter, ShaderExtract, Stage};

pub(super) fn recognize(ex: &ShaderExtract) -> bool {
    if ex.stage != Stage::Compute || ex.entry.as_deref() != Some("MainCS") {
        return false;
    }
    let [8, height, 1] = compute_group(ex) else {
        return false;
    };
    let fingerprint = match (height, ex.isa.len()) {
        (4, 1612) => 0xfc7cd033ce003bc7,
        (8, 1612) => 0x1477524139be7a84,
        (4, 1728) => 0xb6e4bbb1ba3b1e9e,
        (8, 1728) => 0x8e594c9a8e7b7649,
        (4, 3336) => 0x9dfdfc43847d211d,
        (8, 3336) => 0xfb0a94f7e3710413,
        (4, 3444) => 0x71d6fc0ac5b3b28c,
        (8, 3444) => 0xd9dd28f6129b103e,
        (4, 9500) => 0xf7647dd17e1c05f8,
        (8, 9500) => 0xb202bcb623a34f81,
        (4, 9604) => 0x9d06a0caee0a1973,
        (8, 9604) => 0x2c2eade12ad1935e,
        _ => return false,
    };
    matches_program(&ex.isa, ex.isa.len(), fingerprint)
}

pub(super) fn resources(em: &mut Emitter<'_>) -> Result<(), String> {
    // The MSAA ID and depth views share the sample index, but are read as
    // unsigned bits and floating point depth, respectively.
    let tiled = em.tbuf.contains(&(false, 3));
    let coverage = if tiled { 5 } else { 3 };
    if em.tex.get(&0) != Some(&1)
        || em.tex.get(&1) != Some(&6)
        || em.tex.get(&2) != Some(&6)
        || em.tex.get(&coverage) != Some(&1)
        || ![0, 1, 4].iter().all(|slot| em.uav.get(slot) == Some(&1))
        || !em.sbuf.contains_key(&(true, 2))
        || !em.tbuf.contains(&(true, 3))
        || (tiled && !em.tbuf.contains(&(false, 4)))
    {
        return Err("unexpected multisample hair compaction bindings".into());
    }
    em.img_ty.insert((false, 1), 1);
    em.img_ty.remove(&(false, 2));
    em.img_ty.insert((true, 1), 1);
    em.tbuf_ty.insert((true, 3), 1);
    if tiled {
        em.tbuf_ty.extend([((false, 3), 1), ((false, 4), 1)]);
    }
    Ok(())
}
