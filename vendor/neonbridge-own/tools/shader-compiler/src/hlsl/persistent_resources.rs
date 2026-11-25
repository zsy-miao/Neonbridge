//! Resource layout of the Xbox persistent hierarchy and cluster queue kernel.
use super::{fssm_bindings, Emitter, Enc, Opnd, Res};
use crate::meta::RootParamKind;

pub(super) fn prepare(em: &mut Emitter<'_>) -> Result<(), String> {
    // Four user-data SGPRs are descriptor table pointers, in this order.
    let root = em
        .ex
        .root_sig
        .as_ref()
        .ok_or("missing PersistentClusterCull root signature")?;
    let tables = [(0, 7), (2, 1), (3, 0), (1, 6)];
    if root.params.len() != tables.len()
        || !root.params.iter().zip(tables).all(|(p, (kind, minimum))| {
            p.visibility == 0
                && matches!(&p.kind, RootParamKind::Table(ranges)
                if matches!(ranges.as_slice(), [r] if r.range_type == kind
                    && r.space == 0 && r.base_register == 0 && r.num >= minimum
                    && matches!(r.offset_in_table, 0 | u32::MAX)))
        })
    {
        return Err("unsupported PersistentClusterCull descriptor tables".into());
    }
    if has_resource(em, 3, 3) {
        return shadow_resources(em);
    }
    let hzb = em
        .ex
        .resources
        .iter()
        .any(|r| (r.res_type, r.space, r.lower, r.upper) == (3, 0, 5, 5));
    let post = hzb
        && em
            .ex
            .resources
            .iter()
            .any(|r| (r.res_type, r.space, r.lower, r.upper) == (3, 0, 7, 7));
    let shift = u32::from(hzb);
    let prior_counts = 6 + shift + u32::from(post);
    let bindings = [
        (2, 0),
        (5, 0),
        (5, 1),
        (5, 2),
        (4, 3),
        (5, 4),
        (4, 5 + shift),
        (5, prior_counts),
        (8, 0),
        (8, 1),
        (7, 2),
        (7, 3),
        (7, 4),
        (6, 5),
    ];
    let extra = [(3, 5), (1, 0), (3, 7)];
    let bindings = bindings.iter().chain(if post {
        &extra[..]
    } else if hzb {
        &extra[..2]
    } else {
        &[]
    });
    if em.ex.resources.len() != bindings.clone().count()
        || !bindings.into_iter().all(|&(kind, slot)| {
            em.ex
                .resources
                .iter()
                .any(|r| (r.res_type, r.space, r.lower, r.upper) == (kind, 0, slot, slot))
        })
    {
        return Err("unsupported PersistentClusterCull resource bindings".into());
    }
    if hzb {
        // First and second passes touch different fields of the same buffers.
        // The record size remains the one published in the binding table.
        for (&(uav, slot), extent) in &mut em.sbuf {
            *extent = fssm_bindings::structured_words(em.ex, slot, uav)?;
        }
        if post {
            em.tbuf_ty.insert((false, 7), 1);
        }
        em.persistent_queue = true;
        return Ok(());
    }
    // Vector accesses address float4 scene rows, uint instance lookups and
    // three-word streaming requests. All five queue counters are fields of
    // element zero, including the pending-node count at byte 20.
    for (key, words) in [
        ((false, 0), 4),
        ((false, 1), 4),
        ((false, 2), 1),
        ((true, 0), 3),
        ((true, 1), 6),
    ] {
        if em.sbuf.get(&key) != Some(&words) {
            return Err(format!(
                "unsupported PersistentClusterCull structured access {key:?}: {:?}",
                em.sbuf.get(&key)
            ));
        }
    }
    // The hierarchy lookup is an indexed structured load of the first field.
    // Element one starts beyond the three padding words in element zero.
    em.sbuf.insert(
        (false, 2),
        fssm_bindings::structured_words(em.ex, 2, false)?,
    );
    // The view prefix read here spans 192 dwords; prior counts use two. The single-view
    // kernel reads view zero with scalar loads. Multiview uses the view index
    // from the queue entry with the same constant field offsets.
    for (slot, expected) in [(4, 192), (prior_counts, 2)] {
        let mut words = 0;
        let mut indexed = false;
        for (&index, resources) in &em.res {
            if resources.as_slice() != [Res::Buf(slot, false)] {
                continue;
            }
            let load = &em.insns[index];
            let width = match load.enc {
                Enc::Smem
                    if (8..=12).contains(&load.op)
                        && load.raw.get(1).map(|w| w >> 25) == Some(0x7d) =>
                {
                    Some(1 << (load.op - 8))
                }
                Enc::Mubuf
                    if slot == 4
                        && load.aux.mask == 2
                        && matches!(
                            load.src.as_slice(),
                            [Opnd::Vgpr(_), Opnd::Sgpr(_), Opnd::ConstInt(0)]
                        )
                        && matches!(
                            load.mnemonic.as_str(),
                            "buffer_load_dword"
                                | "buffer_load_dwordx2"
                                | "buffer_load_dwordx3"
                                | "buffer_load_dwordx4"
                        ) =>
                {
                    indexed = true;
                    Emitter::mubuf_dwords(&load.mnemonic)
                }
                _ => None,
            }
            .ok_or_else(|| format!("unsupported PersistentClusterCull t{slot} addressing"))?;
            if !(0..expected as i64 * 4).contains(&load.aux.imm) || load.aux.imm % 4 != 0 {
                return Err(format!(
                    "invalid PersistentClusterCull t{slot} field offset"
                ));
            }
            words = words.max(load.aux.imm as u32 / 4 + width);
        }
        if words != expected {
            return Err(format!(
                "invalid PersistentClusterCull t{slot} extent {words} (expected {expected})"
            ));
        }
        if indexed {
            let stride = fssm_bindings::structured_words(em.ex, slot, false)?;
            if stride < words {
                return Err(format!(
                    "PersistentClusterCull t{slot} fields exceed its element stride"
                ));
            }
            words = stride;
        }
        em.sbuf.insert((false, slot), words);
    }
    em.persistent_queue = true;
    Ok(())
}

