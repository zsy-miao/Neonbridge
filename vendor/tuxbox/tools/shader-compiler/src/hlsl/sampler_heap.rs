//! FSSM descriptor-table and root-constant entry registers.
use crate::{meta::RootParamKind, ShaderExtract, Stage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DirectSampler {
    pub register: u32,
    pub space: u32,
    pub count: u32,
    pub component: u32,
}

pub(super) fn constants(ex: &ShaderExtract) -> Option<(u8, DirectSampler)> {
    use tuxbox_dxbc::compute_table::records;
    let root = ex.root_sig.as_ref()?;
    let (index, start) = match (ex.stage, root.params.as_slice()) {
        (Stage::Pixel, [table, _, rest @ ..])
            if matches!(table.kind, RootParamKind::Table(_))
                && (rest.is_empty()
                    || matches!(rest, [map] if matches!(map.kind, RootParamKind::Descriptor {kind: 3, register: 0, space: 32766}))) =>
        {
            (1, 9u8)
        }
        (Stage::Compute, [table, _]) if matches!(table.kind, RootParamKind::Table(_)) => (1, 5),
        (Stage::Compute, [first, second, _])
            if matches!(first.kind, RootParamKind::Table(_))
                && matches!(second.kind, RootParamKind::Table(_)) =>
        {
            (2, 6)
        }
        _ => return None,
    };
    let constants = &root.params[index];
    let RootParamKind::Constants {
        register,
        space,
        num32,
    } = constants.kind
    else {
        return None;
    };
    if !(1..=4).contains(&num32)
        || !records(&ex.fssm)?.any(|[class, offset, reg, _, location]| {
            class == 2
                && offset == 0
                && reg == register
                && location == ((index as u32) << 16 | (u32::from(start) << 8) | 4)
        })
    {
        return None;
    }
    if !ex
        .resources
        .iter()
        .any(|r| (r.res_type, r.space, r.lower, r.upper) == (2, space, register, register))
    {
        return None;
    }
    Some((
        start,
        DirectSampler {
            register,
            space,
            count: num32,
            component: 0,
        },
    ))
}

pub(super) fn init_constants(
    ex: &ShaderExtract,
    regs: &std::collections::BTreeSet<String>,
    output: &mut String,
) {
    use std::fmt::Write;
    let Some((start, key)) = constants(ex) else {
        return;
    };
    for component in 0..key.count {
        let reg = format!("s{}", u32::from(start) + component);
        if regs.contains(&reg) {
            writeln!(
                output,
                "    {reg} = tb_index{}_{}.{};",
                key.register,
                key.space,
                super::XYZW[component as usize]
            )
            .unwrap();
        }
    }
}

pub(super) fn index(ex: &ShaderExtract, entry: u8) -> Option<DirectSampler> {
    if ex.stage != Stage::Pixel || ex.root_sig.as_ref()?.flags & 0x800 == 0 {
        return None;
    }
    let (start, mut key) = constants(ex)?;
    key.component = u32::from(entry.checked_sub(start)?);
    (key.component < key.count).then_some(key)
}

/// These vertex shaders receive the table in s8 even after their generated fetch call.
pub(super) fn vertex_s8_table(ex: &ShaderExtract) -> bool {
    ex.stage == Stage::Vertex
        && tuxbox_dxbc::compute_table::records(&ex.fssm).is_some_and(|mut rows| {
            rows.len() != 0
                && rows.all(|[class, _, _, _, loc]| matches!(class, 0x302 | 0x300) && loc >> 8 == 8)
        })
}

/// Confirm that the pixel-stage resource uses the table slot in the shared raster root.
pub(super) fn matches_table(
    ex: &ShaderExtract,
    offset: u32,
    kind: u32,
    reg: u32,
    space: u32,
) -> bool {
    let (class, psv_type) = match kind {
        0 => (0x300, 3),
        1 => (0x303, 6),
        2 => (0x302, 2),
        _ => return false,
    };
    tuxbox_dxbc::compute_table::records(&ex.fssm).is_some_and(|mut rows| {
        rows.any(|[ty, pos, slot, _, _]| {
            ty == class && pos.checked_mul(4) == Some(offset) && slot == reg
        })
    }) && ex
        .resources
        .iter()
        .any(|r| (r.res_type, r.lower, r.upper, r.space) == (psv_type, reg, reg, space))
}

/// Requires the actual resource heap, its FSSM user-data location, and the
/// runtime's private guest-to-host descriptor index map in the raster root.
pub(super) fn resource_base(ex: &ShaderExtract) -> Option<u8> {
    let root = ex.root_sig.as_ref()?;
    if ex.stage != Stage::Pixel
        || ex.required_features? & (3 << 25) != 1 << 25
        || root.flags & 0x400 == 0
        || !matches!(root.params.as_slice(), [_, _, map] if matches!(map.kind, RootParamKind::Descriptor { kind: 3, register: 0, space: 32766 }))
    {
        return None;
    }
    let (_, c) = constants(ex)?;
    tuxbox_dxbc::compute_table::scalar_heap_operand(&ex.isa, c.count)?;
    u8::try_from(tuxbox_dxbc::compute_table::pixel_heap_base(
        &ex.fssm, c.count,
    )?)
    .ok()
}

/// Match the literal RDNA2 state of the normalized, isotropic samplers used
/// with direct textures to a static sampler in the supplied root signature.
/// Require every serialized state field before selecting a register.
pub(super) fn inline_sampler(ex: &ShaderExtract, literal: [u32; 4]) -> Option<(u32, u32)> {
    fn address(mode: u32) -> Option<u32> {
        match mode {
            1 => Some(0),
            3 => Some(2),
            4 => Some(6),
            _ => None,
        }
    }
    ex.root_sig.as_ref()?.static_samplers.iter().find_map(|s| {
        let [filter, u, v, w, bias, aniso, compare, border, min, max, slot, space, vis] = *s;
        if !(vis == 0 || (vis == 5 && ex.stage == Stage::Pixel))
            || (bias, aniso, compare, border, min, max) != (0, 1, 1, 0, 0, 12.0_f32.to_bits())
            || !ex
                .resources
                .iter()
                .any(|r| (r.res_type, r.lower, r.upper, r.space) == (1, slot, slot, space))
        {
            return None;
        }
        let filter = match filter {
            0 => 0x14000000,
            0x14 => 0x14500000,
            0x15 => 0x18500000,
            _ => return None,
        };
        let clamp = address(u)? | address(v)? << 3 | address(w)? << 6;
        (literal == [clamp, 0x00c00000, filter, 0]).then_some((slot, space))
    })
}

/// An empty EXEC branch can skip inline sampler initialization only when the
/// immediately following image instruction is also inactive. In a full
/// compute wave ANDing EXEC with the preserved launch mask changes no bits.
pub(super) fn guarded_words(
    insns: &[super::Insn],
    image: usize,
    base: u8,
    group: [u32; 3],
) -> Option<[u32; 4]> {
    use super::{Emitter, Enc, Opnd};
    use crate::rdna2::clobbers_scalar;
    let n = group.into_iter().try_fold(1u32, |n, w| n.checked_mul(w))?;
    if n == 0 || n % 64 != 0 {
        return None;
    }
    let restore = image.checked_sub(1)?;
    let r = &insns[restore];
    let [Opnd::Sgpr(saved)] = *r.src.as_slice() else {
        return None;
    };
    if r.mnemonic != "s_mov_b64" || r.dst != [Opnd::ExecLo] {
        return None;
    }
    let target = |i: &super::Insn| i.offset as i64 + 4 + i.aux.imm * 4;
    let branch = insns[..restore]
        .iter()
        .rposition(|i| i.mnemonic == "s_cbranch_execz" && target(i) == r.offset as i64)?;
    let guard = &insns[branch.checked_sub(1)?];
    let [Opnd::Sgpr(entry)] = *guard.src.as_slice() else {
        return None;
    };
    if guard.mnemonic != "s_and_saveexec_b64" || guard.dst != [Opnd::Sgpr(saved)] || saved == entry
    {
        return None;
    }
    let capture = insns[..branch - 1].iter().position(|i| {
        i.mnemonic == "s_mov_b64" && i.dst == [Opnd::Sgpr(entry)] && i.src == [Opnd::ExecLo]
    })?;
    if insns[..capture].iter().any(|i| {
        i.mnemonic.contains("branch")
            || i.mnemonic.contains("exec")
            || i.mnemonic.starts_with("v_cmpx")
            || i.dst
                .iter()
                .any(|d| matches!(d, Opnd::ExecLo | Opnd::ExecHi))
    }) {
        return None;
    }
    if insns[capture + 1..branch - 1]
        .iter()
        .any(|i| clobbers_scalar(i, entry) || clobbers_scalar(i, entry + 1))
    {
        return None;
    }
    if insns.iter().enumerate().any(|(j, i)| {
        matches!(
            i.mnemonic.as_str(),
            "s_setpc_b64" | "s_swappc_b64" | "s_call_b64"
        ) || (i.enc == Enc::Sopp
            && Emitter::branch_cond(i.op).is_some()
            && (target(i) <= insns[capture].offset as i64
                || ((guard.offset as i64 + 1..=insns[image].offset as i64).contains(&target(i))
                    && j != branch)))
    }) {
        return None;
    }
    let mut words = [0; 4];
    let block = &insns[branch + 1..restore];
    if block.iter().any(|i| {
        i.mnemonic.contains("branch") || clobbers_scalar(i, saved) || clobbers_scalar(i, saved + 1)
    }) {
        return None;
    }
    for (j, out) in words.iter_mut().enumerate() {
        let reg = base.checked_add(j as u8)?;
        let i = block.iter().rev().find(|op| clobbers_scalar(op, reg))?;
        if i.dst != [Opnd::Sgpr(reg)] {
            return None;
        }
        *out = match (i.mnemonic.as_str(), i.src.as_slice()) {
            ("s_movk_i32", []) => i.aux.imm as i16 as i32 as u32,
            ("s_mov_b32", [Opnd::ConstInt(v)]) => *v as u32,
            ("s_mov_b32", [Opnd::Literal(v)]) => *v,
            ("s_brev_b32", [Opnd::ConstInt(v)]) => (*v as u32).reverse_bits(),
            ("s_brev_b32", [Opnd::Literal(v)]) => v.reverse_bits(),
            _ => return None,
        };
    }
    Some(words)
}
