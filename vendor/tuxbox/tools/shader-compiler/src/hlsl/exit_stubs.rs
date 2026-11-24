//! Move loop-exit stubs back to the code they rejoin.
//!
//! The guest compiler places some loop exits after the end of the program: a
//! branch out of an inner loop lands on a few lane-local writes and an
//! `s_branch` back to the instruction after that loop. In a64767c5 four such
//! stubs (`v_mov_b32 v41, -1; s_branch`) sit past the final `s_endpgm`, so each
//! `s_branch` looks like the back edge of a loop spanning most of the program.
//! Those loops overlap, the structured emitter cannot nest them, and the
//! program fell back to the `tb_pc` block dispatcher (34 s at dxc -O1).
//!
//! A stub is a run of lane-local VALU writes ending in an unconditional
//! `s_branch` to T, reachable only by branches (the instruction before it never
//! falls through), whose every branch comes from before T. Placing it just
//! before T, behind a branch to T for the code that falls through, keeps every
//! path executing the same instructions with the same EXEC, and every branch
//! into it becomes a forward branch. Instructions keep their byte offsets, so
//! branch targets resolve unchanged; only their order in the list changes.
//! TB_INLINE_EXIT_STUBS=0 off.
use super::{Emitter, Enc, Insn, Opnd};
use std::collections::HashMap;

/// Longest stub body moved, in instructions.
const MAX_BODY: usize = 16;

fn enabled() -> bool {
    std::env::var("TB_INLINE_EXIT_STUBS").map_or(true, |v| v.trim() != "0")
}

fn is_branch(ins: &Insn) -> bool {
    ins.enc == Enc::Sopp && Emitter::branch_cond(ins.op).is_some()
}

/// Never falls through to the next instruction: `s_endpgm` or `s_branch`.
fn ends_flow(ins: &Insn) -> bool {
    ins.enc == Enc::Sopp && matches!(ins.op, 1 | 2)
}

/// Writes only VGPRs and reads nothing tied to its position or to other lanes.
fn lane_local(ins: &Insn) -> bool {
    if ins.mnemonic == "s_nop" {
        return true;
    }
    let m = ins.mnemonic.as_str();
    m.starts_with("v_")
        && !m.starts_with("v_cmp")
        && !m.contains("lane")
        && !m.contains("movrel")
        && !m.contains("swaprel")
        && !m.contains("dpp")
        && ins.aux.sdwa.is_none()
        && ins.dst.iter().all(|d| matches!(d, Opnd::Vgpr(_)))
        && ins.src.iter().all(|s| matches!(s, Opnd::Vgpr(_) | Opnd::Sgpr(_) | Opnd::ConstInt(_)
            | Opnd::ConstF32(_) | Opnd::Literal(_)))
}

fn target(ins: &Insn) -> i64 {
    ins.offset as i64 + 4 + ins.aux.imm * 4
}

/// One stub to move: body `start..=branch`, placed before index `to`.
fn find(insns: &[Insn]) -> Option<(usize, usize, usize)> {
    let off2idx: HashMap<usize, usize> = insns.iter().enumerate().map(|(i, x)| (x.offset, i)).collect();
    let idx = |t: i64| usize::try_from(t).ok().and_then(|t| off2idx.get(&t).copied());
    'stub: for (b, ins) in insns.iter().enumerate() {
        if !(ins.enc == Enc::Sopp && ins.op == 2) {
            continue;
        }
        let Some(to) = idx(target(ins)) else { continue };
        if to >= b {
            continue;
        }
        let mut start = b;
        while start > 0 && b - start < MAX_BODY && lane_local(&insns[start - 1]) {
            start -= 1;
        }
        if start == 0 || !ends_flow(&insns[start - 1]) || to >= start {
            continue;
        }
        let mut preds = 0;
        for (p, op) in insns.iter().enumerate().filter(|(_, op)| is_branch(op)) {
            match idx(target(op)) {
                Some(t) if t == start => {
                    if p >= to {
                        continue 'stub; // a real loop through the stub
                    }
                    preds += 1;
                }
                Some(t) if t > start && t <= b => continue 'stub,
                _ => {}
            }
        }
        if preds > 0 {
            return Some((start, b, to));
        }
    }
    None
}

