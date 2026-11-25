//! A console program's control flow over its basic blocks: successors, post-dominators and
//! control dependence – which blocks run (or not) because of which branch, for analyses that
//! follow what a wave-wide vote decides.
//!
//! Ported from xcl's crates/xcl/src/gpu/recomp/cfg.rs (commits dfc5578 / 5796cc4, emirduran707,
//! GPL-2.0, copied with credit), adapted to `crate::rdna2::Insn`. Loaded by `partition.rs`
//! through a `#[path]` module, so it needs no `mod` line of its own.

use std::collections::BTreeMap;
use std::ops::Range;

use super::D;

/// Basic blocks of a program (instructions in address order).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Cfg {
    /// first instruction of each block; block `b` is `start[b]..start[b + 1]` (the last one ends
    /// at the program's end)
    pub start: Vec<usize>,
    /// the block of each instruction
    pub block_of: Vec<usize>,
    /// successors of each block (in the order the branch gives them: target first)
    pub succ: Vec<Vec<usize>>,
    /// predecessors of each block
    pub pred: Vec<Vec<usize>>,
    len: usize,
}

/// `Cfg::post_dominators`: the immediate post-dominator of a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PostDom {
    /// this block
    Block(usize),
    /// the program's end: every path from the block ends the program without a common block
    Exit,
    /// no path from the block ends the program (a loop with no way out)
    None,
}

/// A fetch-shader call (`s_swappc_b64 sX, sX`): it returns to the next instruction.
fn fetch_call(d: &D) -> bool {
    d.name == "S_SWAPPC_B64" && d.f("SDST") == d.f("SSRC0")
}

/// Successors of each instruction (index); None: unknown (an indirect jump, or a branch whose
/// target is not an instruction of the program). Replaces xcl's `upper_lanes::successors`.
fn successors(insts: &[D], pcs: &BTreeMap<i64, usize>) -> Vec<Option<Vec<usize>>> {
    insts
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let next = (i + 1 < insts.len()).then_some(i + 1);
            let n = d.name.as_str();
            if fetch_call(d) {
                return Some(next.into_iter().collect());
            }
            if matches!(n, "S_SETPC_B64" | "S_SWAPPC_B64" | "S_RFE_B64" | "S_CALL_B64" | "S_TRAP") {
                return None;
            }
            if n.starts_with("S_ENDPGM") {
                return Some(Vec::new());
            }
            let branch = n == "S_BRANCH";
            let conditional = n.starts_with("S_CBRANCH_");
            if branch || conditional {
                let target = d.insn.offset as i64 + 4 + 4 * d.insn.aux.imm;
                let mut out = vec![*pcs.get(&target)?];
                if conditional {
                    out.extend(next);
                }
                return Some(out);
            }
            Some(next.into_iter().collect())
        })
        .collect()
}

impl Cfg {
    /// None when the program has an indirect jump (its successors are not known).
    pub fn new(insts: &[D]) -> Option<Cfg> {
        let n = insts.len();
        let pcs: BTreeMap<i64, usize> = insts.iter().enumerate().map(|(i, d)| (d.insn.offset as i64, i)).collect();
        let succ_i: Vec<Vec<usize>> = successors(insts, &pcs).into_iter().collect::<Option<_>>()?;
        let mut leader = vec![false; n];
        if n > 0 {
            leader[0] = true;
        }
        for (i, s) in succ_i.iter().enumerate() {
            // anything but a plain fall-through ends a block, and every target starts one
            if s.as_slice() != [i + 1] {
                if i + 1 < n {
                    leader[i + 1] = true;
                }
                for &t in s {
                    leader[t] = true;
                }
            }
        }
        let start: Vec<usize> = (0..n).filter(|&i| leader[i]).collect();
        let mut block_of = vec![0; n];
        for (b, &s) in start.iter().enumerate() {
            let end = start.get(b + 1).copied().unwrap_or(n);
            block_of[s..end].fill(b);
        }
        let mut succ: Vec<Vec<usize>> = vec![Vec::new(); start.len()];
        let mut pred: Vec<Vec<usize>> = vec![Vec::new(); start.len()];
        for b in 0..start.len() {
            let last = start.get(b + 1).copied().unwrap_or(n) - 1;
            for &t in &succ_i[last] {
                let tb = block_of[t];
                if !succ[b].contains(&tb) {
                    succ[b].push(tb);
                    pred[tb].push(b);
                }
            }
        }
        Some(Cfg { start, block_of, succ, pred, len: n })
    }

