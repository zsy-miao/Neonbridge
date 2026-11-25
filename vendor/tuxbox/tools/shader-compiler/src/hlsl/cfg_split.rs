//! Lay a program out so its loops nest: split irreducible regions, then sort.
//!
//! The structured emitter turns each back edge (L, B) into a do/while over the
//! instruction range L..=B, so two loops whose ranges overlap without nesting
//! ("interleaved") cannot be written. Those programs go to the tb_pc
//! dispatcher, except a non-pixel program with a DPP quad exchange, which is
//! refused (hlsl.rs, "DPP quad exchange with interleaved loops").
//!
//! The overlap comes from the guest layout or from a truly irreducible CFG.
//! This pass builds the basic blocks, makes the graph reducible by node
//! splitting (xcl crates/xcl/src/gpu/dxil/cfg.rs make_reducible, GPL-2.0,
//! copied with credit to its authors: the innermost multi-entry cycle region,
//! minus its first entry in RPO, is copied for the edges entering it
//! elsewhere, capped at four times the block count), then orders the blocks
//! so every loop body is contiguous and every other edge goes forward (the
//! loop-contiguous topological order LLVM's CFGSort uses). Fall-through edges
//! that no longer fall through get an `s_branch`.
//!
//! Original instructions keep their byte offsets, as exit_stubs does, so the
//! offset-keyed analyses still find them. Copies, inserted branches and a new
//! final `s_endpgm` get fresh offsets past the old end; `alias` maps a copy's
//! offset to its original. Every exit (a branch to or past the old end) is
//! retargeted to the new end. If the result still interleaves, the program is
//! left as it was.
//!
//! TB_CFG_SPLIT=0 off; unset, only programs with a quad exchange (the refused
//! case); TB_CFG_SPLIT=1 every interleaved program (instead of the dispatcher).

use super::{Emitter, Enc, Insn};
use std::collections::{BTreeSet, HashMap};

fn mode() -> u8 {
    match std::env::var("TB_CFG_SPLIT").as_deref().map(str::trim) {
        Ok("0") => 0,
        Ok("1") => 2,
        _ => 1,
    }
}

fn is_branch(ins: &Insn) -> bool {
    ins.enc == Enc::Sopp && Emitter::branch_cond(ins.op).is_some()
}

fn target(ins: &Insn) -> i64 {
    ins.offset as i64 + 4 + ins.aux.imm * 4
}

/// The emitter's own test (emit_body_structured): back edges by list index,
/// one loop per header closed by its last back edge, ranges neither disjoint
/// nor nested.
pub(super) fn interleaved(insns: &[Insn]) -> bool {
    let Some(last) = insns.last() else { return false };
    let end = last.offset + last.dwords * 4;
    let idx: HashMap<usize, usize> = insns.iter().enumerate().map(|(i, x)| (x.offset, i)).collect();
    let mut heads: HashMap<usize, usize> = HashMap::new();
    for (i, ins) in insns.iter().enumerate() {
        if !is_branch(ins) {
            continue;
        }
        let t = target(ins);
        if t < 0 || t as usize >= end {
            continue;
        }
        if let Some(&ti) = idx.get(&(t as usize)) {
            if ti <= i {
                let e = heads.entry(ti).or_insert(i);
                *e = (*e).max(i);
            }
        }
    }
    let loops: Vec<(usize, usize)> = heads.into_iter().collect();
    loops.iter().any(|&(l1, b1)| loops.iter().any(|&(l2, b2)| {
        !(b1 < l2 || b2 < l1) && !((l1 <= l2 && b2 <= b1) || (l2 <= l1 && b1 <= b2))
    }))
}

#[derive(Clone)]
enum Term {
    /// Falls through, or an `s_branch` (kept in `body`'s last slot when explicit).
    Jump(usize),
    /// Conditional branch (last of `body`): taken, not taken.
    Cond(usize, usize),
    End,
}

#[derive(Clone)]
struct Block {
    /// Instructions, the terminating branch included. Empty for the exit.
    body: Vec<Insn>,
    term: Term,
    copy: bool,
}

fn succ(b: &Block) -> Vec<usize> {
    match b.term {
        Term::Jump(t) => vec![t],
        Term::Cond(t, f) => vec![t, f],
        Term::End => vec![],
    }
}

