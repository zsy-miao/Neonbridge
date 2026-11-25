//! Find register arrays written under an explicit unsigned index bound. This
//! lets descriptor analysis invalidate just the VGPRs the guest can select.
use crate::ray::continuation::{target, writes};
use crate::rdna2::{Enc, Insn, Opnd};
use std::ops::Range;

pub(super) fn bounded_write(code: &[Insn], i: usize) -> Option<Range<usize>> {
    let [read, equal, group, m0, store, clear, repeat] = code.get(i.checked_sub(4)?..=i + 2)?
    else {
        return None;
    };
    let ([Opnd::Vgpr(index)], [Opnd::Sgpr(scalar)], [Opnd::Vgpr(base)]) = (
        read.src.as_slice(),
        read.dst.as_slice(),
        store.dst.as_slice(),
    ) else {
        return None;
    };
    if read.mnemonic != "v_readfirstlane_b32"
        || equal.mnemonic != "v_cmp_eq_u32"
        || equal.src != [Opnd::Sgpr(*scalar), Opnd::Vgpr(*index)]
        || group.mnemonic != "s_and_saveexec_b32"
        || group.src != equal.dst
        || m0.mnemonic != "s_mov_b32"
        || m0.dst != [Opnd::M0]
        || m0.src != read.dst
        || store.mnemonic != "v_movreld_b32"
        || clear.mnemonic != "s_xor_b32"
        || clear.dst != [Opnd::ExecLo]
        || clear.src != [Opnd::ExecLo, *group.dst.first()?]
        || repeat.mnemonic != "s_cbranch_execnz"
        || target(repeat) != Some(read.offset)
    {
        return None;
    }

    let before_loop = &code[..i - 4];
    let guard = before_loop
        .iter()
        .rposition(|ins| ins.mnemonic == "s_and_saveexec_b32")?;
    let skip = before_loop.get(guard + 1)?;
    if skip.mnemonic != "s_cbranch_execz"
        || target(skip)? <= repeat.offset
        || before_loop[guard].src != [Opnd::VccLo]
        || before_loop[guard + 2..].iter().any(|ins| {
            !matches!(ins.mnemonic.as_str(), "s_nop" | "s_waitcnt")
                && !(ins.mnemonic == "s_mov_b32"
                    && matches!(ins.dst.as_slice(), [Opnd::Sgpr(_)])
                    && ins.src == [Opnd::ExecLo])
        })
    {
        return None;
    }
    let predicate = before_loop[..guard]
        .iter()
        .rposition(|ins| ins.dst.contains(&Opnd::VccLo))?;
    let compare = &code[predicate];
    let [Opnd::ConstInt(limit), Opnd::Vgpr(tested)] = compare.src.as_slice() else {
        return None;
    };
    if compare.mnemonic != "v_cmp_gt_u32"
        || tested != index
        || before_loop[predicate + 1..guard].iter().any(|op| {
            // Arithmetic between the compare and its first mask save must
            // preserve both VCC and EXEC, including their implicit writes.
            !matches!(op.mnemonic.as_str(), "s_nop" | "s_waitcnt")
                && !(matches!(op.enc, Enc::Vop1 | Enc::Vop2 | Enc::Vop3)
                    && (op.mnemonic.ends_with("_f32") || op.mnemonic == "v_mov_b32")
                    && !op.mnemonic.starts_with("v_cmp")
                    && matches!(op.dst.as_slice(), [Opnd::Vgpr(_)])
                    && !writes(op, *index))
        })
        || code.iter().any(|op| {
            op.offset != repeat.offset
                && target(op).is_some_and(|pc| (compare.offset + 1..=repeat.offset).contains(&pc))
        })
    {
        return None;
    }
    let start = usize::from(*base);
    let end = start.checked_add(usize::try_from(*limit).ok()?)?;
    (end <= 256).then_some(start..end)
}