    /// The instructions of block `b`.
    pub fn range(&self, b: usize) -> Range<usize> {
        self.start[b]..self.start.get(b + 1).copied().unwrap_or(self.len)
    }

    /// The immediate post-dominator of each block (Cooper, Harvey and Kennedy's iteration on the
    /// reversed graph, with one exit node after every block that ends the program). A successor
    /// from which no path ends the program counts as the exit: the branch to it does not pass
    /// through the other side's blocks.
    pub fn post_dominators(&self) -> Vec<PostDom> {
        let nb = self.start.len();
        let exit = nb;
        // the reversed graph's successors of a node: the blocks that can go to it
        let rsucc = |v: usize| -> Vec<usize> {
            if v == exit {
                (0..nb).filter(|&b| self.succ[b].is_empty()).collect()
            } else {
                self.pred[v].clone()
            }
        };
        // post-order of the reversed graph from the exit
        let mut order = vec![usize::MAX; nb + 1];
        let mut post = Vec::with_capacity(nb + 1);
        let mut seen = vec![false; nb + 1];
        let mut stack = vec![(exit, rsucc(exit), 0usize)];
        seen[exit] = true;
        while let Some((v, next, k)) = stack.last_mut() {
            if let Some(&w) = next.get(*k) {
                *k += 1;
                if !seen[w] {
                    seen[w] = true;
                    let ws = rsucc(w);
                    stack.push((w, ws, 0));
                }
            } else {
                order[*v] = post.len();
                post.push(*v);
                stack.pop();
            }
        }
        // the reversed graph's predecessors of a block: its successors, and the exit after a last block
        let rpred = |v: usize| -> Vec<usize> {
            if self.succ[v].is_empty() {
                vec![exit]
            } else {
                self.succ[v].clone()
            }
        };
        let mut idom = vec![usize::MAX; nb + 1];
        idom[exit] = exit;
        let intersect = |idom: &[usize], mut a: usize, mut b: usize| {
            while a != b {
                while order[a] < order[b] {
                    a = idom[a];
                }
                while order[b] < order[a] {
                    b = idom[b];
                }
            }
            a
        };
        let mut changed = true;
        while changed {
            changed = false;
            for &v in post.iter().rev().filter(|&&v| v != exit) {
                let mut new = usize::MAX;
                for p in rpred(v) {
                    let p = if seen[p] { p } else { exit };
                    if idom[p] == usize::MAX {
                        continue;
                    }
                    new = if new == usize::MAX { p } else { intersect(&idom, p, new) };
                }
                if new != usize::MAX && idom[v] != new {
                    idom[v] = new;
                    changed = true;
                }
            }
        }
        (0..nb)
            .map(|b| match idom[b] {
                usize::MAX => PostDom::None,
                d if d == exit => PostDom::Exit,
                d => PostDom::Block(d),
            })
            .collect()
    }

    /// For each block, the blocks with a branch it is control-dependent on: the branch decides
    /// whether the block runs (Ferrante, Ottenstein and Warren: from each successor of a branch up
    /// the post-dominator tree to the branch's own post-dominator). A block on a path that never
    /// ends the program counts as dependent on every branch that leads to it.
    pub fn control_dependence(&self, ipdom: &[PostDom]) -> Vec<Vec<usize>> {
        let mut cd: Vec<Vec<usize>> = vec![Vec::new(); self.start.len()];
        for (a, s) in self.succ.iter().enumerate() {
            if s.len() < 2 {
                continue;
            }
            let stop = ipdom[a];
            for &first in s {
                let mut run = first;
                loop {
                    if PostDom::Block(run) == stop {
                        break;
                    }
                    if !cd[run].contains(&a) {
                        cd[run].push(a);
                    } else {
                        break;
                    }
                    match ipdom[run] {
                        PostDom::Block(next) => run = next,
                        PostDom::Exit => break,
                        PostDom::None => {
                            // nothing after it ends the program either: every block from here on
                            // runs only on this side
                            let mut todo = self.succ[run].clone();
                            while let Some(b) = todo.pop() {
                                if !cd[b].contains(&a) {
                                    cd[b].push(a);
                                    todo.extend(self.succ[b].iter().copied());
                                }
                            }
                            break;
                        }
                    }
                }
            }
        }
        cd
    }
}
