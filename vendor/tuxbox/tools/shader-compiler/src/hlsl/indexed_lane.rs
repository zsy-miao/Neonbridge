//! Full-wave elections in branch-free compute programs. Each participant reads
//! the requested guest wave64 lane by its linear workgroup index, independently
//! of physical subgroup width or the driver's assignment of invocations.
use super::{writes_scalar, Emitter, Enc, Insn, Opnd};

/// A launched compute wave enters with an active low lane. The compiler emits an
/// out-of-line lane-number fallback behind a guard that always bypasses it.
pub(super) fn skip_empty_wave_fallback(insns: &mut Vec<Insn>) -> bool {
    let branch_target = |op: &Insn| op.offset as i64 + 4 + op.aux.imm * 4;
    let branch = |op: &Insn| op.enc == Enc::Sopp && Emitter::branch_cond(op.op).is_some();
    let Some(guard) = insns.iter().position(branch) else {
        return false;
    };
    if guard == 0
        || insns[guard - 1].raw != [0xbeea037e]
        || insns[guard].mnemonic != "s_cbranch_vccnz"
    {
        return false;
    }
    // The initial copy of EXEC_LO to VCC_LO must still see the launch mask.
    if insns[..guard - 1].iter().any(|op| {
        op.mnemonic.contains("exec")
            || op.mnemonic.starts_with("v_cmpx")
            || op.mnemonic == "s_endpgm"
            || op
                .dst
                .iter()
                .any(|v| matches!(v, Opnd::ExecLo | Opnd::ExecHi))
    }) {
        return false;
    }
    // A dynamic transfer or a later jump into the prefix could enter the
    // fallback or revisit its guard with a different EXEC mask.
    let target = branch_target(&insns[guard]);
    let Some(join) = insns.iter().position(|op| op.offset as i64 == target) else {
        return false;
    };
    if join <= guard + 1
        || insns.iter().any(|op| {
            matches!(
                op.mnemonic.as_str(),
                "s_setpc_b64" | "s_swappc_b64" | "s_call_b64"
            ) || (op.mnemonic.contains("branch") && !branch(op))
        })
        || insns[join..]
            .iter()
            .any(|op| branch(op) && branch_target(op) < target)
    {
        return false;
    }
    if !insns[guard + 1..join]
        .iter()
        .any(|op| op.mnemonic == "v_readlane_b32" && matches!(op.src.get(1), Some(Opnd::Sgpr(_))))
    {
        return false;
    }
    insns.drain(guard..join);
    true
}

/// A constant read in a converged group observes another invocation's VGPR,
/// including lane 32 on hosts with 32-lane subgroups. Keep SGPR spills separate.
pub(super) fn constant_reads(group: [u32; 3], insns: &mut [Insn], allow_small_group: bool) -> bool {
    let count = group.into_iter().fold(1u32, u32::saturating_mul);
    // Up to 32 threads fit in one guest wave of either width. Only read a
    // lane that actually launched, even when its selector comes from an SGPR.
    let small = allow_small_group && (1..=32).contains(&count);
    if count == 0 || count > 1024 || (!small && count % 64 != 0) { return false; }
    let Some(last) = insns.iter().rposition(|op| op.mnemonic == "v_readlane_b32") else { return false };
    let end = insns[last].offset as i64;
    let selectors = if small {
        super::scalar_values::at_reads(insns, super::scalar_values::Proof::UpperBound, |op| match (op.mnemonic.as_str(), op.src.get(1)) {
            ("v_readlane_b32", Some(Opnd::Sgpr(reg))) => Some(*reg),
            _ => None,
        })
    } else { Default::default() };
    let upper_lane = |op: &Insn| match op.src.get(1)? {
        Opnd::ConstInt(n @ 0..=63) => Some(*n as u32),
        Opnd::Sgpr(reg) => selectors.get(&(op.offset, *reg)).copied(),
        _ => None,
    };
    if insns.iter().any(|op| {
        let name = op.mnemonic.as_str();
        let branch = op.enc == Enc::Sopp && Emitter::branch_cond(op.op).is_some();
        name == "v_writelane_b32" || matches!(name, "s_setpc_b64" | "s_swappc_b64" | "s_call_b64")
            || (op.offset as i64 <= end && name == "s_endpgm")
            || (name.contains("branch") && (!branch || op.aux.imm < 0))
            || (name == "v_readlane_b32" && (!matches!((&op.dst[..], op.src.first()),
                ([Opnd::Vgpr(_)], Some(Opnd::Vgpr(_))))
                || !upper_lane(op).is_some_and(|n| n < count.min(64))))
    }) { return false; }
    for op in insns.iter_mut().filter(|op| op.mnemonic == "v_readlane_b32") {
        let Opnd::Vgpr(dst) = op.dst[0] else { unreachable!() };
        op.enc = Enc::Sop1;
        op.mnemonic = "tb_indexed_readlane".into();
        op.dst[0] = Opnd::Sgpr(dst);
        op.src.truncate(2);
    }
    true
}

