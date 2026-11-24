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
pub(super) fn constant_reads(group: [u32; 3], insns: &mut [Insn]) -> bool {
    let count = group.into_iter().fold(1u32, u32::saturating_mul);
    if count == 0 || count > 1024 || count % 64 != 0 { return false; }
    let Some(last) = insns.iter().rposition(|op| op.mnemonic == "v_readlane_b32") else { return false };
    let end = insns[last].offset as i64;
    if insns.iter().any(|op| {
        let name = op.mnemonic.as_str();
        let branch = op.enc == Enc::Sopp && Emitter::branch_cond(op.op).is_some();
        name == "v_writelane_b32" || matches!(name, "s_setpc_b64" | "s_swappc_b64" | "s_call_b64")
            || (op.offset as i64 <= end && name == "s_endpgm")
            || (name.contains("branch") && (!branch || op.aux.imm < 0))
            || (name == "v_readlane_b32" && !matches!((&op.dst[..], &op.src[..]),
                ([Opnd::Vgpr(_)], [Opnd::Vgpr(_), Opnd::ConstInt(0..=63), ..])))
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
mod tests {
    use super::*;

    fn recognizes(words: &[u32], group: [u32; 3]) -> bool {
        let isa: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        prepare(group, &mut crate::rdna2::disassemble(&isa))
    }

    #[test]
    fn empty_wave_fallback_cannot_be_entered_from_another_edge() {
        let pruned = |words: &[u32]| {
            let isa: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
            let mut ops = crate::rdna2::disassemble(&isa);
            skip_empty_wave_fallback(&mut ops)
                && !ops.iter().any(|op| op.mnemonic == "v_readlane_b32")
        };
        let words = [
            0xbf800000, 0xbeea037e, 0xbf870002, 0xd760000b, 0x00000d01, 0xbf800000, 0xbf810000,
        ];
        assert!(pruned(&words));
        for (index, word) in [
            (0, 0xbefe0380),
            (1, 0xbeea0380),
            (2, 0xbf860002),
            (5, 0xbf82fffd),
            (5, 0xbf82fffb),
            (5, 0xbe802000),
        ] {
            let mut changed = words;
            changed[index] = word;
            assert!(!pruned(&changed));
        }
    }

    #[test]
    fn lane_election_requires_an_intact_full_wave_predicate() {
        let words = [
            0xd4850004, 0x00010081, // v_cmp_ne_i32 s[4:5], 1, 0
            0xbf800000, // s_nop
            0xbed01404, // s_ff1_i32_b64 s80, s[4:5]
            0xd766000c, 0x00020cc1, // v_mbcnt_hi_u32_b32
            0xd7600000, 0x0000a104, // v_readlane_b32 s0, v4, s80
            0xbf810000,
        ];
        assert!(recognizes(&words, [16, 16, 1]));
        assert!(!recognizes(&words, [32, 1, 1]));
        for change in [0xbe850380, 0xbf820001, 0xbefe047e] {
            let mut changed = words;
            changed[2] = change; // clobber mask high, branch, write EXEC
            assert!(!recognizes(&changed, [16, 16, 1]));
        }
        let mut equal = words;
        equal[1] = 0x00010080; // empty mask
        assert!(!recognizes(&equal, [16, 16, 1]));
    }
}
