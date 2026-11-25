//! Control-flow structurisation (decision 0003): the program as one lane
//! runs it, without EXEC.
//!
//! Lane masks (EXEC, compare and carry results, the SGPRs they are saved
//! in) become per-lane booleans ([`Bools`]). A branch on a wave-wide test of
//! a mask (`s_cbranch_execz`, `vccz`, SCC of a mask operation) becomes a
//! branch on the lane's own bit: compiled code only skips work that lanes
//! outside the mask would not change, or tests a value that is the same in
//! every lane, so either way each lane can decide alone. A path-sensitive
//! pass then proves, for each vector instruction, that EXEC holds in every
//! lane that reaches it; what it cannot prove stays as the instruction's
//! guard, a per-lane `if` around it.
//!
//! The control-flow graph keeps its shape and must be reducible (DXIL's
//! only requirement). Helper lanes of a pixel shader count as live: on PC
//! their side effects are dropped too, and what they compute is only read
//! through derivatives.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};

use crate::ShaderError;
use crate::gfx10::{Fields, NULL, Operand};
use crate::ir::{EXEC, Exit, Inst, Program, Reg, VCC, Value};

/// A per-lane boolean: a node of [`Bools`]. Equal functions are equal nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Bool(u32);

impl Bool {
    pub const FALSE: Bool = Bool(0);
    pub const TRUE: Bool = Bool(1);
}

/// What a variable of [`Bools`] stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Atom {
    /// The lane's bit of the compare or carry result in this VALU-written
    /// value, before EXEC masking.
    Lane(Value),
    /// An SCC value, the same in every lane.
    Uniform(Value),
    /// A lane mask joined at a block entry, by its low dword's phi.
    Phi(Value),
    /// `s_wqm` result: some lane of the quad is in the source mask.
    Quad(Value),
    /// A mask the pass cannot see into (loaded, built from scalars, an
    /// EXEC write it does not follow): the lane's bit of the register pair
    /// this value is a dword of.
    Opaque(Value),
    /// A constant mask other than all lanes or none.
    Constant(u64),
}

/// Reduced ordered binary decision diagrams over [`Atom`]s.
#[derive(Debug, Clone)]
pub struct Bools {
    /// (variable, low, high); nodes 0 and 1 are the constants.
    nodes: Vec<(u32, Bool, Bool)>,
    unique: Map<(u32, Bool, Bool), Bool>,
    memo: Map<(Bool, Bool, Bool), Bool>,
    atoms: Vec<Atom>,
    /// Reverse-postorder index of the block that defines each atom.
    born: Vec<usize>,
    ids: Map<Atom, u32>,
    overflow: bool,
}

/// The tables below have small integer keys; SipHash dominated run time.
type Map<K, V> = HashMap<K, V, BuildHasherDefault<Fx>>;

/// FxHash's multiply-rotate step.
#[derive(Default)]
struct Fx(u64);

impl Hasher for Fx {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u64(u64::from(b));
        }
    }

    fn write_u32(&mut self, n: u32) {
        self.write_u64(u64::from(n));
    }

    fn write_u64(&mut self, n: u64) {
        self.0 = (self.0.rotate_left(5) ^ n).wrapping_mul(0x517c_c1b7_2722_0a95);
    }

    fn write_usize(&mut self, n: usize) {
        self.write_u64(n as u64);
    }
}

const TERMINAL: u32 = u32::MAX;
const MAX_NODES: usize = 1 << 21;
/// Case splits [`Bools::meet`] makes before it gives up.
const MAX_MEET: usize = 1 << 16;
/// Conditions [`Meets`] keeps; past that, they are searched anew.
const MAX_MEETS: usize = 1 << 18;

/// What [`Bools::meet`] found out about one [`Bools`], for its later calls:
/// the conditions no lane satisfies, and the case splits it took to find
/// that out of each.
#[derive(Debug, Default)]
pub struct Meets {
    none: Map<Box<[u32]>, usize>,
}

impl Bools {
    fn new() -> Self {
        Bools {
            nodes: vec![
                (TERMINAL, Bool::FALSE, Bool::FALSE),
                (TERMINAL, Bool::TRUE, Bool::TRUE),
            ],
            unique: Map::default(),
            memo: Map::default(),
            atoms: Vec::new(),
            born: Vec::new(),
            ids: Map::default(),
            overflow: false,
        }
    }

    /// The atom tested at the top of `b` and the results when it is false
    /// and true; `None` for the constants.
    pub fn node(&self, b: Bool) -> Option<(Atom, Bool, Bool)> {
        let (v, lo, hi) = self.nodes[b.0 as usize];
        (v != TERMINAL).then(|| (self.atoms[v as usize], lo, hi))
    }

    /// The atoms `b` depends on, in variable order.
    pub fn support(&self, b: Bool) -> Vec<Atom> {
        let mut vars = self.vars(b);
        vars.sort_unstable();
        vars.into_iter().map(|v| self.atoms[v as usize]).collect()
    }

    fn vars(&self, b: Bool) -> Vec<u32> {
        let (mut seen, mut vars, mut stack) = (Vec::new(), Vec::new(), vec![b]);
        while let Some(b) = stack.pop() {
            let (v, lo, hi) = self.nodes[b.0 as usize];
            if v == TERMINAL || seen.contains(&b) {
                continue;
            }
            seen.push(b);
            if !vars.contains(&v) {
                vars.push(v);
            }
            stack.extend([lo, hi]);
        }
        vars
    }

    fn atom(&mut self, atom: Atom, born: usize) -> Bool {
        let v = *self.ids.entry(atom).or_insert_with(|| {
            self.atoms.push(atom);
            self.born.push(born);
            (self.atoms.len() - 1) as u32
        });
        self.mk(v, Bool::FALSE, Bool::TRUE)
    }

    fn mk(&mut self, v: u32, lo: Bool, hi: Bool) -> Bool {
        if lo == hi {
            return lo;
        }
        if let Some(&b) = self.unique.get(&(v, lo, hi)) {
            return b;
        }
        if self.nodes.len() >= MAX_NODES {
            // The caller fails once it sees `overflow`.
            self.overflow = true;
            return Bool::FALSE;
        }
        let b = Bool(self.nodes.len() as u32);
        self.nodes.push((v, lo, hi));
        self.unique.insert((v, lo, hi), b);
        b
    }

    /// Cofactors of `b` for `v`, which is `b`'s top variable or above it.
    fn split(&self, b: Bool, v: u32) -> (Bool, Bool) {
        match self.nodes[b.0 as usize] {
            (bv, lo, hi) if bv == v => (lo, hi),
            _ => (b, b),
        }
    }

    fn ite(&mut self, f: Bool, g: Bool, h: Bool) -> Bool {
        if f == Bool::TRUE || g == h {
            return g;
        }
        if f == Bool::FALSE {
            return h;
        }
        if (g, h) == (Bool::TRUE, Bool::FALSE) {
            return f;
        }
        if let Some(&r) = self.memo.get(&(f, g, h)) {
            return r;
        }
        let top = |b: Bool| self.nodes[b.0 as usize].0;
        let v = top(f).min(top(g)).min(top(h));
        let ((f0, f1), (g0, g1), (h0, h1)) = (self.split(f, v), self.split(g, v), self.split(h, v));
        let lo = self.ite(f0, g0, h0);
        let hi = self.ite(f1, g1, h1);
        let r = self.mk(v, lo, hi);
        self.memo.insert((f, g, h), r);
        r
    }

    fn and(&mut self, a: Bool, b: Bool) -> Bool {
        self.ite(a, b, Bool::FALSE)
    }

    fn or(&mut self, a: Bool, b: Bool) -> Bool {
        self.ite(a, Bool::TRUE, b)
    }

    fn not(&mut self, a: Bool) -> Bool {
        self.ite(a, Bool::FALSE, Bool::TRUE)
    }

    fn xor(&mut self, a: Bool, b: Bool) -> Bool {
        let nb = self.not(b);
        self.ite(a, nb, b)
    }

    /// Whether some lane can satisfy every `(b, holds)` of `all` at once:
    /// `b` where `holds`, its negation where not. Yes when the search grows
    /// too large to finish.
    pub fn meet(&self, all: &[(Bool, bool)]) -> bool {
        self.meet_seen(all, &mut Meets::default())
    }

