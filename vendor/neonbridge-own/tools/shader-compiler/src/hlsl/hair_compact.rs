//! Workgroup-wide compaction of hair visibility samples.
//!
//! The programs restrict the reservation to local lane zero before the
//! election. Its active-lane count is one, its prefix is zero, and it consumes
//! its own returning atomic value. This remains true when the 64-thread group
//! spans multiple host waves. The three waits publish the initialization,
//! per-pixel additions and reservation, respectively, before their consumers.
use super::{
    compute_group, packed_root::matches_program, Emitter, Enc, Insn, Opnd, Res, ShaderExtract,
    Stage,
};

#[derive(Clone, Copy)]
pub(super) enum Layout {
    Direct,
    Indirect,
    Linked8,
    Linked16,
    Linked32,
}

pub(super) fn prepare(ex: &ShaderExtract, insns: &mut [Insn]) -> Option<Layout> {
    if ex.stage != Stage::Compute || ex.entry.as_deref() != Some("MainCS") {
        return None;
    }
    let [8, height, 1] = compute_group(ex) else {
        return None;
    };
    let (layout, fingerprint) = match (height, ex.isa.len()) {
        (8, 1836) => (Layout::Indirect, 0x9f0e1f2c8563a83b),
        (8, 1720) => (Layout::Direct, 0x63424472876e231b),
        (4, 1728) => (Layout::Linked16, 0xc9d1c0330df2e2a7),
        (8, 1728) => (Layout::Linked16, 0x000e91c9f2e6f9ca),
        (4, 1924) => (Layout::Linked32, 0xd0e2fae725b6cea1),
        (8, 1924) => (Layout::Linked32, 0xb65179d23a17aada),
        (4, 0x868) => (Layout::Linked8, 0x5be73e0f30f0ff2f),
        (8, 0x868) => (Layout::Linked8, 0x50e97b59aab6feab),
        (4, 0xa1c) => (Layout::Linked8, 0x43296ecdbad684c2),
        (8, 0xa1c) => (Layout::Linked8, 0x1e0ea8cd083441de),
        (4, 0x87c) => (Layout::Linked16, 0xe985332a4f5b9297),
        (8, 0x87c) => (Layout::Linked16, 0x471df984af51362a),
        (4, 0x944) => (Layout::Linked32, 0x47d413f7cd2bd88a),
        (8, 0x944) => (Layout::Linked32, 0x68ddd5d60ff22273),
        _ => return None,
    };
    if !matches_program(&ex.isa, ex.isa.len(), fingerprint) {
        return None;
    }
    // The linked-list shaders flatten the local xy coordinates at 0x60 or 0x78. VCC
    // from the local index == 0 comparison survives the LDS sum and is intersected with total != 0
    // before the reservation. All four MBCNTs in that region produce zero.
    let (sync, reservation) = match layout {
        Layout::Indirect => ([0x054, 0x4b8, 0x578], 0x4e4),
        Layout::Direct => ([0x0ac, 0x460, 0x51c], 0x488),
        Layout::Linked8 => ([0x444, 0x450, 0x504], 0x474),
        Layout::Linked16 => ([0x3dc, 0x3e8, 0x49c], 0x40c),
        Layout::Linked32 => ([0x494, 0x4a0, 0x554], 0x4c4),
    };
    for ins in insns {
        if sync.contains(&ins.offset) {
            ins.mnemonic = "tb_subsurface_group_sync".into();
        } else if (reservation..sync[2]).contains(&ins.offset) {
            match ins.mnemonic.as_str() {
                "v_mbcnt_lo_u32_b32" | "v_mbcnt_hi_u32_b32" => {
                    ins.enc = Enc::Vop1;
                    ins.op = 1;
                    ins.mnemonic = "v_mov_b32".into();
                    ins.src = vec![Opnd::ConstInt(0)];
                }
                "s_ff1_i32_b64" | "v_readfirstlane_b32" => {
                    if ins.mnemonic == "s_ff1_i32_b64" {
                        ins.src = vec![Opnd::ConstInt(0)];
                    }
                    ins.enc = Enc::Sop1;
                    ins.op = 3;
                    ins.mnemonic = "s_mov_b32".into();
                }
                _ => {}
            }
        }
    }
    Some(layout)
}

pub(super) fn resources(em: &mut Emitter<'_>, layout: Layout) -> Result<(), String> {
    let bindings: &[(usize, Res)] = match layout {
        Layout::Indirect => &[
            (0x058, Res::TBuf(6, false)),
            (0x0e0, Res::TBuf(7, false)),
            (0x69c, Res::TBuf(3, true)),
            (0x718, Res::Uav(1, 1)),
        ],
        Layout::Direct => &[(0x63c, Res::TBuf(3, true)), (0x6a4, Res::Uav(1, 1))],
        Layout::Linked8 | Layout::Linked16 | Layout::Linked32 => {
            // The velocity permutations have another typed UAV at u5 and
            // schedule stores differently. Check the actual resolved resources
            // for each opcode before selecting integer views for u1 and u3.
            for (idx, op) in em.insns.iter().enumerate() {
                let allowed: &[Res] = match op.mnemonic.as_str() {
                    "image_load" => &[Res::Tex(0, 1)],
                    "buffer_load_dword" => &[Res::Buf(1, false)],
                    "buffer_store_dword" => &[Res::Buf(2, true)],
                    "buffer_store_format_xy" => &[Res::TBuf(3, true)],
                    "image_store" if op.aux.mask == 3 => &[Res::Uav(5, 1)],
                    "image_store" => &[Res::Uav(1, 1), Res::Uav(4, 1)],
                    _ => continue,
                };
                if !em
                    .res
                    .get(&idx)
                    .and_then(|r| r.first())
                    .is_some_and(|r| allowed.contains(r))
                {
                    return Err(format!(
                        "unexpected linked-list hair binding at {:#x}",
                        op.offset
                    ));
                }
            }
            &[]
        }
    };
    for &(offset, expected) in bindings {
        let Some((idx, _)) = em
            .insns
            .iter()
            .enumerate()
            .find(|(_, i)| i.offset == offset)
        else {
            return Err("missing hair compaction binding".into());
        };
        if em.res.get(&idx).and_then(|r| r.first()).copied() != Some(expected) {
            return Err(format!("unexpected hair compaction binding at {offset:#x}"));
        }
    }
    // Count and tile coordinates, output pixel coordinates, and packed pixel
    // index/counts are integer views. Their bit patterns are not floats.
    if matches!(layout, Layout::Indirect) {
        em.tbuf_ty.extend([((false, 6), 1), ((false, 7), 1)]);
    }
    em.tbuf_ty.insert((true, 3), 1);
    em.img_ty.insert((true, 1), 1);
    Ok(())
}