/// Returns how many stubs moved.
pub(super) fn inline(insns: &mut Vec<Insn>) -> usize {
    if enabled() { inline_all(insns) } else { 0 }
}

fn inline_all(insns: &mut Vec<Insn>) -> usize {
    if insns.iter().any(|i| matches!(i.mnemonic.as_str(), "s_getpc_b64" | "s_setpc_b64" | "s_swappc_b64" | "s_call_b64"))
    {
        return 0;
    }
    let mut moved = 0;
    while let Some((start, b, to)) = find(insns) {
        let mut stub: Vec<Insn> = insns.drain(start..=b).collect();
        // The stub's own `s_branch` to T goes first: code falling through to T skips the body.
        let branch = stub.pop().expect("stub ends in its branch");
        stub.insert(0, branch);
        insns.splice(to..to, stub);
        moved += 1;
    }
    moved
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rdna2::InsnAux;

    fn op(offset: usize, mnemonic: &str, enc: Enc, code: u32, imm: i64, dst: Vec<Opnd>, src: Vec<Opnd>) -> Insn {
        Insn { offset, dwords: 1, enc, op: code, raw: vec![0], mnemonic: mnemonic.into(), dst, src,
            aux: InsnAux { imm, ..Default::default() } }
    }
    fn branch(offset: usize, mnemonic: &str, code: u32, to: usize) -> Insn {
        op(offset, mnemonic, Enc::Sopp, code, (to as i64 - offset as i64 - 4) / 4, vec![], vec![])
    }
    fn alu(offset: usize) -> Insn {
        op(offset, "v_mov_b32", Enc::Vop1, 1, 0, vec![Opnd::Vgpr(41)], vec![Opnd::ConstInt(-1)])
    }

    #[test]
    fn exit_stub_moves_before_its_rejoin() {
        // 0x00 loop: alu; 0x04 scc1 -> stub 0x20; 0x08 scc1 -> loop; 0x0c rejoin alu; 0x10 endpgm;
        // 0x14 stub? no: 0x14 v_mov, 0x18 s_branch -> 0x0c; 0x1c endpgm
        let mut code = vec![
            alu(0x00),
            branch(0x04, "s_cbranch_scc1", 5, 0x14),
            branch(0x08, "s_cbranch_scc0", 4, 0x00),
            alu(0x0c),
            op(0x10, "s_endpgm", Enc::Sopp, 1, 0, vec![], vec![]),
            alu(0x14),
            branch(0x18, "s_branch", 2, 0x0c),
            op(0x1c, "s_endpgm", Enc::Sopp, 1, 0, vec![], vec![]),
        ];
        assert_eq!(inline_all(&mut code), 1);
        let order: Vec<usize> = code.iter().map(|i| i.offset).collect();
        assert_eq!(order, [0x00, 0x04, 0x08, 0x18, 0x14, 0x0c, 0x10, 0x1c]);
    }

    #[test]
    fn loop_through_stub_stays() {
        // The branch into the stub comes after its target: that is a real loop.
        let mut code = vec![
            alu(0x00),
            branch(0x04, "s_cbranch_scc1", 5, 0x10),
            op(0x08, "s_endpgm", Enc::Sopp, 1, 0, vec![], vec![]),
            op(0x0c, "s_endpgm", Enc::Sopp, 1, 0, vec![], vec![]),
            alu(0x10),
            branch(0x14, "s_branch", 2, 0x00),
            op(0x18, "s_endpgm", Enc::Sopp, 1, 0, vec![], vec![]),
        ];
        assert_eq!(inline_all(&mut code), 0);
    }
}