    /// [`Self::meet`], which looks up in `seen` what earlier calls on these
    /// `Bools` found out and adds what it finds. The answer is the one
    /// `meet` gives.
    pub fn meet_seen(&self, all: &[(Bool, bool)], seen: &mut Meets) -> bool {
        self.meet_within(all, MAX_MEET, seen)
    }

    /// [`Self::meet_seen`] with `budget` case splits.
    fn meet_within(&self, all: &[(Bool, bool)], mut budget: usize, seen: &mut Meets) -> bool {
        let mut work = all
            .iter()
            .map(|&(b, holds)| b.0 << 1 | u32::from(holds))
            .collect();
        self.meet_in(0, &mut work, &mut budget, seen)
    }

    /// The search of [`Self::meet`] for the conditions `work[at..]` (a node
    /// doubled, plus one where it must hold), which it takes off `work`.
    ///
    /// The search is a case split on the top variable, and the cofactors of
    /// a few large functions come down to the same conditions over and over
    /// (a block's reach under a mask and under its negation share most of
    /// their nodes), so splitting each anew is exponential where the
    /// distinct conditions are few. `seen` keeps the conditions that no lane
    /// satisfies with the case splits it took to find that out: meeting
    /// them again takes those splits off the budget at once. A search
    /// therefore gives up exactly where one that split everything anew
    /// would (nothing below such conditions ends it early, so it either
    /// makes all those splits or runs out among them), and gives the same
    /// answer.
    fn meet_in(
        &self,
        at: usize,
        work: &mut Vec<u32>,
        budget: &mut usize,
        seen: &mut Meets,
    ) -> bool {
        // Constants either fail or say nothing.
        let mut kept = at;
        for i in at..work.len() {
            let (b, holds) = (work[i] >> 1, work[i] & 1 != 0);
            if b > Bool::TRUE.0 {
                work[kept] = work[i];
                kept += 1;
            } else if (b == Bool::TRUE.0) != holds {
                work.truncate(at);
                return false;
            }
        }
        work.truncate(kept);
        if kept == at || *budget == 0 {
            work.truncate(at);
            return true;
        }
        // As a set: neither order nor repeats change the search.
        work[at..].sort_unstable();
        let mut end = at + 1;
        for i in at + 1..kept {
            if work[i] != work[end - 1] {
                work[end] = work[i];
                end += 1;
            }
        }
        work.truncate(end);
        if let Some(&splits) = seen.none.get(&work[at..]) {
            let within = splits <= *budget;
            *budget = budget.saturating_sub(splits);
            work.truncate(at);
            return !within;
        }
        let before = *budget;
        *budget -= 1;
        let top = |code: u32| self.nodes[(code >> 1) as usize].0;
        let v = work[at..].iter().map(|&c| top(c)).min().expect("not empty");
        let mut found = false;
        for value in [false, true] {
            for i in at..end {
                let (lo, hi) = self.split(Bool(work[i] >> 1), v);
                work.push((if value { hi } else { lo }).0 << 1 | work[i] & 1);
            }
            if self.meet_in(end, work, budget, seen) {
                found = true;
                break;
            }
        }
        if !found && seen.none.len() < MAX_MEETS {
            seen.none.insert(work[at..end].into(), before - *budget);
        }
        work.truncate(at);
        found
    }

    fn implies(&mut self, a: Bool, b: Bool) -> bool {
        let nb = self.not(b);
        self.and(a, nb) == Bool::FALSE
    }

    /// `b` with every atom born at reverse-postorder index `from` or later
    /// existentially quantified: what still holds once those are redefined.
    fn forget(&mut self, b: Bool, from: usize, memo: &mut Map<Bool, Bool>) -> Bool {
        let (v, lo, hi) = self.nodes[b.0 as usize];
        if v == TERMINAL {
            return b;
        }
        if let Some(&r) = memo.get(&b) {
            return r;
        }
        let lo = self.forget(lo, from, memo);
        let hi = self.forget(hi, from, memo);
        let r = if self.born[v as usize] >= from {
            self.or(lo, hi)
        } else {
            self.mk(v, lo, hi)
        };
        memo.insert(b, r);
        r
    }

    fn restrict(&mut self, b: Bool, v: u32, value: bool, memo: &mut Map<Bool, Bool>) -> Bool {
        let (bv, lo, hi) = self.nodes[b.0 as usize];
        if bv == TERMINAL || bv > v {
            return b;
        }
        if bv == v {
            return if value { hi } else { lo };
        }
        if let Some(&r) = memo.get(&b) {
            return r;
        }
        let l = self.restrict(lo, v, value, memo);
        let h = self.restrict(hi, v, value, memo);
        let r = self.mk(bv, l, h);
        memo.insert(b, r);
        r
    }

    /// `b` with each atom that `known` decides replaced by its value.
    fn given(&mut self, mut b: Bool, known: Bool) -> Bool {
        for v in self.vars(b) {
            let x = self.mk(v, Bool::FALSE, Bool::TRUE);
            let nx = self.not(x);
            if self.implies(known, x) {
                b = self.restrict(b, v, true, &mut Map::default());
            } else if self.implies(known, nx) {
                b = self.restrict(b, v, false, &mut Map::default());
            }
        }
        b
    }
}

/// How a lane leaves a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaneExit {
    Jump(usize),
    /// To `taken` in lanes where `when` holds. A condition over
    /// [`Atom::Uniform`]s only is a uniform branch.
    Branch {
        when: Bool,
        taken: usize,
        not_taken: usize,
    },
    End,
    Indirect,
}

/// A [`Program`] as one lane runs it: same blocks, per-lane branches.
#[derive(Debug, Clone)]
pub struct Structured {
    pub bools: Bools,
    /// Per block.
    pub exits: Vec<LaneExit>,
    /// Per block and instruction: the condition, beyond reaching it, under
    /// which a lane runs a vector instruction; `None` when reaching it is
    /// enough (lanes outside EXEC then never get there, so a VGPR write
    /// needs no old value). Always `None` for scalar instructions.
    pub guards: Vec<Vec<Option<Bool>>>,
    /// Per block: what holds in every lane that enters it (over forward
    /// edges; a loop header forgets what its loop redefines).
    pub reach: Vec<Bool>,
    /// The lane's bit of each lane-mask value, by its low dword's value.
    pub masks: HashMap<Value, Bool>,
    /// Exact Wave64 pairs for checking masks at their uses.
    paired_masks: Map<Key, Bool>,
    /// Wave64 masks: low dword's value by the high dword's.
    pub highs: HashMap<Value, Value>,
    /// Masks (low dwords) set only in lanes whose EXEC bit is set where
    /// they are written: every such lane reaches the writing block, so a
    /// ballot there gives the mask as data. No split low dword, as in
    /// `masks`.
    pub active: HashSet<Value>,
    /// Incoming lane boolean per predecessor edge (in block predecessor order)
    /// for each [`Atom::Phi`] id.
    pub phi_masks: HashMap<Value, Vec<Bool>>,
    /// Blocks whose closing branch the whole wave takes together ([`structure_wide`]): its
    /// `when` is still the lane's own bit; the wave goes by all of them.
    pub wide: HashSet<usize>,
    /// Per block closed by a branch on a lane mask: whether the console takes it when some
    /// lane's `when` holds (`true`) or when every lane's does (`false`).
    pub wave_any: Vec<Option<bool>>,
    /// SCC values a mask operation sets when some lane's bit of its result is set, with that
    /// lane bit: as data such an SCC is "some lane of the wave has it".
    pub scc_some: HashMap<Value, Bool>,
}

impl Structured {
    pub(crate) fn mask_bits(&self, low: Value, high: Option<Value>) -> Option<Bool> {
        match high {
            Some(high) => self.paired_masks.get(&(low, high)),
            None => self.masks.get(&low),
        }
        .copied()
    }
}

/// Builds the per-lane form of `program`.
pub fn structure(program: &Program) -> Result<Structured, ShaderError> {
    structure_wide(program, &HashSet::new())
}

