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
    if std::env::var("TB_ENDPGM_BRANCH_EXIT").map_or(true, |v| v.trim() != "0") {
        endpgm_exits(insns);
    }
    if enabled() { inline_all(insns) } else { 0 }
}

/// A backward branch to an `s_endpgm` leaves the program, which the emitters already
/// express as a branch to the end (an early `s_endpgm` is `j{end} = true`). Taken as a
/// back edge, it made 60310787 look like a loop from that `s_endpgm` to the branch,
/// overlapping its real loops, and the program went to the tb_pc dispatcher (71 s at
/// dxc -O1). Point it at the end of the program instead. TB_ENDPGM_BRANCH_EXIT=0 off.
fn endpgm_exits(insns: &mut [Insn]) -> usize {
    let Some(last) = insns.last() else { return 0 };
    let end = (last.offset + last.dwords * 4) as i64;
    let endpgm: std::collections::HashSet<usize> =
        insns.iter().filter(|i| i.enc == Enc::Sopp && i.op == 1).map(|i| i.offset).collect();
    let mut n = 0;
    for ins in insns.iter_mut().filter(|i| is_branch(i)) {
        let t = target(ins);
        if t <= ins.offset as i64 && usize::try_from(t).is_ok_and(|t| endpgm.contains(&t)) {
            ins.aux.imm = (end - ins.offset as i64 - 4) / 4;
            n += 1;
        }
    }
    n
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
    if std::env::var("TB_MOVE_BLOCKS").map_or(true, |v| v.trim() != "0") {
        moved += move_blocks(insns);
    }
    moved
}

/// Branch targets as positions in `order`; `n` is the end of the program.
struct View<'a> {
    insns: &'a [Insn],
    order: &'a [usize],
    pos: HashMap<usize, usize>,
    end: i64,
}

impl<'a> View<'a> {
    fn new(insns: &'a [Insn], order: &'a [usize]) -> Self {
        let pos = order.iter().enumerate().map(|(p, &i)| (insns[i].offset, p)).collect();
        let end = insns.last().map_or(0, |l| (l.offset + l.dwords * 4) as i64);
        View { insns, order, pos, end }
    }
    fn at(&self, p: usize) -> &Insn {
        &self.insns[self.order[p]]
    }
    fn target(&self, p: usize) -> Option<usize> {
        let t = target(self.at(p));
        if t >= self.end { Some(self.order.len()) } else { usize::try_from(t).ok().and_then(|t| self.pos.get(&t).copied()) }
    }
    /// Pairs of loops that overlap without nesting (the structured form needs none).
    fn interleaved(&self) -> usize {
        let mut heads: HashMap<usize, usize> = HashMap::new();
        for p in 0..self.order.len() {
            if is_branch(self.at(p)) {
                if let Some(t) = self.target(p).filter(|&t| t <= p) {
                    let e = heads.entry(t).or_insert(p);
                    *e = (*e).max(p);
                }
            }
        }
        let loops: Vec<(usize, usize)> = heads.into_iter().collect();
        let mut n = 0;
        for (i, &(l1, b1)) in loops.iter().enumerate() {
            for &(l2, b2) in &loops[i + 1..] {
                let disjoint = b1 < l2 || b2 < l1;
                let nested = (l1 <= l2 && b2 <= b1) || (l2 <= l1 && b1 <= b2);
                n += usize::from(!(disjoint || nested));
            }
        }
        n
    }
}

enum Move {
    /// Region `a..=b` goes before its target H, its closing `s_branch` first.
    Before { a: usize, b: usize, h: usize },
    /// Region `a..=b` goes after the flow-ending instruction at `p`.
    After { a: usize, b: usize, p: usize },
}

impl Move {
    fn apply(&self, order: &[usize]) -> Vec<usize> {
        let mut out = Vec::with_capacity(order.len());
        match *self {
            Move::Before { a, b, h } => {
                out.extend_from_slice(&order[..h]);
                out.push(order[b]);
                out.extend_from_slice(&order[a..b]);
                out.extend_from_slice(&order[h..a]);
            }
            Move::After { a, b, p } => {
                out.extend_from_slice(&order[..=p]);
                out.extend_from_slice(&order[a..=b]);
                out.extend_from_slice(&order[p + 1..a]);
            }
        }
        out.extend_from_slice(&order[match *self { Move::Before { b, .. } | Move::After { b, .. } => b + 1 }..]);
        out
    }
}

