//! Convergence proof for a split-wave LDS branch candidate. This runs on
//! the emitted control flow, including the loop caps and predicated loads.

use super::{Bin, Const, Module, Op, Term, TypeDef, V, pred};
use std::cell::Cell;
use std::collections::HashSet;

struct ControlFlow {
    succ: Vec<Vec<usize>>,
    pred: Vec<Vec<usize>>,
    dom: Dominators,
    postdom: Dominators,
}

impl ControlFlow {
    fn new(m: &Module) -> Option<Self> {
        let mut succ = Vec::new();
        let mut pred = vec![Vec::new(); m.blocks.len()];
        let mut exit = None;
        for (b, block) in m.blocks.iter().enumerate() {
            let next = match block.term? {
                Term::Br(t) => vec![t],
                Term::CondBr(_, t, f) if t == f => vec![t],
                Term::CondBr(_, t, f) => vec![t, f],
                Term::Ret if exit.is_none() => {
                    exit = Some(b);
                    Vec::new()
                }
                Term::Ret => return None,
            };
            for &t in &next {
                pred[t].push(b);
            }
            succ.push(next);
        }
        let exit = exit?;
        if !reaches_every_block(&succ, 0) || !reaches_every_block(&pred, exit) {
            return None;
        }
        let dom = Dominators::new(&pred, 0);
        let postdom = Dominators::new(&succ, exit);
        Some(Self {
            succ,
            pred,
            dom,
            postdom,
        })
    }

    fn region(&self, b: usize) -> Vec<usize> {
        let mut region = Vec::new();
        let mut seen = vec![false; self.succ.len()];
        let mut pending = self.succ[b].clone();
        while let Some(t) = pending.pop() {
            // A loop may revisit its own branch before reconvergence.
            if seen[t] || (t != b && self.postdom.has(b, t)) {
                continue;
            }
            seen[t] = true;
            region.push(t);
            pending.extend(self.succ[t].iter().copied());
        }
        region
    }

    fn capped_graph_is_acyclic(&self, caps: &[Option<usize>]) -> bool {
        let mut incoming: Vec<_> = self.pred.iter().map(Vec::len).collect();
        for &header in caps.iter().flatten() {
            incoming[header] -= 1;
        }
        let mut pending: Vec<_> = (0..incoming.len()).filter(|&b| incoming[b] == 0).collect();
        let mut order = vec![0; incoming.len()];
        let mut visited = 0;
        while let Some(b) = pending.pop() {
            order[b] = visited;
            visited += 1;
            for &t in &self.succ[b] {
                if caps[b] == Some(t) {
                    continue;
                }
                incoming[t] -= 1;
                if incoming[t] == 0 {
                    pending.push(t);
                }
            }
        }
        visited == incoming.len()
            && caps
                .iter()
                .enumerate()
                .all(|(b, h)| h.is_none_or(|h| order[h] <= order[b]))
    }
}

fn reaches_every_block(edges: &[Vec<usize>], root: usize) -> bool {
    let mut seen = vec![false; edges.len()];
    let mut pending = vec![root];
    while let Some(b) = pending.pop() {
        if !seen[b] {
            seen[b] = true;
            pending.extend(edges[b].iter().copied());
        }
    }
    seen.into_iter().all(|s| s)
}

struct Dominators(Vec<Vec<u64>>);

impl Dominators {
    fn new(pred: &[Vec<usize>], root: usize) -> Self {
        let n = pred.len();
        let full = vec![u64::MAX; n.div_ceil(u64::BITS as usize)];
        let mut dom = vec![full.clone(); n];
        dom[root].fill(0);
        dom[root][root / 64] = 1 << (root % 64);
        let mut changed = true;
        let mut row = full.clone();
        while changed {
            changed = false;
            for (b, incoming) in pred.iter().enumerate().filter(|(b, _)| *b != root) {
                row.copy_from_slice(&full);
                for &p in incoming {
                    for (bits, pred) in row.iter_mut().zip(&dom[p]) {
                        *bits &= pred;
                    }
                }
                row[b / 64] |= 1 << (b % 64);
                if dom[b] != row {
                    std::mem::swap(&mut dom[b], &mut row);
                    changed = true;
                }
            }
        }
        Self(dom)
    }

    fn has(&self, block: usize, dominator: usize) -> bool {
        self.0[block][dominator / 64] & (1 << (dominator % 64)) != 0
    }
}