/// [`structure`], with the branches that close the blocks of `wide` kept as the console has
/// them: one decision for the whole wave (`s_cbranch_execz` and `vccz`: taken when no lane's
/// bit is set; `execnz` and `vccnz`: when some lane's is; SCC of a mask operation likewise,
/// [`Structured::wave_any`]), which every lane follows. A lane
/// then learns nothing about its own bit from the way it went, so past such a branch EXEC
/// stays a guard on every vector instruction, as it is on the console. For a console wave
/// run as two host waves, which exchange that decision and can then meet (a group barrier)
/// inside the branch.
pub fn structure_wide(program: &Program, wide: &HashSet<usize>) -> Result<Structured, ShaderError> {
    let mut l = Lanes::new(program)?;
    l.wide = wide.clone();
    loop {
        let execs = l.masks_in_order();
        let cond = l.paths()?;
        let (wrong, new_derived) = l.check_joins(&cond);
        if l.b.overflow {
            return Err(TOO_LARGE);
        }
        if wrong.is_empty() && new_derived.is_empty() {
            return l.finish(&execs, &cond);
        }
        l.unresolved.extend(wrong);
        l.derived_phis.extend(new_derived);
        l.reset();
    }
}

/// A lane mask by the values of its low and high dword (both the low one in
/// wave32).
type Key = (Value, Value);

struct Lanes<'a> {
    p: &'a Program,
    order: Vec<usize>,
    /// Reverse-postorder index per block.
    index: Vec<usize>,
    /// Per block, in predecessor order: (block, over its taken edge).
    incoming: Vec<Vec<(usize, bool)>>,
    /// Block and instruction index writing each value.
    defs: Map<Value, (usize, usize)>,
    /// Block and index of each phi value.
    phis: Map<Value, (usize, usize)>,
    /// Mask joins found wrong when resolved to a constant: atoms instead.
    unresolved: HashSet<Value>,
    /// Phi ids whose joins depend on EXEC or VALU results (including along
    /// back edges), carried across resets so their joins stay in `masks`.
    derived_phis: HashSet<Value>,
    b: Bools,
    /// Masks derived from EXEC and VALU results.
    masks: Map<Key, Bool>,
    /// Other values read as masks: constants, uniform selects, opaque ones.
    other: Map<Key, Bool>,
    /// Pairs read as masks that `mask` does not see: carry-ins, and pairs
    /// joins take over back edges.
    seen: Vec<Key>,
    /// For branches: SCC of a mask operation ("some lane is set") as the
    /// lane's own bit.
    scc_lane: Map<Value, Bool>,
    /// Joins resolved to a constant, to check once all edges are known.
    joins: Vec<Join>,
    /// [`Atom::Phi`] joins created in the current pass, resolved in
    /// `check_joins` and exported in `finish`.
    phi_joins: Vec<PhiJoin>,
    phi_join_ids: HashSet<Value>,
    /// Per block: what a lane knows on entry, over forward edges only.
    first: Vec<Bool>,
    /// Per block ending in a branch: when a lane takes it.
    when: Vec<Option<Bool>>,
    /// See [`Structured::active`].
    active: HashSet<Value>,
    /// See [`Structured::wide`].
    wide: HashSet<usize>,
    /// SCC values of [`Self::scc_lane`] that are set when EVERY lane's bit is (`s_cmp_eq` of
    /// two masks: "no lane differs"); the others are set when some lane's is.
    scc_every: HashSet<Value>,
    /// See [`Structured::wave_any`].
    wave_any: Vec<Option<bool>>,
}

/// A mask join taken as `value` in every lane that reaches `block`: each
/// incoming mask held (or failed) on its edge.
struct Join {
    block: usize,
    /// The phi value naming the join.
    id: Value,
    value: bool,
    derived: bool,
    args: Vec<Key>,
}

/// A mask join represented as [`Atom::Phi`], whose incoming edge masks are
/// resolved once every block (including loop back edges) has been visited.
struct PhiJoin {
    block: usize,
    id: Value,
    derived: bool,
    args: Vec<Key>,
}

struct SopMasks {
    dst: Option<(Key, Bool)>,
    exec: Option<(Key, Bool)>,
    /// Some operand derives from EXEC or a VALU result.
    derived: bool,
    /// For branches: SCC ("result is not zero") per lane.
    scc: Option<Bool>,
}

const MAX_ROUNDS: usize = 256;

impl<'a> Lanes<'a> {
    fn new(p: &'a Program) -> Result<Self, ShaderError> {
        let blocks = &p.blocks;
        let order = crate::ir::reverse_postorder(blocks);
        let mut index = vec![usize::MAX; blocks.len()];
        for (i, &b) in order.iter().enumerate() {
            index[b] = i;
        }
        check_reducible(p, &order, &index)?;
        let mut incoming = vec![Vec::new(); blocks.len()];
        let (mut defs, mut phis) = (Map::default(), Map::default());
        for (b, block) in blocks.iter().enumerate() {
            for (k, s) in block.exit.successors().into_iter().enumerate() {
                incoming[s].push((b, k == 0));
            }
            for (i, phi) in block.phis.iter().enumerate() {
                phis.insert(phi.value, (b, i));
            }
            for (i, inst) in block.insts.iter().enumerate() {
                for &(_, v) in &inst.writes {
                    defs.insert(v, (b, i));
                }
            }
        }
        Ok(Lanes {
            p,
            order,
            index,
            incoming,
            defs,
            phis,
            unresolved: HashSet::new(),
            derived_phis: HashSet::new(),
            b: Bools::new(),
            masks: Map::default(),
            other: Map::default(),
            seen: Vec::new(),
            scc_lane: Map::default(),
            joins: Vec::new(),
            phi_joins: Vec::new(),
            phi_join_ids: HashSet::new(),
            first: vec![Bool::FALSE; blocks.len()],
            when: vec![None; blocks.len()],
            active: HashSet::new(),
            wide: HashSet::new(),
            scc_every: HashSet::new(),
            wave_any: vec![None; blocks.len()],
        })
    }

    fn reset(&mut self) {
        self.b = Bools::new();
        self.masks.clear();
        self.other.clear();
        self.seen.clear();
        self.scc_lane.clear();
        self.scc_every.clear();
        self.wave_any.fill(None);
        self.joins.clear();
        self.phi_joins.clear();
        self.phi_join_ids.clear();
        self.first.fill(Bool::FALSE);
        self.when.fill(None);
        self.active.clear();
    }

    /// Lane masks, branch conditions and forward-edge knowledge, block by
    /// block in reverse postorder; returns EXEC at each instruction reading it.
    fn masks_in_order(&mut self) -> Vec<Vec<Option<Bool>>> {
        let p = self.p;
        let input = |reg| p.inputs.iter().find(|i| i.0 == reg).map(|i| i.1);
        if let Some(lo) = input(Reg::S(EXEC)) {
            let hi = if p.wave64 {
                input(Reg::S(EXEC + 1)).unwrap_or(lo)
            } else {
                lo
            };
            self.masks.insert((lo, hi), Bool::TRUE);
            self.active.insert(lo);
        }
        let mut execs: Vec<Vec<Option<Bool>>> =
            p.blocks.iter().map(|b| vec![None; b.insts.len()]).collect();
        for n in 0..self.order.len() {
            let b = self.order[n];
            let mut c = if n == 0 { Bool::TRUE } else { Bool::FALSE };
            for k in 0..self.incoming[b].len() {
                let (q, taken) = self.incoming[b][k];
                if self.index[q] < n {
                    let e = self.edge(self.first[q], q, taken);
                    c = self.b.or(c, e);
                }
            }
            self.first[b] = c;
            // EXEC joined at the block's entry holds the lanes that arrive.
            for phi in &p.blocks[b].phis {
                if phi.reg == Reg::S(EXEC) {
                    self.active.insert(phi.value);
                }
            }
            // EXEC where known in this block (read or written so far).
            let mut now = None;
            for (i, inst) in p.blocks[b].insts.iter().enumerate() {
                if inst.read(Reg::S(EXEC)).is_some() {
                    let e = self.exec(inst);
                    execs[b][i] = Some(e);
                    now = Some(e);
                }
                self.step(inst, now);
                if let Some(k) = self.key(|r| inst.write(r), EXEC) {
                    now = self.masks.get(&k).or(self.other.get(&k)).copied();
                }
            }
            if let Exit::Branch { .. } = p.blocks[b].exit
                && let Some(last) = p.blocks[b].insts.last()
            {
                self.when[b] = Some(self.branch(last));
                self.wave_any[b] = self.wave_kind(last);
            }
        }
        execs
    }