/// Blocks of the program; the last block is the virtual exit.
fn blocks(insns: &[Insn], end: usize) -> Option<Vec<Block>> {
    let idx: HashMap<usize, usize> = insns.iter().enumerate().map(|(i, x)| (x.offset, i)).collect();
    let n = insns.len();
    let to_idx = |t: i64| -> Option<usize> {
        if t >= 0 && t as usize >= end { Some(n) } else { usize::try_from(t).ok().and_then(|t| idx.get(&t).copied()) }
    };
    let mut leaders = BTreeSet::from([0usize]);
    for (i, ins) in insns.iter().enumerate() {
        if ins.enc == Enc::Sopp && ins.mnemonic.starts_with("s_cbranch") && !is_branch(ins) {
            return None; // fork/join and the debugger branches
        }
        if is_branch(ins) {
            leaders.insert(to_idx(target(ins))?);
            leaders.insert(i + 1);
        } else if ins.enc == Enc::Sopp && ins.op == 1 {
            leaders.insert(i + 1);
        }
    }
    leaders.retain(|&l| l < n);
    let starts: Vec<usize> = leaders.iter().copied().collect();
    let block_of: HashMap<usize, usize> = starts.iter().enumerate().map(|(b, &s)| (s, b)).collect();
    let exit = starts.len();
    let bt = |i: usize| if i >= n { exit } else { block_of[&i] };
    let mut out = Vec::with_capacity(exit + 1);
    for (b, &s) in starts.iter().enumerate() {
        let e = starts.get(b + 1).copied().unwrap_or(n);
        let last = &insns[e - 1];
        let term = if is_branch(last) {
            let t = bt(to_idx(target(last))?);
            if last.op == 2 { Term::Jump(t) } else { Term::Cond(t, bt(e)) }
        } else if last.enc == Enc::Sopp && last.op == 1 {
            Term::End
        } else {
            Term::Jump(bt(e))
        };
        out.push(Block { body: insns[s..e].to_vec(), term, copy: false });
    }
    out.push(Block { body: Vec::new(), term: Term::End, copy: false });
    Some(out)
}

/// Reachable blocks in reverse post-order from block 0 (xcl `order`).
fn rpo(blocks: &[Block]) -> Vec<usize> {
    let n = blocks.len();
    let mut state = vec![0u8; n];
    let mut post = Vec::new();
    let mut stack = vec![(0usize, 0usize)];
    state[0] = 1;
    while let Some(&mut (b, ref mut k)) = stack.last_mut() {
        let s = succ(&blocks[b]);
        if *k < s.len() {
            let t = s[*k];
            *k += 1;
            if state[t] == 0 {
                state[t] = 1;
                stack.push((t, 0));
            }
        } else {
            state[b] = 2;
            post.push(b);
            stack.pop();
        }
    }
    post.reverse();
    post
}

/// Cooper-Harvey-Kennedy immediate dominators over `order` (xcl).
fn idoms(blocks: &[Block], order: &[usize]) -> (Vec<usize>, Vec<usize>, Vec<Vec<usize>>) {
    let n = blocks.len();
    let mut pos = vec![usize::MAX; n];
    for (i, &b) in order.iter().enumerate() {
        pos[b] = i;
    }
    let mut preds = vec![Vec::new(); n];
    for &b in order {
        for t in succ(&blocks[b]) {
            preds[t].push(b);
        }
    }
    let mut idom = vec![usize::MAX; n];
    idom[order[0]] = order[0];
    let mut changed = true;
    while changed {
        changed = false;
        for &b in &order[1..] {
            let mut nd = usize::MAX;
            for &p in preds[b].iter().filter(|&&p| idom[p] != usize::MAX) {
                nd = if nd == usize::MAX {
                    p
                } else {
                    let (mut x, mut y) = (p, nd);
                    while x != y {
                        while pos[x] > pos[y] { x = idom[x]; }
                        while pos[y] > pos[x] { y = idom[y]; }
                    }
                    x
                };
            }
            if nd != usize::MAX && idom[b] != nd {
                idom[b] = nd;
                changed = true;
            }
        }
    }
    (idom, pos, preds)
}

fn dominates(idom: &[usize], a: usize, mut b: usize) -> bool {
    loop {
        if a == b { return true; }
        if idom[b] == b || idom[b] == usize::MAX { return false; }
        b = idom[b];
    }
}