pub(super) fn prepare(group: [u32; 3], insns: &mut [Insn]) -> bool {
    if !insns
        .iter()
        .any(|op| op.mnemonic == "v_readlane_b32" && matches!(op.src.get(1), Some(Opnd::Sgpr(_))))
        || !insns.iter().any(|op| op.mnemonic == "v_mbcnt_hi_u32_b32")
    {
        return false;
    }
    let count = group.into_iter().fold(1u32, u32::saturating_mul);
    if count == 0 || count > 1024 || count % 64 != 0 {
        return false;
    }
    // Every invocation in every guest wave must participate in the group
    // exchange. Keep arbitrary EXEC, LDS and branching programs on their
    // existing translation paths until their convergence is established.
    if insns.iter().enumerate().any(|(i, op)| {
        op.enc == Enc::Ds
            || op.mnemonic.contains("branch")
            || op.mnemonic.contains("exec")
            || op.mnemonic.starts_with("v_cmpx")
            || matches!(
                op.mnemonic.as_str(),
                "s_setpc_b64" | "s_swappc_b64" | "s_call_b64" | "v_writelane_b32"
            )
            || (op.mnemonic == "s_endpgm" && i + 1 < insns.len())
            || op
                .dst
                .iter()
                .any(|r| matches!(r, Opnd::ExecLo | Opnd::ExecHi))
            || (op.mnemonic.starts_with("v_mbcnt_") && op.src.first() != Some(&Opnd::ConstInt(-1)))
    }) {
        return false;
    }
    let mut reads = Vec::new();
    for i in 0..insns.len() {
        let op = &insns[i];
        let ([Opnd::Vgpr(dst)], [Opnd::Vgpr(source), Opnd::Sgpr(reg), ..]) =
            (&op.dst[..], &op.src[..])
        else {
            continue;
        };
        let (dst, value, selector) = (*dst, Opnd::Vgpr(*source), Opnd::Sgpr(*reg));
        if op.mnemonic != "v_readlane_b32"
            || op.raw
                != [
                    0xd7600000 | u32::from(dst),
                    (u32::from(*reg) << 9) | 256 | u32::from(*source),
                ]
        {
            continue;
        }
        let Some(elected) = insns[..i].iter().rposition(|op| writes_scalar(op, *reg)) else {
            continue;
        };
        let select = &insns[elected];
        let [Opnd::Sgpr(mask)] = select.src[..] else {
            continue;
        };
        if select.mnemonic != "s_ff1_i32_b64" || select.dst != [selector] {
            continue;
        }
        let Some(compare) = insns[..elected]
            .iter()
            .rev()
            .find(|op| writes_scalar(op, mask) || writes_scalar(op, mask.saturating_add(1)))
        else {
            continue;
        };
        // A comparison of unequal literal integers sets all 64 predicate bits.
        // FF1 of that complete mask selects guest lane zero, even on wave32 hosts.
        if compare.dst != [Opnd::Sgpr(mask)]
            || compare.mnemonic != "v_cmp_ne_i32"
            || !matches!(compare.src[..], [Opnd::ConstInt(a), Opnd::ConstInt(b), ..] if a != b)
            || compare.raw[0] & 0xff00 != 0
            || compare.raw[1] >> 27 != 0
        {
            continue;
        }
        reads.push((elected, i, dst, value, selector));
    }
    let changed = !reads.is_empty();
    for (elected, i, dst, value, selector) in reads {
        let select = &mut insns[elected];
        select.mnemonic = "s_mov_b32".into();
        select.op = 3;
        select.src = vec![Opnd::ConstInt(0)];
        let read = &mut insns[i];
        read.mnemonic = "tb_indexed_readlane".into();
        read.enc = Enc::Sop1;
        read.dst = vec![Opnd::Sgpr(dst)];
        read.src = vec![value, selector];
    }
    changed
}

#[cfg(test)]
#[path = "../tests/hlsl/indexed_lane.rs"]
mod tests;
