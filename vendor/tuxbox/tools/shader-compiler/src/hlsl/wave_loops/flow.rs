//! CFG for the structured emitter. Each instruction has at most two successors.
use super::super::{branch_pc, ray_resolve, Emitter, Enc, Insn};
use std::collections::{BTreeSet, HashMap};

pub(super) struct Flow {
    pub(super) edges: Vec<[Option<usize>; 2]>,
    pub(super) incoming: Vec<Vec<usize>>,
    pub(super) reachable: Vec<bool>,
    pub(super) loops: Vec<(usize, usize)>,
}

impl Flow {
    /// The graph of a program whose loops the structured emitter can express.
    pub(super) fn new(code: &[Insn]) -> Option<Self> {
        let flow = Self::graph(code)?;
        flow.structured().then_some(flow)
    }

    /// Whether every loop has one backedge and no two interleave.
    pub(super) fn structured(&self) -> bool {
        let mut enclosing = Vec::new();
        let mut previous_head = None;
        for &(head, latch) in &self.loops {
            while enclosing.last().is_some_and(|&end| end < head) {
                enclosing.pop();
            }
            if previous_head == Some(head) || enclosing.last().is_some_and(|&end| end <= latch) {
                return false;
            }
            previous_head = Some(head);
            enclosing.push(latch);
        }
        true
    }

    pub(super) fn graph(code: &[Insn]) -> Option<Self> {
        let n = code.len();
        let end = code.last()?.offset + 4 * code.last()?.dwords;
        let offsets: HashMap<_, _> = code
            .iter()
            .enumerate()
            .map(|(i, op)| (op.offset, i))
            .collect();
        let mut flow = Self {
            edges: vec![[None; 2]; n],
            incoming: vec![Vec::new(); n + 1],
            reachable: vec![false; n + 1],
            loops: Vec::new(),
        };
        for (i, op) in code.iter().enumerate() {
            let branch = op.enc == Enc::Sopp && Emitter::branch_cond(op.op).is_some();
            let lane_spill = op.enc == Enc::Sop1
                && op.raw.len() == 2
                && matches!(op.raw[0] >> 16, 0xd760 | 0xd761)
                && matches!(op.mnemonic.as_str(), "s_mov_b32" | ray_resolve::READ_LANE);
            if (!op.is_complete() && !lane_spill)
                || op.enc == Enc::Unknown
                || (!branch && op.mnemonic.contains("branch"))
                || matches!(
                    op.mnemonic.as_str(),
                    "s_setpc_b64" | "s_swappc_b64" | "s_call_b64" | "s_rfe_b64" | "s_trap"
                )
            {
                return None;
            }
            if branch {
                let target = branch_pc(op);
                let target = usize::try_from(target).ok()?;
                let to = if target >= end {
                    n
                } else {
                    *offsets.get(&target)?
                };
                flow.edges[i] = [Some(to), (op.op != 2).then_some(i + 1)];
                if to <= i {
                    flow.loops.push((to, i));
                }
            } else if !op.mnemonic.starts_with("s_endpgm") {
                flow.edges[i][1] = Some(i + 1);
            }
            for to in flow.edges[i].into_iter().flatten() {
                flow.incoming[to].push(i);
            }
        }
        flow.loops.sort_unstable();
        let mut pending = vec![0];
        while let Some(i) = pending.pop() {
            if !std::mem::replace(&mut flow.reachable[i], true) && i < n {
                pending.extend(flow.edges[i].into_iter().flatten());
            }
        }
        Some(flow)
    }

    /// Visit every reaching definition, cutting the search at the specified
    /// loop entry or backedge to inspect one iteration or its initialization.
    pub(super) fn definitions(
        &self,
        at: usize,
        floor: usize,
        cut_edge: Option<(usize, usize)>,
        writes: impl Fn(usize) -> bool,
    ) -> Option<BTreeSet<usize>> {
        let (found, complete) = self.reaching_writes(at, Some(floor), cut_edge, writes);
        (complete && !found.is_empty()).then_some(found)
    }

    pub(super) fn reaching_writes(
        &self,
        at: usize,
        floor: Option<usize>,
        cut_edge: Option<(usize, usize)>,
        writes: impl Fn(usize) -> bool,
    ) -> (BTreeSet<usize>, bool) {
        let mut found = BTreeSet::new();
        let mut complete = true;
        let mut seen = vec![false; self.incoming.len()];
        let mut pending = vec![at];
        while let Some(to) = pending.pop() {
            if std::mem::replace(&mut seen[to], true) {
                continue;
            }
            if floor.is_some_and(|floor| to <= floor) {
                complete = false;
                continue;
            }
            let mut has_predecessor = false;
            for &from in &self.incoming[to] {
                if !self.reachable[from] || cut_edge == Some((from, to)) {
                    continue;
                }
                has_predecessor = true;
                if writes(from) {
                    found.insert(from);
                } else {
                    pending.push(from);
                }
            }
            complete &= has_predecessor && to != 0;
        }
        (found, complete)
    }

    pub(super) fn definition(
        &self,
        at: usize,
        head: usize,
        writes: impl Fn(usize) -> bool,
    ) -> Option<usize> {
        let defs = self.definitions(at, head, None, writes)?;
        (defs.len() == 1).then(|| *defs.first().unwrap())
    }
}