/// xcl make_reducible: split until no retreating edge targets a non-dominator.
fn make_reducible(blocks: &mut Vec<Block>) -> Option<()> {
    let limit = 4 * blocks.len() + 256;
    loop {
        let order = rpo(blocks);
        let (idom, pos, preds) = idoms(blocks, &order);
        let bad = order.iter().find_map(|&a| succ(&blocks[a]).into_iter()
            .find(|&t| pos[t] <= pos[a] && !dominates(&idom, t, a)));
        let Some(t) = bad else { return Some(()) };
        let n = blocks.len();
        let reach = |from: usize, inside: &[bool], fwd: bool| {
            let mut seen = vec![false; n];
            let mut stack = vec![from];
            seen[from] = true;
            while let Some(x) = stack.pop() {
                let next = if fwd { succ(&blocks[x]) } else { preds[x].clone() };
                for y in next {
                    if inside[y] && !seen[y] {
                        seen[y] = true;
                        stack.push(y);
                    }
                }
            }
            seen
        };
        let mut inside: Vec<bool> = (0..n).map(|b| pos[b] != usize::MAX).collect();
        let (region, entries) = loop {
            let (f, r) = (reach(t, &inside, true), reach(t, &inside, false));
            let region: Vec<bool> = (0..n).map(|b| f[b] && r[b]).collect();
            let entries: Vec<usize> = (0..n)
                .filter(|&b| region[b] && (b == order[0] || preds[b].iter().any(|&p| !region[p])))
                .collect();
            if entries.len() != 1 {
                break (region, entries);
            }
            inside = region;
            inside[entries[0]] = false;
            if !inside[t] {
                return None;
            }
        };
        if blocks.len() >= limit || entries.is_empty() {
            return None;
        }
        let h = *entries.iter().min_by_key(|&&e| pos[e])?;
        let mut copy = vec![usize::MAX; n];
        for b in (0..n).filter(|&b| region[b] && b != h) {
            copy[b] = blocks.len();
            let mut c = blocks[b].clone();
            c.copy = true;
            blocks.push(c);
        }
        let redirect = |term: &mut Term, to: &dyn Fn(usize) -> usize| match term {
            Term::Jump(x) => *x = to(*x),
            Term::Cond(x, y) => {
                *x = to(*x);
                *y = to(*y);
            }
            Term::End => {}
        };
        for b in (0..n).filter(|&b| copy[b] != usize::MAX) {
            redirect(&mut blocks[copy[b]].term, &|x| if copy[x] != usize::MAX { copy[x] } else { x });
        }
        for p in (0..n).filter(|&p| !region[p] && pos[p] != usize::MAX) {
            redirect(&mut blocks[p].term, &|x| if region[x] && x != h { copy[x] } else { x });
        }
    }
}

/// Loop-contiguous topological order of a reducible graph (exit excluded).
fn sort(blocks: &[Block], exit: usize) -> Option<Vec<usize>> {
    let order = rpo(blocks);
    let (idom, pos, preds) = idoms(blocks, &order);
    let n = blocks.len();
    // Natural loops: header -> members.
    let mut loops: HashMap<usize, Vec<bool>> = HashMap::new();
    for &a in &order {
        for h in succ(&blocks[a]) {
            if pos[h] <= pos[a] && dominates(&idom, h, a) {
                let body = loops.entry(h).or_insert_with(|| { let mut v = vec![false; n]; v[h] = true; v });
                let mut stack = vec![a];
                while let Some(x) = stack.pop() {
                    if !body[x] {
                        body[x] = true;
                        stack.extend(preds[x].iter().copied());
                    }
                }
            }
        }
    }
    let back = |a: usize, h: usize| loops.contains_key(&h) && loops[&h][a] && pos[h] <= pos[a];
    let mut remaining = vec![0usize; n];
    for &a in &order {
        for t in succ(&blocks[a]) {
            if !back(a, t) {
                remaining[t] += 1;
            }
        }
    }
    let mut ready: BTreeSet<(usize, usize)> = BTreeSet::from([(pos[order[0]], order[0])]);
    let mut active: Vec<usize> = Vec::new();
    let mut left: HashMap<usize, usize> = loops.iter().map(|(&h, b)| (h, b.iter().filter(|&&x| x).count())).collect();
    let mut placed = vec![false; n];
    let mut out = Vec::new();
    while !ready.is_empty() {
        let pick = ready.iter().copied().find(|&(_, b)| active.last().is_none_or(|h| loops[h][b]))?;
        ready.remove(&pick);
        let b = pick.1;
        placed[b] = true;
        if b != exit {
            out.push(b);
        }
        if loops.contains_key(&b) {
            active.push(b);
        }
        for (&h, body) in &loops {
            if body[b] {
                *left.get_mut(&h)? -= 1;
            }
        }
        while active.last().is_some_and(|h| left[h] == 0) {
            active.pop();
        }
        for t in succ(&blocks[b]) {
            if !back(b, t) {
                remaining[t] -= 1;
                if remaining[t] == 0 && !placed[t] {
                    ready.insert((pos[t], t));
                }
            }
        }
    }
    // Every reachable block placed, or the order is not loop-contiguous.
    let reachable = order.iter().filter(|&&b| b != exit).count();
    (out.len() == reachable).then_some(out)
}