struct Proof<'a> {
    m: &'a Module,
    cfg: ControlFlow,
    /// Definition's block before constants and values are numbered for DXIL.
    block_of: Vec<usize>,
    /// Per latch, the loop header reached by a bounded backedge.
    caps: Vec<Option<usize>>,
    /// The shader-specific snapshot the convergence proof must use.
    snapshot_slot: Option<u32>,
    snapshot_seen: Cell<bool>,
}

impl Module {
    pub(crate) fn certifies_split_lds(&self, snapshot_slot: Option<u32>) -> bool {
        let Some(cfg) = ControlFlow::new(self) else {
            return false;
        };
        let mut block_of = vec![usize::MAX; self.insts.len()];
        for (b, block) in self.blocks.iter().enumerate() {
            for &i in block.phis.iter().chain(&block.body) {
                block_of[i as usize] = b;
            }
        }
        let mut proof = Proof {
            m: self,
            cfg,
            block_of,
            caps: vec![None; self.blocks.len()],
            snapshot_slot,
            snapshot_seen: Cell::new(false),
        };
        proof.bound_loops()
            && proof.align_meetings()
            && (snapshot_slot.is_none() || proof.snapshot_seen.get())
    }
}

impl Proof<'_> {
    fn def(&self, v: V) -> Option<(usize, &Op)> {
        let V::Inst(i) = v else { return None };
        let b = self.block_of[i as usize];
        (b != usize::MAX).then_some((b, &self.m.insts[i as usize].op))
    }

    // A natural loop header entered once at zero and incremented on its
    // sole backedge. Returns (header, latch, step).
    fn counter(&self, v: V) -> Option<(usize, usize, V)> {
        let (header, Op::Phi(incoming)) = self.def(v)? else {
            return None;
        };
        let [a, b] = incoming.as_slice() else {
            return None;
        };
        let ((init, entry), (step, latch)) = if self.cfg.dom.has(b.1, header) {
            (*a, *b)
        } else {
            (*b, *a)
        };
        if self.m.int_value(init) != Some(0)
            || self.cfg.dom.has(entry, header)
            || !self.cfg.dom.has(latch, header)
            || self.cfg.pred[header].len() != 2
            || !self.cfg.pred[header].contains(&entry)
            || !self.cfg.pred[header].contains(&latch)
            || self.m.types[self.m.type_of(v).0 as usize] != TypeDef::Int(32)
        {
            return None;
        }
        let (block, Op::Bin(Bin::Add, a, b)) = self.def(step)? else {
            return None;
        };
        let adds_one = (*a == v && self.m.int_value(*b) == Some(1))
            || (*b == v && self.m.int_value(*a) == Some(1));
        (block == latch && adds_one).then_some((header, latch, step))
    }

    // Removing the capped edges must make the graph acyclic. An infinite
    // path would take some cap infinitely often. The earliest such header
    // in the resulting topological order would then never reset its count.
    fn bound_loops(&mut self) -> bool {
        for (b, edges) in self.cfg.succ.iter().enumerate() {
            let mut back = edges.iter().copied().filter(|&t| self.cfg.dom.has(b, t));
            let Some(header) = back.next() else { continue };
            if back.next().is_some() || !self.caps_exit(b, header) {
                return false;
            }
            self.caps[b] = Some(header);
        }
        self.cfg.capped_graph_is_acyclic(&self.caps)
    }

    fn caps_exit(&self, latch: usize, header: usize) -> bool {
        let Some(Term::CondBr(cond, yes, no)) = self.m.blocks[latch].term else {
            return false;
        };
        let Some((_, Op::Bin(bin, a, b))) = self.def(cond) else {
            return false;
        };
        let cmp = match bin {
            Bin::Or if no == header && yes != header => pred::IUGE,
            Bin::And if yes == header && no != header => pred::IULT,
            _ => return false,
        };
        [*a, *b].into_iter().any(|part| {
            let Some((_, Op::Cmp(op, step, limit))) = self.def(part) else {
                return false;
            };
            let Some((_, Op::Bin(Bin::Add, x, y))) = self.def(*step) else {
                return false;
            };
            *op == cmp
                && self.m.int_value(*limit).is_some_and(|n| n as u32 != 0)
                && [*x, *y]
                    .into_iter()
                    .any(|phi| self.counter(phi) == Some((header, latch, *step)))
        })
    }

    fn wait_mode(&self, op: &Op) -> Option<i64> {
        match op {
            Op::Call(f, args) if self.m.funcs[*f as usize].name == "dx.op.barrier" => {
                // An unavailable mode still counts as a possible meeting.
                Some(args.get(1).and_then(|&v| self.m.int_value(v)).unwrap_or(1))
            }
            _ => None,
        }
    }

    fn align_meetings(&self) -> bool {
        let mut checkpoints: Vec<_> = self
            .m
            .blocks
            .iter()
            .map(|b| {
                b.body.iter().any(|&i| {
                    self.wait_mode(&self.m.insts[i as usize].op)
                        .is_some_and(|mode| mode & 1 != 0)
                })
            })
            .collect();
        if !checkpoints.iter().any(|&wait| wait) {
            return false;
        }
        let branches: Vec<_> = self
            .m
            .blocks
            .iter()
            .enumerate()
            .filter_map(|(b, block)| {
                if let Some(Term::CondBr(v, t, f)) = block.term
                    && t != f
                {
                    Some((b, v, self.cfg.region(b)))
                } else {
                    None
                }
            })
            .collect();

        // Every path through a varying split must reconverge before it
        // reaches a meeting. If a meeting depends on a loop counter,
        // also require both the entry and the latch to be reached together.
        let mut checked = vec![false; checkpoints.len()];
        let mut changed = true;
        while changed {
            changed = false;
            for (b, v, region) in &branches {
                if checked[*b] || !region.iter().any(|&q| checkpoints[q]) {
                    continue;
                }
                if !self.uniform(*v, *b, &mut checkpoints, &mut Default::default()) {
                    return false;
                }
                checked[*b] = true;
                checkpoints[*b] = true;
                changed = true;
            }
        }
        true
    }

    fn constant(&self, c: u32) -> bool {
        match &self.m.consts[c as usize].1 {
            Const::Int(_) | Const::Float(_) => true,
            Const::Aggregate(parts) => parts.iter().all(|&part| self.constant(part)),
            Const::Undef => false,
        }
    }

    fn uniform(&self, v: V, b: usize, checkpoints: &mut [bool], proven: &mut HashSet<V>) -> bool {
        if proven.contains(&v) {
            return true;
        }
        let same = &mut |v| self.uniform(v, b, checkpoints, proven);
        let valid = match self.def(v) {
            Some((_, Op::Phi(_))) => self.counter(v).is_some_and(|(header, latch, _)| {
                if self.caps[latch] != Some(header) {
                    return false;
                }
                checkpoints[header] = true;
                checkpoints[latch] = true;
                true
            }),
            Some((
                _,
                Op::Bin(Bin::Add | Bin::Sub | Bin::Mul | Bin::And | Bin::Or | Bin::Xor, a, c),
            ))
            | Some((_, Op::Cmp(32..=41, a, c))) => same(*a) && same(*c),
            Some((_, Op::Bin(Bin::Shl | Bin::LShr | Bin::AShr, a, c))) => {
                self.m.types[self.m.type_of(*a).0 as usize] == TypeDef::Int(32)
                    && self
                        .m
                        .int_value(*c)
                        .is_some_and(|shift| (0..32).contains(&shift))
                    && same(*a)
            }
            Some((_, Op::Extract(a, _))) => same(*a),
            Some((_, Op::Call(f, args))) => {
                let accepted = match self.m.funcs[*f as usize].name.as_str() {
                    "dx.op.groupId.i32"
                    | "dx.op.annotateHandle"
                    | "dx.op.createHandleFromBinding"
                    | "dx.op.createHandleFromHeap" => true,
                    // The view heap caps indices at its null slot with UMin.
                    // Both operands still have to be group-uniform below.
                    "dx.op.binary.i32" => matches!(args.as_slice(), [op, _, limit]
                        if self.m.int_value(*op) == Some(40) && self.m.int_value(*limit).is_some()),
                    "dx.op.cbufferLoadLegacy.i32" => {
                        args.get(1).is_some_and(|&handle| self.cbuffer(handle))
                    }
                    _ => false,
                };
                accepted && args.iter().copied().all(same)
            }
            Some((at, Op::Load(ptr))) if at == b => {
                if self.snapshot(v, *ptr, b) {
                    self.snapshot_seen.set(true);
                    true
                } else {
                    false
                }
            }
            _ => matches!(v, V::Const(c) if self.constant(c)),
        };
        if valid {
            proven.insert(v);
        }
        valid
    }

    fn cbuffer(&self, handle: V) -> bool {
        let Some((_, Op::Call(f, args))) = self.def(handle) else {
            return false;
        };
        let [_, _, V::Const(props)] = args.as_slice() else {
            return false;
        };
        let Const::Aggregate(fields) = &self.m.consts[*props as usize].1 else {
            return false;
        };
        self.m.funcs[*f as usize].name == "dx.op.annotateHandle"
            && fields
                .first()
                .is_some_and(|&kind| self.m.int_value(V::Const(kind)) == Some(13))
    }

    fn snapshot(&self, v: V, ptr: V, b: usize) -> bool {
        let V::Inst(load) = v else { return false };
        let Some((_, Op::Gep(array, V::Global(g), indices))) = self.def(ptr) else {
            return false;
        };
        let TypeDef::Array(len, elem) = self.m.types[array.0 as usize] else {
            return false;
        };
        let [leading, slot] = indices.as_slice() else {
            return false;
        };
        let Some(slot) = self.m.int_value(*slot) else {
            return false;
        };
        if self.m.globals[*g as usize].space != 3
            || self.m.types[elem.0 as usize] != TypeDef::Int(32)
            || self.m.int_value(*leading) != Some(0)
            || !(0..i64::from(len)).contains(&slot)
            || self
                .snapshot_slot
                .is_some_and(|required| slot != i64::from(required))
        {
            return false;
        }
        let body = &self.m.blocks[b].body;
        let Some(at) = body.iter().position(|&i| i == load) else {
            return false;
        };
        let fence = |&i: &u32| {
            self.wait_mode(&self.m.insts[i as usize].op)
                .is_some_and(|mode| mode & 9 == 9)
        };
        let Some(before) = body[..at].iter().rposition(fence) else {
            return false;
        };
        let Some(after) = body[at + 1..].iter().position(fence) else {
            return false;
        };
        // No other lane can update the dword between the two barriers.
        body[before + 1..at + 1 + after].iter().all(|&i| {
            i == load
                || matches!(
                    self.m.insts[i as usize].op,
                    Op::Gep(..) | Op::Cmp(..) | Op::Bin(..) | Op::Cast(..)
                )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Attr, Bin, Module, Term, V, pred};

    fn meeting(m: &mut Module, block: usize) {
        barrier(m, block, 9);
    }

    fn barrier(m: &mut Module, block: usize, mode: u32) {
        let void = m.void();
        let i32t = m.int(32);
        let ty = m.func_ty(void, vec![i32t, i32t]);
        let barrier = m.function("dx.op.barrier", ty, Some(Attr::NoDuplicate), false);
        let op = m.i32(80);
        let mode = m.i32(mode);
        m.call(block, barrier, vec![op, mode]);
    }

    #[test]
    fn split_lds_rejects_a_fence_without_group_sync() {
        let mut m = Module::default();
        let entry = m.new_block();
        barrier(&mut m, entry, 8);
        m.terminate(entry, Term::Ret);
        assert!(!m.certifies_split_lds(None));
    }

    fn lane_index(m: &mut Module, block: usize) -> V {
        let ty = m.int(32);
        let function = m.func_ty(ty, vec![ty, ty]);
        let index = m.function(
            "dx.op.threadIdInGroup.i32",
            function,
            Some(Attr::ReadNone),
            false,
        );
        let op = m.i32(95);
        let zero = m.i32(0);
        m.call(block, index, vec![op, zero])
    }

    #[test]
    fn split_lds_snapshot_requires_a_constant_slot_and_two_meetings() {
        for (constant_address, after, accepted) in [
            (true, true, true),
            (true, false, false),
            (false, true, false),
        ] {
            let mut m = Module::default();
            let entry = m.new_block();
            let work = m.new_block();
            let done = m.new_block();
            let i32t = m.int(32);
            let arr = m.array(64, i32t);
            let lds = m.global("lds", arr, 3);
            let zero = m.i32(0);
            meeting(&mut m, entry);
            let index = if constant_address {
                zero
            } else {
                lane_index(&mut m, entry)
            };
            let ptr = m.element(entry, lds, index);
            let data = m.load(entry, ptr);
            let branch = m.cmp(entry, pred::EQ, data, zero);
            if after {
                meeting(&mut m, entry);
            }
            m.terminate(entry, Term::CondBr(branch, work, done));
            meeting(&mut m, work);
            m.terminate(work, Term::Br(done));
            m.terminate(done, Term::Ret);
            assert_eq!(m.certifies_split_lds(Some(0)), accepted);
            assert!(!m.certifies_split_lds(Some(1)));
        }
    }

    #[test]
    fn split_lds_accepts_only_a_uniform_heap_clamp() {
        for (opcode, uniform, accepted) in [(40, true, true), (40, false, false), (39, true, false)]
        {
            let mut m = Module::default();
            let entry = m.new_block();
            let work = m.new_block();
            let done = m.new_block();
            let i32t = m.int(32);
            let binary_ty = m.func_ty(i32t, vec![i32t, i32t, i32t]);
            let binary = m.function("dx.op.binary.i32", binary_ty, Some(Attr::ReadNone), false);
            let zero = m.i32(0);
            let limit = m.i32(999_999);
            let operand = if uniform {
                zero
            } else {
                lane_index(&mut m, entry)
            };
            let opcode = m.i32(opcode);
            let clamped = m.call(entry, binary, vec![opcode, operand, limit]);
            let branch = m.cmp(entry, pred::EQ, clamped, zero);
            m.terminate(entry, Term::CondBr(branch, work, done));
            meeting(&mut m, work);
            m.terminate(work, Term::Br(done));
            m.terminate(done, Term::Ret);
            assert_eq!(m.certifies_split_lds(None), accepted);
        }
    }

    #[test]
    fn split_lds_refuses_an_early_return_before_a_meeting() {
        let mut m = Module::default();
        let entry = m.new_block();
        let early = m.new_block();
        let done = m.new_block();
        let lane = lane_index(&mut m, entry);
        let zero = m.i32(0);
        let branch = m.cmp(entry, pred::EQ, lane, zero);
        m.terminate(entry, Term::CondBr(branch, early, done));
        m.terminate(early, Term::Ret);
        meeting(&mut m, done);
        m.terminate(done, Term::Ret);
        assert!(!m.certifies_split_lds(None));
    }

    #[test]
    fn split_lds_meeting_can_postdominate_a_capped_partial_loop() {
        for (capped, in_loop, accepted) in [
            (true, false, true),
            (false, false, false),
            (true, true, false),
        ] {
            let mut m = Module::default();
            let entry = m.new_block();
            let begin = m.new_block();
            let header = m.new_block();
            let latch = m.new_block();
            let join = m.new_block();
            let i32t = m.int(32);
            let zero = m.i32(0);
            let one = m.i32(1);
            let limit = m.i32(32);
            let lane = lane_index(&mut m, entry);
            let skip = m.cmp(entry, pred::EQ, lane, zero);
            m.terminate(entry, Term::CondBr(skip, join, begin));
            m.terminate(begin, Term::Br(header));
            let count = m.phi(header, i32t);
            if in_loop {
                meeting(&mut m, header);
            }
            m.terminate(header, Term::Br(latch));
            let next = m.bin(latch, Bin::Add, count, one);
            m.set_phi(count, vec![(zero, begin), (next, latch)]);
            let stop = m.cmp(latch, pred::EQ, next, lane);
            let over = m.cmp(latch, pred::IUGE, next, limit);
            let until = if capped {
                m.bin(latch, Bin::Or, stop, over)
            } else {
                stop
            };
            m.terminate(latch, Term::CondBr(until, join, header));
            meeting(&mut m, join);
            m.terminate(join, Term::Ret);
            assert_eq!(m.certifies_split_lds(None), accepted);
            assert!(!m.certifies_split_lds(Some(0)));
        }
    }

    #[test]
    fn split_lds_checks_native_counter_seeds_when_loop_contains_meetings() {
        for uniform_seed in [true, false] {
            let mut m = Module::default();
            let entry = m.new_block();
            let header = m.new_block();
            let latch = m.new_block();
            let done = m.new_block();
            let i32t = m.int(32);
            let zero = m.i32(0);
            let one = m.i32(1);
            let limit = m.i32(32);
            let seed = if uniform_seed {
                zero
            } else {
                lane_index(&mut m, entry)
            };
            m.terminate(entry, Term::Br(header));
            let cap = m.phi(header, i32t);
            let native = m.phi(header, i32t);
            meeting(&mut m, header);
            m.terminate(header, Term::Br(latch));
            let next = m.bin(latch, Bin::Add, cap, one);
            let advanced = m.bin(latch, Bin::Add, native, one);
            m.set_phi(cap, vec![(zero, entry), (next, latch)]);
            m.set_phi(native, vec![(seed, entry), (advanced, latch)]);
            let stop = m.cmp(latch, pred::EQ, advanced, limit);
            let over = m.cmp(latch, pred::IUGE, next, limit);
            let until = m.bin(latch, Bin::Or, stop, over);
            m.terminate(latch, Term::CondBr(until, done, header));
            m.terminate(done, Term::Ret);
            assert_eq!(m.certifies_split_lds(None), uniform_seed);
        }
    }
}
