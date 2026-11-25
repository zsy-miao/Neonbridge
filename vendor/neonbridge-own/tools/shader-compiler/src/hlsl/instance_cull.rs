//! A 64-thread Nanite group appends visible instances in increasing guest lane
//! order. Capture participating lanes after culling and reconverge even those
//! that took a branch to ENDPGM before performing the group collectives.
use super::{compute_group, instance_cull_tail, r, Emitter, Enc, Insn, ShaderExtract, Stage};

pub(super) fn resources(em: &mut Emitter<'_>) -> Result<(), String> {
    use crate::meta::RootParamKind;
    // Four one-word table pointers precede the three dispatch-group SGPRs.
    // The replacement append sequence assumes the same four space-0 tables
    // and field types as the original resource descriptor loads.
    let root = em
        .ex
        .root_sig
        .as_ref()
        .ok_or("missing InstanceCull root signature")?;
    let tables = [(0, 5), (2, 1), (3, 0), (1, 4)];
    if root.params.len() != tables.len()
        || !root.params.iter().zip(tables).all(|(p, (kind, minimum))| {
            p.visibility == 0
                && matches!(&p.kind, RootParamKind::Table(ranges)
                if matches!(ranges.as_slice(), [r] if r.range_type == kind
                    && r.space == 0 && r.base_register == 0 && r.num >= minimum
                    && matches!(r.offset_in_table, 0 | u32::MAX)))
        })
    {
        return Err("unsupported InstanceCull descriptor tables".into());
    }
    let bindings = [
        (2, 0),
        (5, 0),
        (5, 1),
        (5, 2),
        (5, 3),
        (4, 4),
        (6, 0),
        (7, 1),
        (8, 2),
        (8, 3),
    ];
    if em.ex.resources.len() != bindings.len()
        || !bindings.iter().all(|&(kind, slot)| {
            em.ex
                .resources
                .iter()
                .any(|r| (r.res_type, r.space, r.lower, r.upper) == (kind, 0, slot, slot))
        })
    {
        return Err("unsupported InstanceCull resource binding list".into());
    }
    // The depth-only and visibility permutations use the same typed UAV slot.
    // DMASK=1 updates a uint depth, DMASK=3 updates a packed depth/ID pair.
    let mut images = em
        .insns
        .iter()
        .enumerate()
        .filter(|(_, i)| i.enc == Enc::Mimg);
    let Some((index, atomic)) = images.next() else {
        return Err("missing InstanceCull image atomic".into());
    };
    if images.next().is_some()
        || atomic.mnemonic != "image_atomic_umax"
        || em.res.get(&index).map(Vec::as_slice) != Some(&[super::Res::Uav(0, 1)])
    {
        return Err("unsupported InstanceCull visibility image".into());
    }
    match atomic.raw.as_slice() {
        [0xf05c1108, _] => {}
        [0xf05c1308, _] => { em.wide_visibility_uavs.insert(0); }
        _ => return Err("unsupported InstanceCull image atomic flags".into()),
    };
    // Every packed-view read uses a byte offset in the first structure. Give
    // that structure room for its entire accessed prefix. Indexing an array
    // of float4s could advance to the next view because of the bound stride.
    let mut view_words = 0;
    for (index, bindings) in &em.res {
        if bindings.as_slice() != [super::Res::Buf(3, false)] {
            continue;
        }
        let load = &em.insns[*index];
        if load.enc != Enc::Smem
            || !(8..=12).contains(&load.op)
            || load.raw.get(1).map(|w| w >> 25) != Some(0x7d)
            || !(0..2048).contains(&load.aux.imm)
            || load.aux.imm % 4 != 0
        {
            return Err("unsupported InstanceCull packed-view addressing".into());
        }
        let end = load.aux.imm as u32 / 4 + (1u32 << (load.op - 8));
        view_words = view_words.max(end);
    }
    if view_words == 0 || view_words > 512 {
        return Err("invalid InstanceCull packed-view structure size".into());
    }
    em.sbuf.insert((false, 3), view_words);
    Ok(())
}

pub(super) fn prepare(ex: &ShaderExtract, insns: &mut Vec<Insn>) -> bool {
    if ex.stage != Stage::Compute || compute_group(ex) != [64, 1, 1] {
        return false;
    }
    let Some(tail) = instance_cull_tail::recognize(insns) else {
        return false;
    };
    // Preserve all incoming branches to ENDPGM. At the first instruction of
    // the recognized tail each participating lane captures its append inputs.
    insns.retain(|i| i.offset <= tail.start || i.mnemonic == "s_endpgm");
    let capture = insns.len() - 2;
    let op = &mut insns[capture];
    op.mnemonic = "tb_instance_candidate".into();
    op.enc = Enc::Vop1;
    op.dst.clear();
    op.src = vec![tail.node, tail.limit, tail.address, tail.flags];
    true
}

pub(super) fn emit(em: &mut Emitter<'_>, op: &Insn) -> Result<bool, String> {
    if op.mnemonic != "tb_instance_candidate" {
        return Ok(false);
    }
    for reg in &op.src {
        em.touch(reg);
    }
    let [node, limit, address, flags] = &op.src[..] else {
        return Err("incomplete instance queue capture".into());
    };
    em.line("cull_candidate = (exec_lo & 1u) != 0u;");
    em.line(&format!(
        "cull_record = uint4({}, {}, {}, {});",
        r(node)?,
        r(limit)?,
        r(address)?,
        r(flags)?
    ));
    Ok(true)
}

// Both counter descriptors address fields of the first structured element.
// Byte offsets 16 and 20 remain independent of the view's structure stride.
pub(super) fn queue_declarations(nodes: u32) -> String {
    let queue = nodes + 1;
    let stats = nodes + 2;
    format!(r#"
struct CullQueueCounters {{ uint4 reserved; uint nodes; uint candidates; }};
struct CullStatistics {{ uint4 reserved; uint instances; }};
RWByteAddressBuffer cull_nodes : register(u{nodes});
RWStructuredBuffer<CullQueueCounters> cull_queue : register(u{queue});
RWStructuredBuffer<CullStatistics> cull_stats : register(u{stats});
groupshared uint cull_mask[2], cull_base;
"#)
}

pub(super) const EPILOGUE: &str = r#"
    uint lane = gtid.x;
    if (lane < 2u) cull_mask[lane] = 0u;
    GroupMemoryBarrierWithGroupSync();
    if (cull_candidate) InterlockedOr(cull_mask[lane >> 5], 1u << (lane & 31));
    GroupMemoryBarrierWithGroupSync();
    uint lower = cull_mask[0], higher = cull_mask[1];
    uint count = countbits(lower) + countbits(higher);
    uint first = lower != 0u ? firstbitlow(lower) : 32u + firstbitlow(higher);
    if (cull_candidate && lane == first) {
        uint base;
        InterlockedAdd(cull_queue[0].nodes, count, base);
        cull_base = base;
        InterlockedAdd(cull_queue[0].candidates, count);
        if ((cull_record.w & 1u) != 0u) InterlockedAdd(cull_stats[0].instances, count);
    }
    GroupMemoryBarrierWithGroupSync();
    if (cull_candidate) {
        uint rank = countbits(cull_mask[lane >> 5] & ((1u << (lane & 31)) - 1u));
        if (lane >= 32u) rank += countbits(lower);
        uint index = cull_base + rank;
        if (index < cull_record.y) {
            uint address = ((cull_record.z >> 4u) & 0x0ffffffcu) + (index << 3u);
            cull_nodes.Store2(address, uint2((cull_record.x << 3u) | 1u, 0u));
        }
    }
"#;
