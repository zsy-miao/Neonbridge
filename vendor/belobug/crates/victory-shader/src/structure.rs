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
type Set<K> = HashSet<K, BuildHasherDefault<Fx>>;

/// A set of what a walk has met: a list scan while it is small (most lane
/// masks are a handful of nodes), a hash set once it grows.
struct Seen<T> {
    list: Vec<T>,
    set: Set<T>,
}

impl<T> Default for Seen<T> {
    fn default() -> Self {
        Seen {
            list: Vec::new(),
            set: Set::default(),
        }
    }
}

impl<T: Copy + Eq + std::hash::Hash> Seen<T> {
    const SCAN: usize = 32;

    /// Adds `x`; false if it was already in.
    fn insert(&mut self, x: T) -> bool {
        if self.set.is_empty() {
            if self.list.contains(&x) {
                return false;
            }
            if self.list.len() < Self::SCAN {
                self.list.push(x);
                return true;
            }
            self.set.extend(self.list.drain(..));
        }
        self.set.insert(x)
    }
}

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
/// Cap the `ite` memo table so complex shaders cannot trigger multi-tens-of-MB
/// HashMap resizes (e.g. 35 MB at 2^21 buckets) under concurrent compilation.
const MAX_MEMO: usize = 1 << 18;

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

    /// The variables `b` tests, in the order a depth-first walk meets them.
    /// Each node is visited once: scanning lists of every node and variable
    /// seen made this quadratic in the size of `b`.
    fn vars(&self, b: Bool) -> Vec<u32> {
        let (mut seen, mut found) = (Seen::default(), Seen::default());
        let (mut vars, mut stack) = (Vec::new(), vec![b]);
        while let Some(b) = stack.pop() {
            let (v, lo, hi) = self.nodes[b.0 as usize];
            if v == TERMINAL || !seen.insert(b) {
                continue;
            }
            if found.insert(v) {
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
        if self.overflow {
            return Bool::FALSE;
        }
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
        if !self.overflow {
            if self.memo.len() >= MAX_MEMO {
                self.memo.clear();
            }
            self.memo.insert((f, g, h), r);
        }
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

    fn implies(&mut self, a: Bool, b: Bool) -> bool {
        let nb = self.not(b);
        self.and(a, nb) == Bool::FALSE
    }

    /// `b` with every atom born at reverse-postorder index `from` or later
    /// existentially quantified: what still holds once those are redefined.
    fn forget(&mut self, b: Bool, from: usize, memo: &mut Map<Bool, Bool>) -> Bool {
        if self.overflow {
            return Bool::FALSE;
        }
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

    /// `b` with every variable in `values` (sorted by variable) replaced by
    /// its value. Nothing below a node testing a later variable than the
    /// last of them changes.
    fn restrict(&mut self, b: Bool, values: &[(u32, bool)], memo: &mut Map<Bool, Bool>) -> Bool {
        let (v, lo, hi) = self.nodes[b.0 as usize];
        if v == TERMINAL || values.last().is_none_or(|&(last, _)| v > last) {
            return b;
        }
        if let Some(&r) = memo.get(&b) {
            return r;
        }
        let decided = values.binary_search_by_key(&v, |&(v, _)| v).ok();
        let r = match decided {
            Some(i) => self.restrict(if values[i].1 { hi } else { lo }, values, memo),
            None => {
                let l = self.restrict(lo, values, memo);
                let h = self.restrict(hi, values, memo);
                self.mk(v, l, h)
            }
        };
        // While only decided nodes were walked the walk is one path, which
        // meets no node twice: the table (and its allocation) can wait.
        if decided.is_none() || !memo.is_empty() {
            memo.insert(b, r);
        }
        r
    }

    /// `b` with each atom that `known` decides replaced by its value. The
    /// atoms are replaced together, in one walk of `b`: cofactors for
    /// different variables commute, and a walk per atom made this cost the
    /// size of `b` times the number of atoms.
    fn given(&mut self, b: Bool, known: Bool) -> Bool {
        let mut values = Vec::new();
        for v in self.vars(b) {
            let x = self.mk(v, Bool::FALSE, Bool::TRUE);
            let nx = self.not(x);
            if self.implies(known, x) {
                values.push((v, true));
            } else if self.implies(known, nx) {
                values.push((v, false));
            }
        }
        if values.is_empty() {
            return b;
        }
        values.sort_unstable();
        self.restrict(b, &values, &mut Map::default())
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
    /// The lane's bit of each lane-mask value, by its low dword's value.
    pub masks: HashMap<Value, Bool>,
    /// Symbolic select conditions, keyed by the destination SSA value.
    /// A wave64 condition belongs to the exact pair read by the select.
    pub(crate) select_masks: HashMap<Value, Bool>,
    /// Wave64 masks: low dword's value by the high dword's.
    pub highs: HashMap<Value, Value>,
    /// Masks (low dwords) set only in lanes whose EXEC bit is set where
    /// they are written: every such lane reaches the writing block, so a
    /// ballot there gives the mask as data.
    pub active: HashSet<Value>,
    /// Per block and instruction: the lane's EXEC bit at each vector
    /// instruction, whatever path the lane took (`None` for scalar ones).
    /// What a program run as the console runs it, every lane through every
    /// block, guards its vector instructions with ([`structure_console`]).
    pub execs: Vec<Vec<Option<Bool>>>,
    /// The lane's bit of values read as masks that derive from neither
    /// EXEC nor a VALU result (constants, uniform selects, opaque ones), by
    /// low dword. What a mask join takes on an edge such a value arrives by:
    /// a loop's exit mask starts as a constant 0 before lanes are added.
    pub others: HashMap<Value, Bool>,
}

/// Builds the per-lane form of `program`.
pub fn structure(program: &Program) -> Result<Structured, ShaderError> {
    run(Lanes::new(program, false)?)
}

/// The same for a program run as the console runs it: every lane of the
/// console wave through every block, the branches taken wave-wide. Masks
/// joined at a block are never resolved from what the lanes arriving there
/// know, since lanes outside EXEC arrive too: each join that differs
/// between edges is a phi ([`Atom::Phi`]). Guard vector instructions with
/// [`Structured::execs`] and ignore [`Structured::exits`]' conditions.
pub fn structure_console(program: &Program) -> Result<Structured, ShaderError> {
    run(Lanes::new(program, true)?)
}

fn run(mut l: Lanes) -> Result<Structured, ShaderError> {
    loop {
        let execs = l.masks_in_order();
        let cond = l.paths()?;
        let wrong = l.wrong_joins(&cond);
        if wrong.is_empty() {
            return l.finish(&execs, &cond);
        }
        l.unresolved.extend(wrong);
        l.reset();
    }
}

/// A lane mask by the values of its low and high dword (both the low one in
/// wave32).
type Key = (Value, Value);

struct Lanes<'a> {
    p: &'a Program,
    /// See [`structure_console`].
    console: bool,
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
    b: Bools,
    /// Masks derived from EXEC and VALU results.
    masks: Map<Key, Bool>,
    /// Other values read as masks: constants, uniform selects, opaque ones.
    other: Map<Key, Bool>,
    select_masks: HashMap<Value, Bool>,
    /// For branches: SCC of a mask operation ("some lane is set") as the
    /// lane's own bit.
    scc_lane: Map<Value, Bool>,
    /// Joins resolved to a constant, to check once all edges are known.
    joins: Vec<Join>,
    /// Per block: what a lane knows on entry, over forward edges only.
    first: Vec<Bool>,
    /// Per block ending in a branch: when a lane takes it.
    when: Vec<Option<Bool>>,
    /// See [`Structured::active`].
    active: HashSet<Value>,
}

/// A mask join taken as `value` in every lane that reaches `block`: each
/// incoming mask held (or failed) on its edge.
struct Join {
    block: usize,
    /// The phi value naming the join.
    id: Value,
    value: bool,
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
    fn new(p: &'a Program, console: bool) -> Result<Self, ShaderError> {
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
            console,
            order,
            index,
            incoming,
            defs,
            phis,
            unresolved: HashSet::new(),
            b: Bools::new(),
            masks: Map::default(),
            other: Map::default(),
            select_masks: HashMap::new(),
            scc_lane: Map::default(),
            joins: Vec::new(),
            first: vec![Bool::FALSE; blocks.len()],
            when: vec![None; blocks.len()],
            active: HashSet::new(),
        })
    }

    fn reset(&mut self) {
        self.b = Bools::new();
        self.masks.clear();
        self.other.clear();
        self.select_masks.clear();
        self.scc_lane.clear();
        self.joins.clear();
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
            }
        }
        execs
    }

    /// `c` narrowed by the branch at the end of `q` towards one successor.
    fn edge(&mut self, c: Bool, q: usize, taken: bool) -> Bool {
        match self.when[q] {
            Some(w) if taken => self.b.and(c, w),
            Some(w) => {
                let nw = self.b.not(w);
                self.b.and(c, nw)
            }
            None => c,
        }
    }

    /// Joins whose constant fails on some edge, given what lanes know.
    fn wrong_joins(&mut self, cond: &[Bool]) -> Vec<Value> {
        let mut wrong = Vec::new();
        let mut j = 0;
        // Checking an argument can resolve another join; it is checked too.
        while j < self.joins.len() {
            let (block, id, value) = (self.joins[j].block, self.joins[j].id, self.joins[j].value);
            for (n, a) in self.joins[j].args.clone().into_iter().enumerate() {
                let (q, taken) = self.incoming[block][n];
                let e = self.edge(cond[q], q, taken);
                let mut m = self.mask(a).0;
                if !value {
                    m = self.b.not(m);
                }
                if !self.b.implies(e, m) {
                    wrong.push(id);
                    break;
                }
            }
            j += 1;
        }
        wrong
    }

    fn finish(
        mut self,
        execs: &[Vec<Option<Bool>>],
        cond: &[Bool],
    ) -> Result<Structured, ShaderError> {
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
        Ok(Structured {
            masks: self.masks.iter().map(|(&(lo, _), &m)| (lo, m)).collect(),
            highs: self
                .masks
                .keys()
                .filter(|(lo, hi)| lo != hi)
                .map(|&(lo, hi)| (hi, lo))
                .collect(),
            select_masks: self.select_masks,
            active: self.active,
            bools: self.b,
            exits,
            guards,
            execs: execs.to_vec(),
            others: self.other.iter().map(|(&(lo, _), &m)| (lo, m)).collect(),
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
        let mut derived = false;
        // (edge condition, mask) per forward edge; back edges come later.
        let mut forward = Vec::new();
        for (n, &a) in args.iter().enumerate() {
            let (q, taken) = self.incoming[block][n];
            if self.index[q] < self.index[block] {
                let (m, d) = self.mask(a);
                derived |= d;
                let e = self.edge(self.first[q], q, taken);
                forward.push((e, m));
            }
        }
        if forward.len() == args.len()
            && let Some(&(_, m)) = forward.first()
            && forward.iter().all(|f| f.1 == m)
        {
            return Some((m, derived));
        }
        if !self.console && !self.unresolved.contains(&id) {
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
                        args,
                    });
                    let m = if value { Bool::TRUE } else { Bool::FALSE };
                    return Some((m, derived));
                }
            }
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
                let born = self.born(scc);
                let u = self.b.atom(Atom::Uniform(scc), born);
                (self.b.ite(u, a, c), None)
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
                // results is resolved here, as for EXEC.
                if name == "v_cndmask_b32"
                    && let Some(&Operand::Scalar { code, .. }) = v.src.get(2)
                    && code != NULL
                    && let Some(k) = self.key(|r| inst.read(r), code)
                    && let Some(&(_, dst)) = inst.writes.first()
                {
                    let (mask, derived) = self.mask(k);
                    if derived {
                        self.select_masks.insert(dst, mask);
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

    /// [`Bools::vars`] as it was: lists of the nodes and variables seen.
    fn vars_by_scan(b: &Bools, root: Bool) -> Vec<u32> {
        let (mut seen, mut vars, mut stack) = (Vec::new(), Vec::new(), vec![root]);
        while let Some(n) = stack.pop() {
            let (v, lo, hi) = b.nodes[n.0 as usize];
            if v == TERMINAL || seen.contains(&n) {
                continue;
            }
            seen.push(n);
            if !vars.contains(&v) {
                vars.push(v);
            }
            stack.extend([lo, hi]);
        }
        vars
    }

    /// One variable's cofactor, as `restrict` did before it took several.
    fn restrict_one(
        b: &mut Bools,
        f: Bool,
        v: u32,
        value: bool,
        memo: &mut Map<Bool, Bool>,
    ) -> Bool {
        let (bv, lo, hi) = b.nodes[f.0 as usize];
        if bv == TERMINAL || bv > v {
            return f;
        }
        if bv == v {
            return if value { hi } else { lo };
        }
        if let Some(&r) = memo.get(&f) {
            return r;
        }
        let l = restrict_one(b, lo, v, value, memo);
        let h = restrict_one(b, hi, v, value, memo);
        let r = b.mk(bv, l, h);
        memo.insert(f, r);
        r
    }

    /// [`Bools::given`] as it was: a walk of `f` per decided variable.
    fn given_by_walks(b: &mut Bools, mut f: Bool, known: Bool) -> Bool {
        for v in vars_by_scan(b, f) {
            let x = b.mk(v, Bool::FALSE, Bool::TRUE);
            let nx = b.not(x);
            if b.implies(known, x) {
                f = restrict_one(b, f, v, true, &mut Map::default());
            } else if b.implies(known, nx) {
                f = restrict_one(b, f, v, false, &mut Map::default());
            }
        }
        f
    }

    /// Atoms `0..n` of a fresh [`Bools`], in variable order.
    fn atoms(n: u64) -> (Bools, Vec<Bool>) {
        let mut b = Bools::new();
        let xs = (0..n).map(|i| b.atom(Atom::Constant(i), 0)).collect();
        (b, xs)
    }

    /// `x[i] == y[i]` for every `i`, with every `x` ordered before every
    /// `y`: the classic order that makes the diagram exponential in `n`.
    fn pairs_equal(b: &mut Bools, xs: &[Bool], n: usize) -> Bool {
        let mut f = Bool::TRUE;
        for i in 0..n {
            let ne = b.xor(xs[i], xs[n + i]);
            let eq = b.not(ne);
            f = b.and(f, eq);
        }
        f
    }

    /// A deterministic mix of ands, ors and xors over `xs`.
    fn mixed(b: &mut Bools, xs: &[Bool], seed: u64, steps: usize) -> Bool {
        let mut s = seed;
        let mut next = || {
            s = s
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (s >> 33) as usize
        };
        let mut f = xs[next() % xs.len()];
        for _ in 0..steps {
            let x = xs[next() % xs.len()];
            let x = if next() % 2 == 0 { b.not(x) } else { x };
            f = match next() % 3 {
                0 => b.and(f, x),
                1 => b.or(f, x),
                _ => b.xor(f, x),
            };
        }
        f
    }

    /// The function of `f` on the assignment `bits` (bit `v`: variable `v`).
    fn value_at(b: &Bools, mut f: Bool, bits: u64) -> bool {
        loop {
            match f {
                Bool::FALSE => return false,
                Bool::TRUE => return true,
                _ => {
                    let (v, lo, hi) = b.nodes[f.0 as usize];
                    f = if bits >> v & 1 == 1 { hi } else { lo };
                }
            }
        }
    }

    #[test]
    fn vars_match_the_scan() {
        let (mut b, xs) = atoms(16);
        let mut fs = vec![Bool::FALSE, Bool::TRUE, xs[3], pairs_equal(&mut b, &xs, 8)];
        for seed in 0..40 {
            fs.push(mixed(&mut b, &xs, seed, 30));
        }
        for f in fs {
            assert_eq!(b.vars(f), vars_by_scan(&b, f));
        }
    }

    #[test]
    fn given_matches_a_walk_per_variable() {
        let (mut b, xs) = atoms(12);
        for seed in 0..60 {
            let f = mixed(&mut b, &xs, seed, 24);
            let known = mixed(&mut b, &xs, seed ^ 0x9e37, 3);
            let fast = b.given(f, known);
            // Equal functions are equal nodes, so the ids match too.
            assert_eq!(fast, given_by_walks(&mut b, f, known), "seed {seed}");
            // Where `known` holds, the result agrees with `f`.
            for bits in 0..1u64 << 12 {
                if value_at(&b, known, bits) {
                    assert_eq!(value_at(&b, fast, bits), value_at(&b, f, bits));
                }
            }
        }
    }

    #[test]
    fn given_replaces_every_decided_atom() {
        let (mut b, xs) = atoms(12);
        let f = pairs_equal(&mut b, &xs, 6);
        // x0..x5 all true: f becomes y0 && .. && y5.
        let mut known = Bool::TRUE;
        let mut ys = Bool::TRUE;
        for i in 0..6 {
            known = b.and(known, xs[i]);
            ys = b.and(ys, xs[6 + i]);
        }
        assert_eq!(b.given(f, known), ys);
        // Nothing decided: unchanged; a contradiction decides nothing useful
        // but must not panic.
        assert_eq!(b.given(f, Bool::TRUE), f);
        let _ = b.given(f, Bool::FALSE);
        assert_eq!(b.given(Bool::TRUE, known), Bool::TRUE);
    }

    #[test]
    #[ignore = "benchmark: cargo test --release -p victory-shader bench_bools -- --ignored --nocapture"]
    fn bench_bools() {
        // Microseconds per call, over at least 3 calls and 50 ms.
        let time = |f: &mut dyn FnMut()| {
            let (t, mut calls) = (std::time::Instant::now(), 0);
            while calls < 3 || t.elapsed().as_millis() < 50 {
                f();
                calls += 1;
            }
            t.elapsed().as_secs_f64() * 1e6 / f64::from(calls)
        };
        for n in [2, 4, 8, 10, 12, 13] {
            let (mut b, xs) = atoms(2 * n as u64);
            let f = pairs_equal(&mut b, &xs, n);
            let count = vars_by_scan(&b, f).len();
            let size = {
                let (mut seen, mut stack) = (Set::default(), vec![f]);
                while let Some(g) = stack.pop() {
                    let (v, lo, hi) = b.nodes[g.0 as usize];
                    if v != TERMINAL && seen.insert(g) {
                        stack.extend([lo, hi]);
                    }
                }
                seen.len()
            };
            // Half the x's known true.
            let mut known = Bool::TRUE;
            for x in &xs[..n / 2] {
                known = b.and(known, *x);
            }
            let (expect, mut got) = (given_by_walks(&mut b, f, known), Bool::FALSE);
            let vars_old = time(&mut || assert_eq!(vars_by_scan(&b, f).len(), count));
            let vars_new = time(&mut || assert_eq!(b.vars(f).len(), count));
            let given_old = time(&mut || got = given_by_walks(&mut b, f, known));
            assert_eq!(got, expect);
            let given_new = time(&mut || got = b.given(f, known));
            assert_eq!(got, expect);
            println!(
                "n={n} nodes={size}: vars {vars_old:.2} -> {vars_new:.2} us ({:.1}x), \
                 given {given_old:.2} -> {given_new:.2} us ({:.1}x)",
                vars_old / vars_new,
                given_old / given_new
            );
        }
    }
}