    /// `c` narrowed by the branch at the end of `q` towards one successor.
    fn edge(&mut self, c: Bool, q: usize, taken: bool) -> Bool {
        if self.wide.contains(&q) {
            // the wave's decision: it says nothing about this lane's own bit
            return c;
        }
        match self.when[q] {
            Some(w) if taken => self.b.and(c, w),
            Some(w) => {
                let nw = self.b.not(w);
                self.b.and(c, nw)
            }
            None => c,
        }
    }

    /// Joins whose constant fails on some edge, and joins that need another
    /// pass because a later edge turned out to derive from EXEC or VALU.
    fn check_joins(&mut self, cond: &[Bool]) -> (Vec<Value>, Vec<Value>) {
        let mut wrong = Vec::new();
        let mut new_derived = Vec::new();
        let mut seen_derived = HashSet::new();
        let mut j = 0;
        let mut p = 0;
        // Checking an argument can resolve another join; it is checked too.
        while j < self.joins.len() || p < self.phi_joins.len() {
            while j < self.joins.len() {
                let (block, id, value, was_derived) = (
                    self.joins[j].block,
                    self.joins[j].id,
                    self.joins[j].value,
                    self.joins[j].derived,
                );
                let mut failed = false;
                for (n, a) in self.joins[j].args.clone().into_iter().enumerate() {
                    let (q, taken) = self.incoming[block][n];
                    let e = self.edge(cond[q], q, taken);
                    let (mut m, d) = self.mask(a);
                    if !was_derived
                        && d
                        && !self.derived_phis.contains(&id)
                        && seen_derived.insert(id)
                    {
                        new_derived.push(id);
                    }
                    if !value {
                        m = self.b.not(m);
                    }
                    if !failed && !self.b.implies(e, m) {
                        wrong.push(id);
                        failed = true;
                    }
                }
                j += 1;
            }
            while p < self.phi_joins.len() {
                let (id, was_derived) = (self.phi_joins[p].id, self.phi_joins[p].derived);
                for a in self.phi_joins[p].args.clone() {
                    let (_, d) = self.mask(a);
                    if !was_derived
                        && d
                        && !self.derived_phis.contains(&id)
                        && seen_derived.insert(id)
                    {
                        new_derived.push(id);
                    }
                }
                p += 1;
            }
        }
        (wrong, new_derived)
    }

    fn finish(
        mut self,
        execs: &[Vec<Option<Bool>>],
        cond: &[Bool],
    ) -> Result<Structured, ShaderError> {
        let phi_joins = std::mem::take(&mut self.phi_joins);
        let mut phi_masks = HashMap::with_capacity(phi_joins.len());
        for pj in phi_joins {
            let incoming: Vec<Bool> = pj
                .args
                .into_iter()
                .enumerate()
                .map(|(n, a)| {
                    let (q, taken) = self.incoming[pj.block][n];
                    let e = self.edge(cond[q], q, taken);
                    let m = self.mask(a).0;
                    self.b.given(m, e)
                })
                .collect();
            phi_masks.insert(pj.id, incoming);
        }
        let blocks = &self.p.blocks;
        let b = &mut self.b;
        let guards = blocks
            .iter()
            .enumerate()
            .map(|(n, block)| {
                block
                    .insts
                    .iter()
                    .zip(&execs[n])
                    .map(|(inst, exec)| match (exec, &inst.fields) {
                        (None, _)
                        | (_, Fields::Sop { .. } | Fields::Sopp { .. } | Fields::Smem { .. }) => {
                            None
                        }
                        (&Some(e), _) if b.implies(cond[n], e) => None,
                        (&Some(e), _) => Some(b.given(e, cond[n])),
                    })
                    .collect()
            })
            .collect();
        let exits = blocks
            .iter()
            .enumerate()
            .map(|(n, block)| match block.exit {
                Exit::Jump(t) => LaneExit::Jump(t),
                Exit::End => LaneExit::End,
                Exit::Indirect => LaneExit::Indirect,
                Exit::Branch { taken, not_taken } => LaneExit::Branch {
                    when: b.given(self.when[n].unwrap_or(Bool::FALSE), cond[n]),
                    taken,
                    not_taken,
                },
            })
            .collect();
        if self.b.overflow {
            return Err(TOO_LARGE);
        }
        // Exports go by low dword. A low dword read with two high dwords
        // (one rewritten alone) names no one mask: it is left out, so
        // lowering refuses what reads it instead of taking the other
        // pair's lanes 32-63. Not where the low dword holds a number: see
        // the shortcut in `mask`.
        let (mut high, mut split) = (Map::default(), HashSet::new());
        let keys = self.masks.keys().chain(self.other.keys());
        for &(lo, hi) in keys.chain(&self.seen) {
            if *high.entry(lo).or_insert(hi) != hi {
                split.insert(lo);
            }
        }
        self.masks.retain(|k, _| !split.contains(&k.0));
        self.active.retain(|lo| !split.contains(lo));
        Ok(Structured {
            masks: self.masks.iter().map(|(&(lo, _), &m)| (lo, m)).collect(),
            highs: self
                .masks
                .keys()
                .filter(|(lo, hi)| lo != hi)
                .map(|&(lo, hi)| (hi, lo))
                .collect(),
            paired_masks: if self.p.wave64 { self.masks } else { Map::default() },
            active: self.active,
            wide: self.wide,
            wave_any: self.wave_any,
            scc_some: self
                .scc_lane
                .iter()
                .filter(|(scc, _)| !self.scc_every.contains(scc))
                .map(|(&scc, &m)| (scc, m))
                .collect(),
            phi_masks,
            bools: self.b,
            exits,
            guards,
            reach: cond.to_vec(),
        })
    }

    fn key(&self, value: impl Fn(Reg) -> Option<Value>, code: u8) -> Option<Key> {
        let lo = value(Reg::S(code))?;
        let hi = if self.p.wave64 {
            value(Reg::S(code + 1))?
        } else {
            lo
        };
        Some((lo, hi))
    }

    fn born(&self, v: Value) -> usize {
        match (self.defs.get(&v), self.phis.get(&v)) {
            (Some(&(b, _)), _) | (_, Some(&(b, _))) => self.index[b],
            _ => 0,
        }
    }

    fn exec(&mut self, inst: &Inst) -> Bool {
        match self.key(|r| inst.read(r), EXEC) {
            Some(k) => self.mask(k).0,
            None => Bool::TRUE,
        }
    }

    /// The mask a value pair holds, and whether it derives from EXEC or a
    /// VALU result.
    fn mask(&mut self, k: Key) -> (Bool, bool) {
        if let Some(&m) = self.masks.get(&k) {
            return (m, true);
        }
        if let Some(&m) = self.other.get(&k) {
            return (m, false);
        }
        let p = self.p;
        let (m, derived) = match self.join(k) {
            Some(join) => join,
            None => {
                let (lo, hi) = (self.defs.get(&k.0).copied(), self.defs.get(&k.1).copied());
                let def = |d: Option<(usize, usize)>| d.map(|(b, i)| &p.blocks[b].insts[i]);
                let m = match (lo, hi) {
                    (Some(at), Some(hi)) if at == hi => def(lo)
                        .and_then(|i| self.sop(i))
                        .and_then(|r| r.dst)
                        .map(|d| d.1),
                    _ => None,
                };
                // What an EXEC write the pass does not follow leaves there.
                let exec_write = |d: Option<(usize, usize)>, v: Value| {
                    def(d).is_some_and(|i| {
                        i.write(Reg::S(EXEC)) == Some(v) || i.write(Reg::S(EXEC + 1)) == Some(v)
                    })
                };
                let derived = exec_write(lo, k.0) || exec_write(hi, k.1);
                // shortcut: the atom names the pair by its low dword, and
                // lowering takes the high dword written beside it. For halves
                // written apart (two `s_mov_b32`) lanes 32-63 are wrong unless
                // lowering has no value for the low dword (EXEC, compares)
                // and refuses. Give the atom both dwords when such code
                // turns up.
                let m = m.unwrap_or_else(|| {
                    let born = self.born(k.0);
                    self.b.atom(Atom::Opaque(k.0), born)
                });
                (m, derived)
            }
        };
        let cache = if derived {
            &mut self.masks
        } else {
            &mut self.other
        };
        cache.insert(k, m);
        (m, derived)
    }

