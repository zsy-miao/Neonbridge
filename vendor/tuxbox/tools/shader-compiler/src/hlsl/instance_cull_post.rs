//! Cull the previous pass's compacted (instance, view) list. Surviving
//! records use three words and follow the first pass's reserved queue.
use super::{
    compute_group, fssm_bindings, packed_root::matches_program, Emitter, Enc, Insn, Opnd, Res,
    ShaderExtract, Stage,
};

pub(super) fn prepare(ex: &ShaderExtract, ops: &mut Vec<Insn>) -> bool {
    if ex.stage != Stage::Compute
        || ex.entry.as_deref() != Some("InstanceCull")
        || compute_group(ex) != [64, 1, 1]
    {
        return false;
    }
    let (tail, limit, address, view) = if matches_program(&ex.isa, 5728, 0x6c1533bb20b0fbdf) {
        (0x1318, 24, 16, Opnd::Vgpr(4))
    } else if matches_program(&ex.isa, 6852, 0x8b58289f2f29eb11) {
        (0x1774, 28, 20, Opnd::ConstInt(0))
    } else {
        return false;
    };
    ops.retain(|op| op.offset <= tail || op.mnemonic == "s_endpgm");
    let Some(op) = ops.iter_mut().find(|op| op.offset == tail) else {
        return false;
    };
    op.mnemonic = "tb_instance_candidate".into();
    op.enc = Enc::Vop1;
    op.dst.clear();
    op.src = vec![Opnd::Vgpr(0), Opnd::Sgpr(limit), Opnd::Sgpr(address), view];
    true
}

pub(super) fn resources(em: &mut Emitter<'_>) -> Result<(), String> {
    let required = [
        (2, 0),
        (5, 0),
        (5, 1),
        (5, 2),
        (3, 3),
        (5, 4),
        (4, 5),
        (5, 6),
        (3, 7),
        (1, 0),
        (6, 0),
        (7, 1),
        (8, 2),
        (8, 3),
    ];
    if em.ex.resources.len() != required.len()
        || !required.iter().all(|&(kind, slot)| {
            em.ex
                .resources
                .iter()
                .any(|r| (r.res_type, r.space, r.lower, r.upper) == (kind, 0, slot, slot))
        })
    {
        return Err("unsupported post-pass instance culling bindings".into());
    }
    em.raw.insert((true, 1));
    em.sbuf.entry((true, 2)).or_insert(11);
    em.sbuf.entry((true, 3)).or_insert(9);
    for (&(uav, slot), width) in &mut em.sbuf {
        let reflected = fssm_bindings::structured_words(em.ex, slot, uav)?;
        if uav && ((slot == 2 && reflected < 11) || (slot == 3 && reflected < 9)) {
            return Err("post-pass instance culling counters exceed their structure".into());
        }
        *width = reflected;
        em.reflected_strides.insert((uav, slot));
    }
    em.tbuf_ty.insert((false, 7), 1);
    for (i, op) in em
        .insns
        .iter()
        .enumerate()
        .filter(|(_, op)| op.mnemonic == "image_atomic_umax")
    {
        if em.res.get(&i).map(Vec::as_slice) != Some(&[Res::Uav(0, 1)]) {
            return Err("unexpected post-pass visibility buffer".into());
        }
        match op.raw.as_slice() {
            [0xf05c1108, _] => {}
            [0xf05c1308, _] => {
                em.wide_visibility_uavs.insert(0);
            }
            _ => return Err("unsupported post-pass visibility atomic".into()),
        }
    }
    Ok(())
}

pub(super) const APPEND: &str = r#"
    uint lane = gtid.x;
    if (lane < 2u) cull_mask[lane] = 0u;
    GroupMemoryBarrierWithGroupSync();
    if (cull_candidate) InterlockedOr(cull_mask[lane >> 5], 1u << (lane & 31u));
    GroupMemoryBarrierWithGroupSync();
    uint lo = cull_mask[0], hi = cull_mask[1];
    if (lane == 0u && (lo | hi) != 0u) {
        uint count = countbits(lo) + countbits(hi);
        uint base;
        InterlockedAdd(ubuf2[0].w[9], count, base);
        cull_base = base;
        InterlockedAdd(ubuf2[0].w[10], count);
        if ((cb0[2].w & 1u) != 0u) InterlockedAdd(ubuf3[0].w[8], count);
    }
    GroupMemoryBarrierWithGroupSync();
    if (cull_candidate) {
        uint rank = countbits(((lane < 32u) ? lo : hi) & ((1u << (lane & 31u)) - 1u));
        uint index = cull_base + rank + ((lane < 32u) ? 0u : countbits(lo));
        if (index < cull_record.y) {
            uint start = ((cull_record.z >> 3) & 0x1ffffff8u) + (cull_record.y << 3);
            raw_ubuf1.Store3(start + index * 12u, uint3((cull_record.x << 3) | 5u, cull_record.w << 16, 255u));
        }
    }
"#;