// The virtual-shadow permutations bind the HZB before their hierarchy
// resources. The larger layout also has page flags, a page table and b1.
fn has_resource(em: &Emitter<'_>, kind: u32, slot: u32) -> bool {
    em.ex
        .resources
        .iter()
        .any(|r| (r.res_type, r.space, r.lower, r.upper) == (kind, 0, slot, slot))
}

fn shadow_resources(em: &mut Emitter<'_>) -> Result<(), String> {
    // PSV resource kinds identify typed, structured and byte-address views.
    let mut bindings = vec![
        (2, 0),
        (1, 0),
        (5, 0),
        (5, 1),
        (5, 2),
        (3, 3),
        (8, 0),
        (8, 1),
        (7, 2),
        (7, 3),
        (7, 4),
        (6, 5),
    ];
    if has_resource(em, 2, 1) {
        bindings.extend([
            (2, 1),
            (5, 4),
            (5, 5),
            (5, 6),
            (5, 7),
            (4, 8),
            (5, 9),
            (4, 10),
        ]);
        if has_resource(em, 5, 11) {
            bindings.push((5, 11));
        }
        if has_resource(em, 8, 6) {
            bindings.push((8, 6));
        }
    } else if has_resource(em, 4, 4) {
        bindings.extend([(4, 4), (5, 5), (4, 6), (3, 7), (5, 8)]);
        // The post pass reads its previously generated R32_UINT counts.
        em.tbuf_ty.insert((false, 7), 1);
    } else {
        bindings.extend([(5, 4), (5, 5), (4, 6), (5, 7), (4, 8)]);
    }
    if bindings.len() != em.ex.resources.len()
        || !bindings
            .into_iter()
            .all(|(kind, slot)| has_resource(em, kind, slot))
    {
        return Err("unrecognized persistent shadow resource layout".into());
    }
    for (&(uav, slot), words) in &mut em.sbuf {
        *words = fssm_bindings::structured_words(em.ex, slot, uav)?;
    }
    em.persistent_queue = true;
    Ok(())
}