    /// A mask joined by phis at a block entry, and whether a mask coming in
    /// on a forward edge derives from EXEC or a VALU result. Where every
    /// lane arriving so far has the incoming mask set (or clear), the join
    /// is that constant, for a lane can only read it after arriving.
    fn join(&mut self, k: Key) -> Option<(Bool, bool)> {
        let p = self.p;
        let (lo, hi) = (self.phis.get(&k.0).copied(), self.phis.get(&k.1).copied());
        let block = match (lo, hi) {
            (Some((a, _)), Some((b, _))) if a != b => return None,
            (Some((b, _)), _) | (_, Some((b, _))) => b,
            _ => return None,
        };
        let target = &p.blocks[block];
        let arg = |phi: Option<(usize, usize)>, own: Value, n: usize| {
            phi.map_or(own, |(_, i)| target.phis[i].args[n])
        };
        let args: Vec<Key> = (0..target.preds.len())
            .map(|n| (arg(lo, k.0, n), arg(hi, k.1, n)))
            .collect();
        let id = lo.map_or(k.1, |_| k.0);
        let mut derived = self.derived_phis.contains(&id);
        // (edge condition, mask) per forward edge; back edges come later.
        let mut forward = Vec::new();
        for (n, &a) in args.iter().enumerate() {
            let (q, taken) = self.incoming[block][n];
            if self.index[q] < self.index[block] {
                let (m, d) = self.mask(a);
                derived |= d;
                let e = self.edge(self.first[q], q, taken);
                forward.push((e, m));
            } else {
                self.seen.push(a);
            }
        }
        if forward.len() == args.len()
            && let Some(&(_, m)) = forward.first()
            && forward.iter().all(|f| f.1 == m)
        {
            return Some((m, derived));
        }
        if !self.unresolved.contains(&id) {
            for value in [true, false] {
                let b = &mut self.b;
                let holds = forward.iter().all(|&(e, m)| {
                    let m = if value { m } else { b.not(m) };
                    b.implies(e, m)
                });
                if holds {
                    self.joins.push(Join {
                        block,
                        id,
                        value,
                        derived,
                        args,
                    });
                    let m = if value { Bool::TRUE } else { Bool::FALSE };
                    return Some((m, derived));
                }
            }
        }
        if self.phi_join_ids.insert(id) {
            self.phi_joins.push(PhiJoin {
                block,
                id,
                derived,
                args,
            });
        }
        let born = self.index[block];
        Some((self.b.atom(Atom::Phi(id), born), derived))
    }

    fn operand(&mut self, inst: &Inst, op: &Operand) -> Option<(Bool, bool)> {
        let wave64 = self.p.wave64;
        let constant = |b: &mut Bools, v: u64| {
            let v = if wave64 { v } else { v & 0xFFFF_FFFF };
            match v {
                0 => Bool::FALSE,
                _ if v == u64::MAX >> (32 * u8::from(!wave64)) => Bool::TRUE,
                _ => b.atom(Atom::Constant(v), 0),
            }
        };
        Some(match *op {
            Operand::Scalar { code, .. } if code != NULL => {
                let k = self.key(|r| inst.read(r), code)?;
                self.mask(k)
            }
            Operand::Const(c @ 128..=208) => {
                let v = if c <= 192 {
                    i64::from(c) - 128
                } else {
                    192 - i64::from(c)
                };
                (constant(&mut self.b, v as u64), false)
            }
            Operand::Literal(x) => (constant(&mut self.b, u64::from(x)), false),
            _ => return None,
        })
    }

    /// Lane-mask meaning of a lane-width scalar operation.
    fn sop(&mut self, inst: &Inst) -> Option<SopMasks> {
        let Fields::Sop { dst, src, .. } = &inst.fields else {
            return None;
        };
        let width = if self.p.wave64 { "_b64" } else { "_b32" };
        let op = inst.name().strip_prefix("s_")?.strip_suffix(width)?;
        let dst = match dst {
            Some(Operand::Scalar { code, .. }) if *code != NULL => {
                self.key(|r| inst.write(r), *code)
            }
            _ => None,
        };
        let exec_out = self.key(|r| inst.write(r), EXEC);
        let (save, f) = match (op.strip_suffix("_saveexec"), op.strip_suffix("_wrexec")) {
            (Some(f), _) => (Some(true), f),
            (_, Some(f)) => (Some(false), f),
            _ => (None, op),
        };
        let mut derived = exec_out.is_some() || save.is_some();
        let mut arg = |n: usize, l: &mut Self| {
            let (m, d) = l.operand(inst, src.get(n)?)?;
            derived |= d;
            Some(m)
        };
        if let Some(saveexec) = save {
            let s = arg(0, self)?;
            let exec = self.exec(inst);
            let new = self.combine(f, s, exec)?;
            let saved = if saveexec { exec } else { new };
            return Some(SopMasks {
                dst: dst.map(|d| (d, saved)),
                exec: exec_out.map(|e| (e, new)),
                derived,
                scc: Some(new),
            });
        }
        let (result, scc) = match op {
            "mov" => (arg(0, self)?, None),
            "not" => {
                let a = arg(0, self)?;
                let r = self.b.not(a);
                (r, Some(r))
            }
            "wqm" => {
                let a = arg(0, self)?;
                let lo = dst.or(exec_out)?.0;
                let born = self.born(lo);
                let q = self.b.atom(Atom::Quad(lo), born);
                let r = self.b.or(a, q);
                (r, Some(r))
            }
            "cselect" => {
                let (a, c) = (arg(0, self)?, arg(1, self)?);
                let scc = inst.read(Reg::Scc)?;
                if self.clear_after_loop(scc) {
                    (c, None)
                } else {
                    let born = self.born(scc);
                    let u = self.b.atom(Atom::Uniform(scc), born);
                    (self.b.ite(u, a, c), None)
                }
            }
            _ => {
                let (a, c) = (arg(0, self)?, arg(1, self)?);
                let r = self.combine(op, a, c)?;
                (r, Some(r))
            }
        };
        Some(SopMasks {
            dst: dst.map(|d| (d, result)),
            exec: exec_out.map(|e| (e, result)),
            derived,
            scc,
        })
    }

    /// Whether SCC value `scc` is clear wherever it is read: the instruction before an
    /// `s_cbranch_execnz` that closes a loop wrote it as "the EXEC it leaves is not
    /// empty" (`s_andn2_wrexec`, say), and the wave leaves `do { .. } while (EXEC)`
    /// with EXEC empty. A read inside the loop or behind another way out of it sees a
    /// join of this value, not the value itself.
    fn clear_after_loop(&self, scc: Value) -> bool {
        let Some(&(b, i)) = self.defs.get(&scc) else {
            return false;
        };
        let block = &self.p.blocks[b];
        let (Some(writer), Some(last)) = (block.insts.get(i), block.insts.last()) else {
            return false;
        };
        let exec = writer.write(Reg::S(EXEC));
        let back = match block.exit {
            Exit::Branch { taken, .. } => self.index[taken] <= self.index[b],
            _ => false,
        };
        back && i + 2 == block.insts.len()
            && exec.is_some()
            && last.name() == "s_cbranch_execnz"
            && last.read(Reg::S(EXEC)) == exec
            && (!self.p.wave64 || last.read(Reg::S(EXEC + 1)) == writer.write(Reg::S(EXEC + 1)))
    }

    fn combine(&mut self, f: &str, a: Bool, c: Bool) -> Option<Bool> {
        let b = &mut self.b;
        Some(match f {
            "and" => b.and(a, c),
            "or" => b.or(a, c),
            "xor" => b.xor(a, c),
            "andn2" => {
                let nc = b.not(c);
                b.and(a, nc)
            }
            "orn2" => {
                let nc = b.not(c);
                b.or(a, nc)
            }
            "andn1" => {
                let na = b.not(a);
                b.and(na, c)
            }
            "orn1" => {
                let na = b.not(a);
                b.or(na, c)
            }
            "nand" => {
                let r = b.and(a, c);
                b.not(r)
            }
            "nor" => {
                let r = b.or(a, c);
                b.not(r)
            }
            "xnor" => {
                let r = b.xor(a, c);
                b.not(r)
            }
            _ => return None,
        })
    }