/// Region `a..=b` (entered only by branches, ending in the `s_branch` at `b` to `h`, every
/// branch inside staying inside, going to `h` or leaving the program) and where it can go.
fn region_move(v: &View, branches: &[usize], a: usize, b: usize, h: usize) -> Option<Move> {
    let n = v.order.len();
    let mut entries = Vec::new();
    for &p in branches {
        let t = v.target(p)?;
        if (a..=b).contains(&p) {
            if p != b && !((a..=b).contains(&t) || t == n || t == h) {
                return None;
            }
        } else if (a..=b).contains(&t) {
            entries.push(p);
        }
    }
    let last = *entries.iter().max()?;
    if last < h {
        // Before h, a branch inside to its own closing `s_branch` would point backwards.
        if branches.iter().any(|&p| (a..b).contains(&p) && v.target(p) == Some(b)) {
            return None;
        }
        return Some(Move::Before { a, b, h });
    }
    let p = (last + 1..a).find(|&q| ends_flow(v.at(q)))?;
    (p >= h).then_some(Move::After { a, b, p })
}

/// Out-of-line blocks that make loops overlap. The guest compiler places some blocks
/// after the code that follows them: in 4326f79f the continue path of the loop at 0x6188
/// (0x7420..0x76c4) sits past the end of the enclosing loop at 0x6100, and three exits
/// to the shared tail at 0x8168 sit past that tail. A block that is entered only by
/// branches and ends in an unconditional `s_branch` runs the same wherever it is placed,
/// so it can go before its target (behind that `s_branch`, as the stubs above) or after
/// any instruction that does not fall through. Only programs whose loops overlap are
/// touched, and a move is kept only when fewer loops overlap after it (a move before
/// the target may also keep the count). TB_MOVE_BLOCKS=0 off.
fn move_blocks(insns: &mut Vec<Insn>) -> usize {
    let mut order: Vec<usize> = (0..insns.len()).collect();
    let mut cur = View::new(insns, &order).interleaved();
    let mut moved = 0;
    'round: while cur > 0 && moved < 64 {
        let v = View::new(insns, &order);
        let branches: Vec<usize> = (0..order.len()).filter(|&p| is_branch(v.at(p))).collect();
        for &b in &branches {
            if !(v.at(b).enc == Enc::Sopp && v.at(b).op == 2) {
                continue;
            }
            let Some(h) = v.target(b).filter(|&h| h < b) else { continue };
            for a in (h + 1..=b).rev() {
                if !ends_flow(v.at(a - 1)) {
                    continue;
                }
                let Some(m) = region_move(&v, &branches, a, b, h) else { continue };
                let trial = m.apply(&order);
                let after = View::new(insns, &trial).interleaved();
                if after < cur || (matches!(m, Move::Before { .. }) && after == cur) {
                    order = trial;
                    cur = after;
                    moved += 1;
                    continue 'round;
                }
            }
        }
        break;
    }
    if moved > 0 {
        let mut old: Vec<Option<Insn>> = insns.drain(..).map(Some).collect();
        insns.extend(order.iter().map(|&i| old[i].take().expect("each instruction once")));
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
    fn backward_branch_to_endpgm_leaves_the_program() {
        let mut code = vec![
            alu(0x00),
            op(0x04, "s_endpgm", Enc::Sopp, 1, 0, vec![], vec![]),
            alu(0x08),
            branch(0x0c, "s_cbranch_scc1", 5, 0x04),
            op(0x10, "s_endpgm", Enc::Sopp, 1, 0, vec![], vec![]),
        ];
        assert_eq!(endpgm_exits(&mut code), 1);
        assert_eq!(target(&code[3]), 0x14);
    }

    #[test]
    fn continue_block_moves_into_the_loop_it_continues() {
        // Loop W 0x00..0x14 and loop Y 0x04..0x20 overlap: Y's continue block 0x1c..0x20
        // sits past W. After 0x0c (Y's other back edge, no fall-through) Y nests in W.
        let end = || op(0, "s_endpgm", Enc::Sopp, 1, 0, vec![], vec![]);
        let mut code = vec![
            alu(0x00),
            alu(0x04),
            branch(0x08, "s_cbranch_scc1", 5, 0x1c),
            branch(0x0c, "s_branch", 2, 0x04),
            alu(0x10),
            branch(0x14, "s_cbranch_scc0", 4, 0x00),
            Insn { offset: 0x18, ..end() },
            alu(0x1c),
            branch(0x20, "s_branch", 2, 0x04),
            Insn { offset: 0x24, ..end() },
        ];
        assert_eq!(inline_all(&mut code), 1);
        let order: Vec<usize> = code.iter().map(|i| i.offset).collect();
        assert_eq!(order, [0x00, 0x04, 0x08, 0x0c, 0x1c, 0x20, 0x10, 0x14, 0x18, 0x24]);
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
