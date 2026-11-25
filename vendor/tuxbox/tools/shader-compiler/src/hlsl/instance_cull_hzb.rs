//! Single-view culling writes occluded IDs for a later dispatch and visible
//! nodes for this dispatch. Each compacted list keeps guest wave64 lane order.
use super::{compute_group, fssm_bindings, packed_root::matches_program, Emitter, Enc, Insn, Opnd, Res, ShaderExtract, Stage};

pub(super) fn prepare(ex: &ShaderExtract, ops: &mut Vec<Insn>) -> bool {
    if ex.stage != Stage::Compute || ex.entry.as_deref() != Some("InstanceCull")
        || compute_group(ex) != [64, 1, 1]
    {
        return false;
    }
    let nodes = if matches_program(&ex.isa, 8364, 0xb63951b34b94332f) {
        0x1d64
    } else if matches_program(&ex.isa, 8308, 0xaa027771a1d2dc9b) {
        0x1d2c
    } else {
        return false;
    };
    // No consumer reads either append before ENDPGM. Keep the EXEC restore
    // and the impediment flag v42 that prevents an occluded node's second append.
    ops.retain(|op| op.offset <= 0x11fc || (0x13b8..=nodes).contains(&op.offset) || op.mnemonic == "s_endpgm");
    for op in ops {
        if op.offset == 0x11fc {
            op.mnemonic = "tb_hidden_instance".into();
            op.enc = Enc::Vop1;
            op.src = vec![Opnd::Vgpr(0), Opnd::Vgpr(28)];
            op.dst = vec![Opnd::Vgpr(42)];
        } else if op.offset == nodes {
            op.mnemonic = "tb_instance_candidate".into();
            op.enc = Enc::Vop1;
            op.src = vec![Opnd::Vgpr(0), Opnd::Sgpr(25), Opnd::Sgpr(28), Opnd::Sgpr(31)];
            op.dst.clear();
        }
    }
    true
}

pub(super) fn resources(em: &mut Emitter<'_>) -> Result<(), String> {
    for (&(uav, slot), words) in &mut em.sbuf {
        *words = fssm_bindings::structured_words(em.ex, slot, uav)?;
    }
    for (i, op) in em.insns.iter().enumerate().filter(|(_,op)| op.mnemonic == "image_atomic_umax") {
        // This variant uses a 32-bit visibility depth texture at u1 and
        // consumes the old value returned by the atomic max.
        if (matches_program(&em.ex.isa, 6164, 0xc9a78b1d5994aff1)
            || matches_program(&em.ex.isa, 5568, 0x136f372983992807)
            || matches_program(&em.ex.isa, 6964, 0x1729a67e96889c85)
            || matches_program(&em.ex.isa, 6900, 0xbc567b72ca1da0bf)
            || matches_program(&em.ex.isa, 6884, 0x910d535f5eff32df)
            || matches_program(&em.ex.isa, 5812, 0x0deaf134b3a7d33c)
            || matches_program(&em.ex.isa, 6664, 0xd5d05d54239b5f0e)
            || matches_program(&em.ex.isa, 6612, 0x46feb57fa9a3dddd)
            || matches_program(&em.ex.isa, 6580, 0x9aa6d5b1dbbb8463)
            || matches_program(&em.ex.isa, 5500, 0x901457214a6d2984)
            || matches_program(&em.ex.isa, 5860, 0xb8d99116ec764600)
            || matches_program(&em.ex.isa, 8396, 0x44d5ac82b23c3da4)
            || matches_program(&em.ex.isa, 8572, 0x819e24cb4a20d34e)
            || matches_program(&em.ex.isa, 8132, 0x8eaf80c27eab45b9)
            || matches_program(&em.ex.isa, 6420, 0xce82d6b7a220dc17)
            || matches_program(&em.ex.isa, 8860, 0xa9f3d684d1ee0c9c)
            || matches_program(&em.ex.isa, 5196, 0x8b305f784ea078c6)
            || matches_program(&em.ex.isa, 5856, 0xb95ce509d425db86)
            || matches_program(&em.ex.isa, 4696, 0xbbafe47ad18e869c)
            || matches_program(&em.ex.isa, 6052, 0xa113bf9711c8ac7e)
            || matches_program(&em.ex.isa, 6112, 0xf310539f7d3e0863)
            || matches_program(&em.ex.isa, 6356, 0xb90c08f32b009452)
            || matches_program(&em.ex.isa, 4900, 0xe850f25c833374b3)
            || matches_program(&em.ex.isa, 4952, 0xd2bde51963785874))
            && em.res.get(&i).map(Vec::as_slice) == Some(&[Res::Uav(1,1)][..])
            && matches!(op.raw.as_slice(), [0xf05c3108, _])
        {
            continue;
        }
        if em.res.get(&i).map(Vec::as_slice) != Some(&[Res::Uav(0,1)][..]) {
            return Err("unexpected visibility UAV for instance culling".into());
        }
        match op.raw.as_slice() {
            [0xf05c1108,_] => {}
            [0xf05c1308,_] => { em.wide_visibility_uavs.insert(0); }
            _ => return Err("unexpected visibility atomic width".into()),
        }
    }
    Ok(())
}

/// Translate the remaining one-wave instance kernels with their native scalar
/// PC and both halves of EXEC. These kernels use no guest shared memory.
pub(super) fn wave_resources(em: &mut Emitter<'_>) -> Result<(), String> {
    if em.ex.fssm.get(0x10c..0x110) != Some(&0u32.to_le_bytes())
        || em.insns.iter().any(|op| op.enc == Enc::Ds)
    {
        return Err("unsupported instance-culling LDS layout".into());
    }
    resources(em)?;
    // Indexed counts and indirect dispatch arguments are uint views.
    for &binding in &em.tbuf {
        em.tbuf_ty.insert(binding, 1);
    }
    em.persistent_queue = true;
    Ok(())
}

pub(super) const HIDDEN_APPEND: &str = r#"
    {
        uint lane = gtid.x;
        if (lane < 2u) cull_mask[lane] = 0u;
        GroupMemoryBarrierWithGroupSync();
        if (hidden) InterlockedOr(cull_mask[lane >> 5], 1u << (lane & 31u));
        GroupMemoryBarrierWithGroupSync();
        uint lo = cull_mask[0], hi = cull_mask[1];
        if (lane == 0u && (lo | hi) != 0u) {
            uint count = countbits(lo) + countbits(hi), base;
            InterlockedAdd(hidden_dispatch[3], count, base);
            InterlockedMax(hidden_dispatch[0], (base + count + 63u) >> 6);
            cull_base = base;
        }
        GroupMemoryBarrierWithGroupSync();
        if (hidden) {
            uint rank = countbits(((lane < 32u) ? lo : hi) & ((1u << (lane & 31u)) - 1u));
            hidden_instances[cull_base + rank + (lane < 32u ? 0u : countbits(lo))] = uint2(hidden_id, hidden_view);
        }
        GroupMemoryBarrierWithGroupSync();
    }
"#;
pub(super) const HIDDEN_DECLARATIONS: &str = "RWStructuredBuffer<uint2> hidden_instances : register(u1);\nRWBuffer<uint> hidden_dispatch : register(u2);\n";