    /// Records the masks `inst` defines; `exec` is EXEC there, if known.
    fn step(&mut self, inst: &Inst, exec: Option<Bool>) {
        let name = inst.name();
        match &inst.fields {
            Fields::Sop { src, .. } => {
                let mut takes_mask = false;
                for s in src {
                    if let Operand::Scalar { code, .. } = *s
                        && code != NULL
                        && let Some(k) = self.key(|r| inst.read(r), code)
                    {
                        let phi = self.phis.contains_key(&k.0) || self.phis.contains_key(&k.1);
                        takes_mask |= self.masks.contains_key(&k) || phi && self.mask(k).1;
                    }
                }
                let touches_exec =
                    inst.write(Reg::S(EXEC)).is_some() || inst.read(Reg::S(EXEC)).is_some();
                if (takes_mask || touches_exec)
                    && let Some(r) = self.sop(inst).filter(|r| r.derived)
                {
                    for (k, m) in r.dst.into_iter().chain(r.exec) {
                        self.masks.insert(k, m);
                        // EXEC itself holds the lanes that run here.
                        if r.exec.is_some_and(|e| e.0 == k)
                            || exec.is_some_and(|e| self.b.implies(m, e))
                        {
                            self.active.insert(k.0);
                        }
                    }
                    if let (Some(scc), Some(m)) = (inst.write(Reg::Scc), r.scc) {
                        self.scc_lane.insert(scc, m);
                    }
                }
                // `s_cmp_eq/lg` of a mask: "no lane differs" per lane.
                let width = if self.p.wave64 { "64" } else { "32" };
                if takes_mask
                    && let (Some(rest), Some(scc)) =
                        (name.strip_prefix("s_cmp_"), inst.write(Reg::Scc))
                    && rest
                        .strip_suffix(width)
                        .is_some_and(|r| r == "eq_u" || r == "lg_u")
                    && let [a, c] = &src[..]
                    && let (Some((a, _)), Some((c, _))) =
                        (self.operand(inst, a), self.operand(inst, c))
                {
                    let ne = self.b.xor(a, c);
                    let m = if rest.starts_with("eq") {
                        self.b.not(ne)
                    } else {
                        ne
                    };
                    self.scc_lane.insert(scc, m);
                    if rest.starts_with("eq") {
                        self.scc_every.insert(scc);
                    }
                }
                // `s_bcnt1` of a mask every running lane sees alike (EXEC under a scalar
                // condition, or EXEC itself): its SCC, "some lane is set", is each lane's own
                // bit. A branch on it then reads no ballot.
                if takes_mask
                    && name.starts_with("s_bcnt1_i32_b")
                    && let Some(scc) = inst.write(Reg::Scc)
                    && let [a] = &src[..]
                    && let Some((m, true)) = self.operand(inst, a)
                {
                    let e = exec.unwrap_or(Bool::TRUE);
                    let own = self.b.given(m, e);
                    if self.b.implies(m, e)
                        && self.b.support(own).iter().all(|a| matches!(a, Atom::Uniform(_)))
                    {
                        self.scc_lane.insert(scc, own);
                    }
                }
            }
            Fields::Valu(v) => {
                let exec = self.exec(inst);
                let lane = |l: &mut Self, k: Key| {
                    let born = l.born(k.0);
                    let a = l.b.atom(Atom::Lane(k.0), born);
                    let m = l.b.and(exec, a);
                    l.masks.insert(k, m);
                    l.active.insert(k.0);
                };
                // A select reads a mask nothing else may: a join of compare
                // results is resolved here, as for EXEC. A carry-in is only
                // recorded for `finish`: resolving its join would turn a
                // refusal into a per-lane phi, wrong after a divergent branch.
                if (name == "v_cndmask_b32" || name.contains("_co_ci_"))
                    && let Some(&Operand::Scalar { code, .. }) = v.src.get(2)
                    && code != NULL
                    && let Some(k) = self.key(|r| inst.read(r), code)
                {
                    if name == "v_cndmask_b32" {
                        self.mask(k);
                    } else {
                        self.seen.push(k);
                    }
                }
                if matches!(name, "v_div_fmas_f32" | "v_div_fmas_f64")
                    && let Some(k) = self.key(|r| inst.read(r), VCC)
                {
                    self.mask(k);
                }
                if name.starts_with("v_cmpx") {
                    if let Some(k) = self.key(|r| inst.write(r), EXEC) {
                        lane(self, k);
                    }
                } else {
                    let compare = v.dst.filter(|_| name.starts_with("v_cmp"));
                    for op in compare.iter().chain(&v.carry) {
                        if let Operand::Scalar { code, .. } = *op
                            && let Some(k) = self.key(|r| inst.write(r), code)
                        {
                            lane(self, k);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// The per-lane condition under which a block's closing branch is taken.
    /// How the console decides the branch `inst` on a lane mask, in terms of the lanes' own
    /// `when` ([`Self::branch`]): taken when some lane's holds (`true`) or when every lane's
    /// does (`false`). `execnz`/`vccnz` ask for some set bit, `execz`/`vccz` for none; SCC
    /// of a mask operation is "some lane's bit is set", SCC of `s_cmp_eq` of two masks is
    /// "every lane's is" ([`Self::scc_every`]), and `scc0` branches on the opposite. None
    /// for a branch on a scalar condition.
    fn wave_kind(&self, inst: &Inst) -> Option<bool> {
        match inst.name() {
            "s_cbranch_execnz" | "s_cbranch_vccnz" => Some(true),
            "s_cbranch_execz" | "s_cbranch_vccz" => Some(false),
            n @ ("s_cbranch_scc0" | "s_cbranch_scc1") => {
                let scc = inst.read(Reg::Scc)?;
                self.scc_lane.get(&scc)?;
                // set: SCC is 1 when some lane's bit is set (every lane's, for a compare)
                let every = self.scc_every.contains(&scc);
                Some((n == "s_cbranch_scc1") != every)
            }
            _ => None,
        }
    }

    fn branch(&mut self, inst: &Inst) -> Bool {
        let exec = self.exec(inst);
        match inst.name() {
            "s_cbranch_execz" => self.b.not(exec),
            "s_cbranch_execnz" => exec,
            n @ ("s_cbranch_vccz" | "s_cbranch_vccnz") => {
                let vcc = match self.key(|r| inst.read(r), VCC) {
                    Some(k) => self.mask(k).0,
                    None => Bool::FALSE,
                };
                let any = self.b.and(vcc, exec);
                if n.ends_with("nz") {
                    any
                } else {
                    self.b.not(any)
                }
            }
            n => {
                let Some(scc) = inst.read(Reg::Scc) else {
                    return Bool::FALSE;
                };
                let set = match self.scc_lane.get(&scc) {
                    Some(&m) => m,
                    None => {
                        let born = self.born(scc);
                        self.b.atom(Atom::Uniform(scc), born)
                    }
                };
                if n == "s_cbranch_scc1" {
                    set
                } else {
                    self.b.not(set)
                }
            }
        }
    }

    /// What a lane knows on entering each block, to a fixpoint. A loop
    /// header forgets what the back edge says about atoms the loop redefines.
    fn paths(&mut self) -> Result<Vec<Bool>, ShaderError> {
        let n = self.p.blocks.len();
        let mut cond = vec![Bool::FALSE; n];
        if let Some(&entry) = self.order.first() {
            cond[entry] = Bool::TRUE;
        }
        for _ in 0..MAX_ROUNDS {
            let mut changed = false;
            for i in 1..self.order.len() {
                let s = self.order[i];
                let mut c = Bool::FALSE;
                for k in 0..self.incoming[s].len() {
                    let (q, taken) = self.incoming[s][k];
                    let mut e = self.edge(cond[q], q, taken);
                    if i <= self.index[q] {
                        e = self.b.forget(e, i, &mut Map::default());
                    }
                    c = self.b.or(c, e);
                }
                if c != cond[s] {
                    cond[s] = c;
                    changed = true;
                }
            }
            if self.b.overflow {
                return Err(TOO_LARGE);
            }
            if !changed {
                return Ok(cond);
            }
        }
        Err(ShaderError::Isa {
            offset: 0,
            reason: "lane conditions do not settle",
        })
    }
}

const TOO_LARGE: ShaderError = ShaderError::Isa {
    offset: 0,
    reason: "lane conditions too large",
};

/// `ir::build` splits irreducible loops; checked again because everything
/// here assumes a reducible graph.
fn check_reducible(p: &Program, order: &[usize], index: &[usize]) -> Result<(), ShaderError> {
    match crate::ir::irreducible_edge(&p.blocks, order, index) {
        Some((from, _)) => Err(ShaderError::Unsupported {
            offset: p.blocks[from].offset,
            what: "irreducible control flow",
        }),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir;

    const S_ENDPGM: u32 = 0xBF81_0000;

    fn build(words: &[u32], wave64: bool) -> Program {
        let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        ir::build(&code, wave64, 256).unwrap()
    }

    fn guarded(s: &Structured) -> usize {
        s.guards.iter().flatten().filter(|g| g.is_some()).count()
    }

    /// The search of [`Bools::meet`] with every case split made anew, as it
    /// was before [`Meets`].
    fn meet_anew(b: &Bools, mut all: Vec<(Bool, bool)>, budget: &mut usize) -> bool {
        let fails = |&(b, holds): &(Bool, bool)| b == if holds { Bool::FALSE } else { Bool::TRUE };
        if all.iter().any(fails) {
            return false;
        }
        all.retain(|&(b, _)| b != Bool::FALSE && b != Bool::TRUE);
        if all.is_empty() || *budget == 0 {
            return true;
        }
        *budget -= 1;
        let top = |x: Bool| b.nodes[x.0 as usize].0;
        let v = all.iter().map(|&(x, _)| top(x)).min().expect("not empty");
        [false, true].into_iter().any(|value| {
            let next = all
                .iter()
                .map(|&(x, holds)| {
                    let (lo, hi) = b.split(x, v);
                    (if value { hi } else { lo }, holds)
                })
                .collect();
            meet_anew(b, next, budget)
        })
    }

    #[test]
    fn a_search_that_remembers_answers_as_one_made_anew() {
        // Functions of 12 variables put together at random, and conditions
        // of two to five of them (repeats and constants among them), under
        // budgets that run out early, late and never: the answer, and where
        // no lane is found the budget left, are those of the search that
        // remembers nothing; with what one `Meets` gathered over all of
        // them, and with an empty one.
        let mut b = Bools::new();
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        let mut random = move |n: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % n as u64) as usize
        };
        let mut f: Vec<Bool> = (0..12).map(|i| b.atom(Atom::Constant(i + 2), 0)).collect();
        f.extend([Bool::FALSE, Bool::TRUE]);
        for _ in 0..120 {
            let (x, y) = (f[random(f.len())], f[random(f.len())]);
            let made = match random(4) {
                0 => b.and(x, y),
                1 => b.or(x, y),
                2 => b.xor(x, y),
                _ => b.not(x),
            };
            f.push(made);
        }
        let mut seen = Meets::default();
        let (mut none, mut some, mut gave_up) = (0, 0, 0);
        for _ in 0..1500 {
            let all: Vec<(Bool, bool)> = (0..2 + random(4))
                .map(|_| (f[random(f.len())], random(2) == 0))
                .collect();
            for budget in [0, 1, 2, 3, 5, 9, 20, 60, 250, 1000, MAX_MEET] {
                let mut left = budget;
                let anew = meet_anew(&b, all.clone(), &mut left);
                for fresh in [false, true] {
                    let mut empty = Meets::default();
                    let seen = if fresh { &mut empty } else { &mut seen };
                    let mut work = all
                        .iter()
                        .map(|&(x, holds)| x.0 << 1 | u32::from(holds))
                        .collect();
                    let mut mine = budget;
                    let found = b.meet_in(0, &mut work, &mut mine, seen);
                    assert_eq!(found, anew, "{all:?} within {budget}");
                    assert!(work.is_empty());
                    if !found {
                        assert_eq!(mine, left, "{all:?} within {budget}");
                    }
                }
                assert_eq!(b.meet_within(&all, budget, &mut seen), anew);
                if !anew {
                    none += 1;
                } else if meet_anew(&b, all.clone(), &mut usize::MAX.clone()) {
                    some += 1;
                } else {
                    gave_up += 1;
                }
            }
        }
        // All three outcomes came up, and searches were remembered.
        assert!(
            none > 100 && some > 100 && gave_up > 100,
            "{none} {some} {gave_up}"
        );
        assert!(seen.none.len() > 100);
    }

    #[test]
    fn divergent_if_becomes_a_lane_branch() {
        let p = build(
            &[
                0x7D84_0080, // v_cmp_eq_u32 vcc, 0, v0
                0xBE80_3C6A, // s_and_saveexec_b32 s0, vcc_lo
                0xBF88_0001, // s_cbranch_execz -> 16
                0x7E02_0280, // v_mov_b32 v1, 0
                0x887E_007E, // 16: s_or_b32 exec_lo, exec_lo, s0
                0x7E04_0280, // v_mov_b32 v2, 0
                S_ENDPGM,
            ],
            false,
        );
        let s = structure(&p).unwrap();
        let LaneExit::Branch {
            when,
            taken: 2,
            not_taken: 1,
        } = s.exits[0]
        else {
            panic!("{:?}", s.exits)
        };
        // Skipped exactly where the compare fails.
        let cmp = p.blocks[0].insts[0].write(Reg::S(VCC)).unwrap();
        assert_eq!(
            s.bools.node(when),
            Some((Atom::Lane(cmp), Bool::TRUE, Bool::FALSE))
        );
        assert_eq!(guarded(&s), 0, "{:?}", s.guards);
    }

    #[test]
    fn exec_narrowed_without_a_branch_leaves_a_guard() {
        let p = build(
            &[
                0x7DA4_0080, // v_cmpx_eq_u32 exec, 0, v0
                0x7E02_0280, // v_mov_b32 v1, 0
                S_ENDPGM,
            ],
            true,
        );
        let s = structure(&p).unwrap();
        assert_eq!(s.guards[0][0], None, "the compare runs in every lane");
        let g = s.guards[0][1].expect("guard");
        assert!(matches!(s.bools.support(g)[..], [Atom::Lane(_)]));
    }

    #[test]
    fn divergent_loop_exits_per_lane() {
        let p = build(
            &[
                0xBE82_0480, // s_mov_b64 s[2:3], 0
                0x4A00_0081, // 4: v_add_nc_u32 v0, 1, v0
                0x7D84_0080, // v_cmp_eq_u32 vcc, 0, v0
                0x8882_026A, // s_or_b64 s[2:3], vcc, s[2:3]
                0x8AFE_027E, // s_andn2_b64 exec, exec, s[2:3]
                0xBF89_FFFB, // s_cbranch_execnz -> 4
                0x88FE_027E, // s_or_b64 exec, exec, s[2:3]
                0x7E02_0280, // v_mov_b32 v1, 0
                S_ENDPGM,
            ],
            true,
        );
        let s = structure(&p).unwrap();
        assert_eq!(guarded(&s), 0, "{:?}", s.guards);
        let LaneExit::Branch {
            when,
            taken: 1,
            not_taken: 2,
        } = s.exits[1]
        else {
            panic!("{:?}", s.exits)
        };
        // Back while the lane's compare fails.
        assert!(matches!(
            s.bools.node(when),
            Some((Atom::Lane(_), Bool::TRUE, Bool::FALSE))
        ));
    }

    /// `s_cselect` on the SCC a lane loop ended with (the blend-shape skinning
    /// programs): the loop is left when no lane remains, so the select takes
    /// its second operand; it was a uniform atom whose value no lane has.
    #[test]
    fn a_select_on_the_condition_a_lane_loop_ended_with_takes_its_second_operand() {
        let cnd = 256 | (256 << 9) | (3 << 18);
        let p = build(
            &[
                0xBE83_037E, // 0: s_mov_b32 s3, exec_lo
                0xBE9C_037E, // s_mov_b32 s28, exec_lo
                0x7E3A_0507, // 8: v_readfirstlane_b32 s29, v7
                0x7DA4_0E1D, // v_cmpx_eq_u32 exec_lo, s29, v7
                0xBE9C_471C, // s_andn2_wrexec_b32 s28, s28
                0xBF89_FFFC, // s_cbranch_execnz -> 8
                0xBEFE_0303, // s_mov_b32 exec_lo, s3
                0x8503_807E, // s_cselect_b32 s3, exec_lo, 0
                0xD501_0001,
                cnd, // v_cndmask_b32 v1, v0, v0, s3
                S_ENDPGM,
            ],
            false,
        );
        let s = structure(&p).unwrap();
        let select = p
            .blocks
            .iter()
            .flat_map(|b| &b.insts)
            .find(|i| i.name() == "s_cselect_b32")
            .expect("the select");
        let mask = select.write(Reg::S(3)).unwrap();
        assert_eq!(s.masks.get(&mask), Some(&Bool::FALSE));
    }

    #[test]
    fn scalar_branches_stay_uniform() {
        let p = build(
            &[
                0xBF06_0100, // s_cmp_eq_u32 s0, s1
                0xBF85_0001, // s_cbranch_scc1 -> 12
                0x7E02_0280, // v_mov_b32 v1, 0
                S_ENDPGM,    // 12
            ],
            true,
        );
        let s = structure(&p).unwrap();
        let LaneExit::Branch { when, .. } = s.exits[0] else {
            panic!()
        };
        assert!(matches!(s.bools.support(when)[..], [Atom::Uniform(_)]));
        assert_eq!(guarded(&s), 0);
    }

    #[test]
    fn a_mask_whose_high_dword_is_rewritten_is_not_the_compare() {
        for reader in [
            0x0202_0480, // v_cndmask_b32 v1, 0, v2, vcc
            0x5002_0480, // v_add_co_ci_u32 v1, vcc, 0, v2, vcc
        ] {
            let p = build(
                &[
                    0x7D84_0080, // v_cmp_eq_u32 vcc, 0, v0
                    0xBEEB_0380, // s_mov_b32 vcc_hi, 0
                    reader,
                    S_ENDPGM,
                ],
                true,
            );
            let s = structure(&p).unwrap();
            let lo = p.blocks[0].insts[2].read(Reg::S(VCC)).unwrap();
            assert_eq!(p.blocks[0].insts[0].write(Reg::S(VCC)), Some(lo));
            // The reader sees (compare low dword, 0): lanes 32-63 are clear.
            // The mask exported for that low dword must not be the compare.
            assert_ne!(
                s.masks.get(&lo).and_then(|&m| s.bools.node(m)),
                Some((Atom::Lane(lo), Bool::FALSE, Bool::TRUE)),
                "{reader:#x}: vcc_hi was zeroed after the compare"
            );
        }
    }

    #[test]
    fn a_mask_joined_over_a_back_edge_with_a_rewritten_high_dword_is_not_the_compare() {
        let p = build(
            &[
                0x7D84_0080, // v_cmp_eq_u32 vcc, 0, v0
                0x0202_0480, // 4: v_cndmask_b32 v1, 0, v2, vcc
                0x7D84_0280, // v_cmp_eq_u32 vcc, 0, v1
                0xBEEB_0380, // s_mov_b32 vcc_hi, 0
                0xBF85_FFFC, // s_cbranch_scc1 -> 4
                S_ENDPGM,
            ],
            true,
        );
        let s = structure(&p).unwrap();
        // The select reads the loop header's join; over the back edge it
        // takes (compare low dword, 0), which only the join sees.
        let lo = p.blocks[1].insts[1].write(Reg::S(VCC)).unwrap();
        assert_ne!(
            s.masks.get(&lo).and_then(|&m| s.bools.node(m)),
            Some((Atom::Lane(lo), Bool::FALSE, Bool::TRUE))
        );
    }

    #[test]
    fn irreducible_loops_are_split() {
        let p = build(
            &[
                0xBF06_0100, // s_cmp_eq_u32 s0, s1
                0xBF85_0001, // s_cbranch_scc1 -> 12, into the loop's middle
                0x8002_8102, // 8: s_add_u32 s2, s2, 1
                0xBF84_FFFE, // 12: s_cbranch_scc0 -> 8
                S_ENDPGM,
            ],
            false,
        );
        // The loop keeps 12 as its header; the entry into 8 gets a copy of it.
        let copy = &p.blocks[4];
        assert_eq!((copy.offset, copy.exit), (8, ir::Exit::Jump(2)));
        assert_eq!(p.blocks[0].exit.successors(), [2, 4]);
        assert_eq!(p.blocks[1].preds, [2]);
        assert!(structure(&p).is_ok());
    }

    #[test]
    fn mask_phi_joining_constant_and_compare_records_edge_bools() {
        let cnd = 258 | (259 << 9) | (2 << 18);
        let p = build(
            &[
                0xBF06_8000, // 0: s_cmp_eq_u32 s0, 0
                0xBF85_0002, // 4: s_cbranch_scc1 -> 16
                0xBE82_0380, // 8: s_mov_b32 s2, 0
                0xBF82_0002, // 12: s_branch -> 24
                0xD4C2_0002,
                0x0002_0080, // 16: v_cmp_eq_u32 s2, 0, v0
                0xD501_0001,
                cnd, // 24: v_cndmask_b32 v1, v2, v3, s2
                S_ENDPGM,
            ],
            false,
        );
        let s = structure(&p).unwrap();
        let phi = p
            .blocks
            .iter()
            .flat_map(|b| &b.phis)
            .find(|phi| phi.reg == Reg::S(2))
            .expect("mask phi for s2");
        assert_eq!(
            s.masks.get(&phi.value).and_then(|&m| s.bools.node(m)),
            Some((Atom::Phi(phi.value), Bool::FALSE, Bool::TRUE))
        );
        let edges = &s.phi_masks[&phi.value];
        assert_eq!(edges.len(), 2);
        assert!(edges.contains(&Bool::FALSE), "{edges:?}");
        assert!(
            edges
                .iter()
                .any(|&e| matches!(s.bools.support(e)[..], [Atom::Lane(_)])),
            "{edges:?}"
        );
    }

    #[test]
    fn loop_mask_accumulator_from_zero_tracks_phi_and_edges() {
        let cnd = 258 | (259 << 9) | (2 << 18);
        let p = build(
            &[
                0xBE82_0480, // 0: s_mov_b64 s[2:3], 0
                0xBE84_0380, // 4: s_mov_b32 s4, 0
                0xD501_0001,
                cnd,         // 8: v_cndmask_b32 v1, v2, v3, s[2:3]
                0x7D84_0080, // 16: v_cmp_eq_u32 vcc, 0, v0
                0x8882_026A, // 20: s_or_b64 s[2:3], vcc, s[2:3]
                0x8004_8104, // 24: s_add_u32 s4, s4, 1
                0xBF0A_8404, // 28: s_cmp_lt_u32 s4, 4
                0xBF85_FFF9, // 32: s_cbranch_scc1 -> 8
                0xD501_0004,
                cnd, // 36: v_cndmask_b32 v4, v2, v3, s[2:3]
                S_ENDPGM,
            ],
            true,
        );
        let s = structure(&p).unwrap();
        let phi = p
            .blocks
            .iter()
            .flat_map(|b| &b.phis)
            .find(|phi| phi.reg == Reg::S(2))
            .expect("loop mask phi for s[2:3]");
        assert!(s.masks.contains_key(&phi.value));
        let edges = &s.phi_masks[&phi.value];
        assert_eq!(edges.len(), 2);
        assert!(edges.contains(&Bool::FALSE), "{edges:?}");
        assert!(edges.iter().any(|&e| {
            let sup = s.bools.support(e);
            sup.contains(&Atom::Phi(phi.value)) && sup.iter().any(|a| matches!(a, Atom::Lane(_)))
        }));
    }

    #[test]
    fn loop_mask_phi_reset_to_constant_on_back_edge() {
        let cnd = 258 | (259 << 9) | (2 << 18);
        let p = build(
            &[
                0xD4C2_0002,
                0x0002_0080, // 0: v_cmp_eq_u32 s2, 0, v0
                0xBE84_0380, // 8: s_mov_b32 s4, 0
                0xD501_0001,
                cnd,         // 12: v_cndmask_b32 v1, v2, v3, s2
                0xBE82_0380, // 20: s_mov_b32 s2, 0
                0x8004_8104, // 24: s_add_u32 s4, s4, 1
                0xBF0A_8404, // 28: s_cmp_lt_u32 s4, 4
                0xBF85_FFFA, // 32: s_cbranch_scc1 -> 12
                S_ENDPGM,
            ],
            false,
        );
        let s = structure(&p).unwrap();
        let phi = p
            .blocks
            .iter()
            .flat_map(|b| &b.phis)
            .find(|phi| phi.reg == Reg::S(2))
            .expect("loop mask phi for s2");
        assert!(s.masks.contains_key(&phi.value));
        let edges = &s.phi_masks[&phi.value];
        assert_eq!(edges.len(), 2);
        assert!(edges.contains(&Bool::FALSE), "{edges:?}");
        assert!(
            edges
                .iter()
                .any(|&e| matches!(s.bools.support(e)[..], [Atom::Lane(_)])),
            "{edges:?}"
        );
    }
}