fn sopp(template: &Insn, op: u32, mnemonic: &str, offset: usize) -> Insn {
    let mut i = template.clone();
    i.offset = offset;
    i.dwords = 1;
    i.op = op;
    i.raw = vec![0xbf80_0000 | (op << 16)];
    i.mnemonic = mnemonic.to_string();
    i.dst.clear();
    i.src.clear();
    i.aux = Default::default();
    i
}

/// Re-lay `insns` when they interleave. Returns the copies' offset aliases.
pub(super) fn apply(insns: &mut Vec<Insn>) -> HashMap<usize, usize> {
    let m = mode();
    if m == 0 || (m == 1 && !insns.iter().any(super::is_quad_exchange)) || !interleaved(insns) {
        return HashMap::new();
    }
    if insns.iter().any(|i| matches!(i.mnemonic.as_str(), "s_getpc_b64" | "s_setpc_b64" | "s_swappc_b64" | "s_call_b64")) {
        return HashMap::new();
    }
    let Some(last) = insns.last() else { return HashMap::new() };
    let end = last.offset + last.dwords * 4;
    let Some(template) = insns.iter().find(|i| i.enc == Enc::Sopp).cloned() else { return HashMap::new() };
    let Some(mut bl) = blocks(insns, end) else { return HashMap::new() };
    let exit = bl.len() - 1;
    if make_reducible(&mut bl).is_none() {
        return HashMap::new();
    }
    let Some(order) = sort(&bl, exit) else { return HashMap::new() };
    // Lay out: fresh offsets for copies and inserted branches.
    let mut fresh = end.next_multiple_of(4);
    let mut alias = HashMap::new();
    let mut start: HashMap<usize, usize> = HashMap::new();
    let mut laid: Vec<(Insn, Option<usize>)> = Vec::new(); // branch target block
    for (k, &b) in order.iter().enumerate() {
        let block = &bl[b];
        let next = order.get(k + 1).copied();
        let mut body = block.body.clone();
        if block.copy {
            for i in &mut body {
                alias.insert(fresh, i.offset);
                i.offset = fresh;
                fresh += i.dwords * 4;
            }
        }
        start.insert(b, body[0].offset);
        let n = body.len();
        for (j, ins) in body.into_iter().enumerate() {
            let t = match (&block.term, j + 1 == n && is_branch(&ins)) {
                (Term::Jump(t), true) | (Term::Cond(t, _), true) => Some(*t),
                _ => None,
            };
            laid.push((ins, t));
        }
        let fall = match block.term {
            Term::Cond(_, f) => Some(f),
            Term::Jump(t) if !block.body.last().is_some_and(is_branch) => Some(t),
            _ => None,
        };
        if let Some(f) = fall.filter(|&f| Some(f) != next) {
            laid.push((sopp(&template, 2, "s_branch", fresh), Some(f)));
            fresh += 4;
        }
    }
    let new_end = fresh + 4;
    laid.push((sopp(&template, 1, "s_endpgm", fresh), None));
    let mut out = Vec::with_capacity(laid.len());
    for (mut ins, t) in laid {
        if let Some(t) = t {
            let to = if t == exit { new_end } else { start[&t] };
            ins.aux.imm = (to as i64 - ins.offset as i64 - 4) / 4;
        }
        out.push(ins);
    }
    if interleaved(&out) {
        return HashMap::new();
    }
    *insns = out;
    alias
}
