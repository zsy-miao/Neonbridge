//! One-to-one SSA form of a native program (decision 0003, step 1).
//!
//! Every machine instruction becomes one [`Inst`], except scheduling-only
//! ones (`s_nop`, `s_waitcnt*`, `s_inst_prefetch`...), which are dropped.
//! Each register dword is a variable: a read names the [`Value`] it sees, a
//! write defines a new one, and [`Phi`]s merge values where control flow
//! joins. Nothing is raised: an instruction keeps its opcode and decoded
//! fields, so its exact semantics stay with it.
//!
//! Implicit operands are explicit: SCC, M0, and EXEC, which every vector
//! instruction reads. A write to a VGPR also reads the register's old value,
//! because lanes disabled in EXEC keep it.
//!
//! M0-relative VGPR access (`v_movrels/d/sd_b32`, `v_swaprel_b32`) names its
//! operand's whole possible range: from the encoded register up to the
//! program's VGPR count, all read (and all written, for a destination).

use crate::ShaderError;
use crate::gfx10::{self, Fields, Flow, Instruction, Operand};

/// One register dword. Scalar codes as in [`Operand::Scalar`]: 0-105 SGPRs,
/// 106/107 VCC, 108-123 trap temporaries, 124 M0, 126/127 EXEC, 128 and up
/// SGPRs spilled to VGPR lanes (see `unspill`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Reg {
    S(u8),
    Scc,
    V(u8),
}

pub const VCC: u8 = 106;
pub const M0: u8 = 124;
pub const EXEC: u8 = 126;

/// An SSA value: the content of one register dword (SCC: one bit).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Value(pub u32);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    pub wave64: bool,
    /// VGPRs v0 up to this exist; bounds M0-relative access.
    pub vgprs: u16,
    /// In code order, then copies made to give every loop one entry. Block 0
    /// is the entry, which can be an empty block before the code.
    pub blocks: Vec<Block>,
    /// Registers read before any write, with their value on entry: what the
    /// hardware or a called fetch routine set up (user SGPRs, system values,
    /// vertex inputs).
    pub inputs: Vec<(Reg, Value)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    /// Byte offset of the block's first machine instruction (shared by a
    /// block and its copies). Zero for an empty entry block.
    pub offset: usize,
    pub preds: Vec<usize>,
    pub phis: Vec<Phi>,
    pub insts: Vec<Inst>,
    pub exit: Exit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Jump(usize),
    /// `s_cbranch_*`, the block's last instruction.
    Branch {
        taken: usize,
        not_taken: usize,
    },
    /// `s_endpgm` and its variants.
    End,
    /// `s_setpc_b64` to an address held in registers, e.g. the handover to
    /// the next half of a merged hardware stage.
    Indirect,
}

impl Exit {
    pub fn successors(&self) -> Vec<usize> {
        match *self {
            Exit::Jump(b) => vec![b],
            Exit::Branch { taken, not_taken } => vec![taken, not_taken],
            Exit::End | Exit::Indirect => vec![],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phi {
    pub reg: Reg,
    pub value: Value,
    /// One per predecessor, in [`Block::preds`] order.
    pub args: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inst {
    pub machine: Instruction,
    pub fields: Fields,
    /// Every register dword read, with the value it holds.
    pub reads: Vec<(Reg, Value)>,
    /// Every register dword written, with the value it receives.
    pub writes: Vec<(Reg, Value)>,
}

impl Inst {
    pub fn name(&self) -> &'static str {
        self.machine.name().unwrap_or("")
    }

    /// The value `reg` holds when this instruction reads it.
    pub fn read(&self, reg: Reg) -> Option<Value> {
        self.reads.iter().find(|r| r.0 == reg).map(|r| r.1)
    }

    /// The value this instruction writes to `reg`.
    pub fn write(&self, reg: Reg) -> Option<Value> {
        self.writes.iter().find(|w| w.0 == reg).map(|w| w.1)
    }
}

/// Decodes `code` and builds its SSA form; `vgprs` is the program's VGPR
/// count. Fails on instructions the IR cannot express yet
/// ([`ShaderError::Unsupported`]).
pub fn build(code: &[u8], wave64: bool, vgprs: u16) -> Result<Program, ShaderError> {
    let list = gfx10::decode(code)?;
    let lanes = 1 + u8::from(wave64);
    let isa = |offset, reason| ShaderError::Isa { offset, reason };

    let mut flows = Vec::with_capacity(list.len());
    let mut leaders = vec![0];
    for (i, inst) in list.iter().enumerate() {
        let flow = gfx10::control(inst, code.len(), |at| {
            list[..list.partition_point(|i| i.offset < at)].last()
        })?;
        if let Flow::Jump(t) | Flow::Branch(t) = flow {
            leaders.push(t);
        }
        if flow != Flow::Next
            && let Some(next) = list.get(i + 1)
        {
            leaders.push(next.offset);
        }
        flows.push(flow);
    }
    leaders.sort_unstable();
    leaders.dedup();
    // When the first instruction is a loop header, give SSA a separate
    // entry edge to carry the initial register values into its phis.
    let preheader = usize::from(
        flows
            .iter()
            .any(|flow| matches!(flow, Flow::Jump(0) | Flow::Branch(0))),
    );
    let block_at = |offset: usize| {
        leaders
            .binary_search(&offset)
            .map(|b| b + preheader)
            .map_err(|_| isa(offset, "jump into a block"))
    };

    let mut blocks: Vec<Block> = leaders
        .iter()
        .map(|&offset| Block {
            offset,
            preds: Vec::new(),
            phis: Vec::new(),
            insts: Vec::new(),
            exit: Exit::End,
        })
        .collect();
    if preheader != 0 {
        blocks.insert(
            0,
            Block {
                offset: 0,
                preds: Vec::new(),
                phis: Vec::new(),
                insts: Vec::new(),
                exit: Exit::Jump(1),
            },
        );
    }
    let next_offsets: Vec<Option<usize>> = (0..list.len())
        .map(|i| list.get(i + 1).map(|n| n.offset))
        .collect();
    for ((machine, flow), next) in list.into_iter().zip(flows).zip(next_offsets) {
        let b = leaders.partition_point(|&l| l <= machine.offset) - 1 + preheader;
        let name = machine.name().ok_or(ShaderError::Unsupported {
            offset: machine.offset,
            what: "unknown opcode",
        })?;
        let fields = machine.fields(wave64);
        let fallthrough = || block_at(next.ok_or(isa(machine.offset, "falls off the code"))?);
        let ends = flow != Flow::Next || next.is_none_or(|n| leaders.binary_search(&n).is_ok());
        if ends {
            blocks[b].exit = match flow {
                Flow::Next => Exit::Jump(fallthrough()?),
                Flow::Jump(t) => Exit::Jump(block_at(t)?),
                Flow::Branch(t) => Exit::Branch {
                    taken: block_at(t)?,
                    not_taken: fallthrough()?,
                },
                Flow::Stop if name == "s_setpc_b64" => Exit::Indirect,
                Flow::Stop => Exit::End,
            };
        }
        if let Some((reads, writes)) = effects(&machine, &fields, name, lanes, vgprs)? {
            let unset = |regs: Vec<Reg>| regs.into_iter().map(|r| (r, NONE)).collect();
            blocks[b].insts.push(Inst {
                machine,
                fields,
                reads: unset(reads),
                writes: unset(writes),
            });
        }
    }
    if !wave64 {
        unspill(&mut blocks);
    }
    link(&mut blocks);
    split_irreducible(&mut blocks)?;

    Ok(rename(blocks, wave64, vgprs))
}

/// Rename register reads and writes after changing edges in a reducible graph.
fn rename(mut blocks: Vec<Block>, wave64: bool, vgprs: u16) -> Program {
    let order = reverse_postorder(&blocks);
    let (mut live_in, mut live_out) = liveness(&blocks, &order);
    // A spilled SGPR that some path reads before any write is zero there.
    let unset: Vec<Inst> = members(&live_in[0])
        .filter_map(|reg| match reg {
            Reg::S(c) if SPILLS.contains(&c) => Some(zero(c, blocks[0].offset)),
            _ => None,
        })
        .collect();
    if !unset.is_empty() {
        blocks[0].insts.splice(0..0, unset);
        (live_in, live_out) = liveness(&blocks, &order);
    }
    let mut values = 0u32;
    let mut new_value = || {
        values += 1;
        Value(values - 1)
    };
    let mut inputs = Vec::new();
    let mut outs: Vec<Vec<(Reg, Value)>> = vec![Vec::new(); blocks.len()];
    let mut current = [NONE; REGS];
    for &b in &order {
        current.fill(NONE);
        let block = &mut blocks[b];
        for reg in members(&live_in[b]) {
            current[index(reg)] = match block.preds[..] {
                [] => {
                    let v = new_value();
                    inputs.push((reg, v));
                    v
                }
                // A single predecessor comes earlier in reverse postorder.
                [p] => lookup(&outs[p], reg),
                _ => {
                    let value = new_value();
                    block.phis.push(Phi {
                        reg,
                        value,
                        args: Vec::new(),
                    });
                    value
                }
            };
        }
        for inst in &mut block.insts {
            for (reg, v) in &mut inst.reads {
                *v = current[index(*reg)];
                debug_assert_ne!(*v, NONE, "{reg:?} read at {:#x}", inst.machine.offset);
            }
            for (reg, v) in &mut inst.writes {
                *v = new_value();
                current[index(*reg)] = *v;
            }
        }
        outs[b] = members(&live_out[b])
            .map(|reg| (reg, current[index(reg)]))
            .collect();
    }
    for block in &mut blocks {
        for phi in &mut block.phis {
            phi.args = block
                .preds
                .iter()
                .map(|&p| lookup(&outs[p], phi.reg))
                .collect();
            debug_assert!(!phi.args.contains(&NONE), "{phi:?}");
        }
    }
    remove_trivial_phis(&mut blocks, values);
    Program {
        wave64,
        vgprs,
        blocks,
        inputs,
    }
}

/// Duplicate a join for each proven (predecessor, join, destination) edge.
/// The instructions still execute on that path and define fresh SSA values.
pub(crate) fn thread_edges(
    p: &Program,
    edges: &[(usize, usize, usize)],
) -> Result<Program, ShaderError> {
    let mut blocks = p.blocks.clone();
    for block in &mut blocks {
        block.phis.clear();
    }
    for &(pred, join, dest) in edges {
        let mut copy = blocks[join].clone();
        copy.exit = Exit::Jump(dest);
        let target = blocks.len();
        match &mut blocks[pred].exit {
            Exit::Jump(t) => *t = target,
            Exit::Branch { taken, not_taken } => {
                if *taken == join {
                    *taken = target;
                }
                if *not_taken == join {
                    *not_taken = target;
                }
            }
            _ => unreachable!("threading an incoming edge"),
        }
        blocks.push(copy);
    }
    link(&mut blocks);
    split_irreducible(&mut blocks)?;
    Ok(rename(blocks, p.wave64, p.vgprs))
}

/// Placeholder before renaming.
const NONE: Value = Value(u32::MAX);
/// SGPR codes 0-255, SCC, VGPRs 0-255.
const REGS: usize = 256 + 1 + 256;
type RegSet = [u64; REGS.div_ceil(64)];

fn index(reg: Reg) -> usize {
    match reg {
        Reg::S(c) => usize::from(c),
        Reg::Scc => 256,
        Reg::V(r) => 257 + usize::from(r),
    }
}

fn reg_at(i: usize) -> Reg {
    match i {
        0..256 => Reg::S(i as u8),
        256 => Reg::Scc,
        _ => Reg::V((i - 257) as u8),
    }
}

fn insert(set: &mut RegSet, reg: Reg) {
    let i = index(reg);
    set[i / 64] |= 1 << (i % 64);
}

fn contains(set: &RegSet, reg: Reg) -> bool {
    let i = index(reg);
    set[i / 64] >> (i % 64) & 1 != 0
}

/// In [`Reg`] order, which is also index order.
fn members(set: &RegSet) -> impl Iterator<Item = Reg> + '_ {
    (0..REGS)
        .filter(|&i| set[i / 64] >> (i % 64) & 1 != 0)
        .map(reg_at)
}

/// Scalar codes of spilled SGPRs.
const SPILLS: std::ops::Range<u8> = 128..254;

/// SOP1 opcode.
const S_MOV_B32: u16 = 3;

/// The VGPR and lane of a `v_writelane_b32` (`true`) or `v_readlane_b32`
/// (`false`) of a constant lane.
fn spill(inst: &Inst) -> Option<(u8, u32, bool)> {
    let Fields::Valu(v) = &inst.fields else {
        return None;
    };
    let write = match inst.name() {
        "v_writelane_b32" => true,
        "v_readlane_b32" => false,
        _ => return None,
    };
    let vector = if write { v.dst.as_ref() } else { v.src.first() };
    let (Some(&Operand::Vector { reg, .. }), Some(&Operand::Const(c @ 128..=159))) =
        (vector, v.src.get(1))
    else {
        return None;
    };
    Some((reg, u32::from(c) - 128, write))
}

/// Compilers out of SGPRs keep some in lanes of VGPRs they use for nothing
/// else. Each such lane becomes a scalar register of its own and its reads
/// and writes become `s_mov_b32`, so that what it holds (a lane mask, say)
/// is followed like any scalar. 32-lane waves only: 64-lane masks are pairs,
/// moved together.
fn unspill(blocks: &mut [Block]) {
    let mut used = [false; 256];
    for inst in blocks.iter().flat_map(|b| &b.insts) {
        // Of a lane access, the spilled register itself is no other use.
        let own = spill(inst).map(|s| Reg::V(s.0));
        // An M0-relative read (`v_movrels_b32`) names every VGPR from its base
        // up; the compiler indexes a small array with it, never the lanes it
        // parks scalars in, so it does not make those VGPRs ordinary ones.
        // (Relative writes still do.)
        let relative_read = inst.name() == "v_movrels_b32";
        let n_reads = inst.reads.len();
        for (k, &(reg, _)) in inst.reads.iter().chain(&inst.writes).enumerate() {
            if relative_read && k < n_reads && matches!(reg, Reg::V(_)) {
                continue;
            }
            if let Reg::V(v) = reg
                && Some(reg) != own
            {
                used[usize::from(v)] = true;
            }
        }
    }
    let mut lanes: Vec<(u8, u32)> = Vec::new();
    for inst in blocks.iter().flat_map(|b| &b.insts) {
        if let Some((reg, lane, _)) = spill(inst)
            && !used[usize::from(reg)]
            && !lanes.contains(&(reg, lane))
        {
            lanes.push((reg, lane));
        }
    }
    // All lanes of a register or none.
    while lanes.len() > SPILLS.len() {
        let Some(&(reg, _)) = lanes.last() else { break };
        lanes.retain(|l| l.0 != reg);
    }
    for inst in blocks.iter_mut().flat_map(|b| &mut b.insts) {
        let Some((reg, lane, write)) = spill(inst) else {
            continue;
        };
        let Some(at) = lanes.iter().position(|&l| l == (reg, lane)) else {
            continue;
        };
        let Fields::Valu(v) = &inst.fields else {
            continue;
        };
        let slot = SPILLS.start + at as u8;
        let operand = Operand::Scalar { code: slot, dwords: 1 };
        let (dst, src) = if write {
            (Some(operand), v.src[0])
        } else {
            (v.dst, operand)
        };
        let slot = (Reg::S(slot), NONE);
        if write {
            inst.reads.clear();
            // Only fails out of bounds, which `effects` checked.
            let mut regs = Vec::new();
            let _ = add_regs(&src, 1, &mut regs);
            inst.reads.extend(regs.into_iter().map(|r| (r, NONE)));
            inst.writes = vec![slot];
        } else {
            inst.reads = vec![slot];
        }
        inst.machine.encoding = gfx10::Encoding::Sop1;
        inst.machine.opcode = S_MOV_B32;
        inst.fields = Fields::Sop {
            dst,
            src: vec![src],
            imm: None,
        };
    }
}

/// `s_mov_b32 code, 0` at `offset`.
fn zero(code: u8, offset: usize) -> Inst {
    Inst {
        machine: Instruction {
            offset,
            encoding: gfx10::Encoding::Sop1,
            opcode: S_MOV_B32,
            words: Vec::new(),
        },
        fields: Fields::Sop {
            dst: Some(Operand::Scalar { code, dwords: 1 }),
            src: vec![Operand::Const(128)],
            imm: None,
        },
        reads: Vec::new(),
        writes: vec![(Reg::S(code), NONE)],
    }
}

/// `outs` is sorted by register, and holds every register live out.
fn lookup(outs: &[(Reg, Value)], reg: Reg) -> Value {
    outs.binary_search_by_key(&reg, |&(r, _)| r)
        .map_or(NONE, |i| outs[i].1)
}

pub(crate) fn reverse_postorder(blocks: &[Block]) -> Vec<usize> {
    let mut seen = vec![false; blocks.len()];
    let mut post = Vec::with_capacity(blocks.len());
    let mut stack = vec![(0, 0)];
    seen[0] = true;
    while let Some(top) = stack.last_mut() {
        let (b, i) = *top;
        top.1 += 1;
        match blocks[b].exit.successors().get(i) {
            Some(&s) if !seen[s] => {
                seen[s] = true;
                stack.push((s, 0));
            }
            Some(_) => {}
            None => {
                post.push(b);
                stack.pop();
            }
        }
    }
    post.reverse();
    post
}

/// Recomputes every block's predecessors from the exits.
fn link(blocks: &mut [Block]) {
    for block in blocks.iter_mut() {
        block.preds.clear();
    }
    for b in 0..blocks.len() {
        for s in blocks[b].exit.successors() {
            blocks[s].preds.push(b);
        }
    }
}

/// An edge to an earlier block in reverse postorder whose target does not
/// dominate its source, if any: there the control flow is irreducible.
/// `index` is each block's position in `order`.
pub(crate) fn irreducible_edge(
    blocks: &[Block],
    order: &[usize],
    index: &[usize],
) -> Option<(usize, usize)> {
    let &entry = order.first()?;
    let idom = idoms(blocks, order, index);
    for &b in order {
        for s in blocks[b].exit.successors() {
            if index[s] > index[b] {
                continue;
            }
            let mut d = b;
            while d != s && d != entry {
                d = idom[d];
            }
            if d != s {
                return Some((b, s));
            }
        }
    }
    None
}

/// Each block's immediate dominator (the entry's is itself, unreachable
/// blocks' `usize::MAX`); `order` is reverse postorder, `index` each
/// block's position in it.
pub(crate) fn idoms(blocks: &[Block], order: &[usize], index: &[usize]) -> Vec<usize> {
    let mut idom = vec![usize::MAX; blocks.len()];
    let Some(&entry) = order.first() else {
        return idom;
    };
    idom[entry] = entry;
    let intersect = |idom: &[usize], mut a: usize, mut b: usize| {
        while a != b {
            while index[a] > index[b] {
                a = idom[a];
            }
            while index[b] > index[a] {
                b = idom[b];
            }
        }
        a
    };
    let mut changed = true;
    while changed {
        changed = false;
        for &b in &order[1..] {
            let mut new = usize::MAX;
            for &q in blocks[b].preds.iter().filter(|&&q| idom[q] != usize::MAX) {
                new = if new == usize::MAX {
                    q
                } else {
                    intersect(&idom, q, new)
                };
            }
            if idom[b] != new {
                idom[b] = new;
                changed = true;
            }
        }
    }
    idom
}

/// Node splitting, before SSA. While a loop is entered at several blocks
/// (a compiler-rotated loop whose peeled first test jumps into its body),
/// the entry first in reverse postorder stays the header, and another
/// entry's outside predecessors move to a copy of everything that entry
/// reaches in the loop before the header. The copies run the same
/// instructions, so behaviour is unchanged.
fn split_irreducible(blocks: &mut Vec<Block>) -> Result<(), ShaderError> {
    let limit = 4 * blocks.len() + 64;
    loop {
        let order = reverse_postorder(blocks);
        let mut index = vec![usize::MAX; blocks.len()];
        for (i, &b) in order.iter().enumerate() {
            index[b] = i;
        }
        let Some((from, _)) = irreducible_edge(blocks, &order, &index) else {
            return Ok(());
        };
        let found = multi_entry_loop(blocks, &vec![true; blocks.len()]);
        let Some((region, mut entries)) = found.filter(|_| blocks.len() < limit) else {
            return Err(ShaderError::Unsupported {
                offset: blocks[from].offset,
                what: "irreducible control flow",
            });
        };
        entries.sort_by_key(|&e| index[e]);
        let (header, entry) = (entries[0], entries[1]);

        // What `entry` reaches in the loop before the header, and its copies.
        let mut copy = vec![usize::MAX; blocks.len()];
        let mut copied = Vec::new();
        let mut stack = vec![entry];
        while let Some(b) = stack.pop() {
            if copy[b] != usize::MAX {
                continue;
            }
            copy[b] = blocks.len() + copied.len();
            copied.push(b);
            for s in blocks[b].exit.successors() {
                if region[s] && s != header {
                    stack.push(s);
                }
            }
        }
        let retarget = |exit: &mut Exit, to: &dyn Fn(usize) -> usize| match exit {
            Exit::Jump(s) => *s = to(*s),
            Exit::Branch { taken, not_taken } => {
                *taken = to(*taken);
                *not_taken = to(*not_taken);
            }
            Exit::End | Exit::Indirect => {}
        };
        for &b in &copied {
            let mut block = blocks[b].clone();
            retarget(&mut block.exit, &|s| {
                if copy[s] == usize::MAX { s } else { copy[s] }
            });
            blocks.push(block);
        }
        let outside: Vec<usize> = blocks[entry]
            .preds
            .iter()
            .copied()
            .filter(|&p| !region[p])
            .collect();
        for p in outside {
            retarget(&mut blocks[p].exit, &|s| {
                if s == entry { copy[entry] } else { s }
            });
        }
        link(blocks);
    }
}

/// A loop among the `inside` blocks entered at more than one block, with
/// its entries. Loops nested in a single-entry loop are searched with its
/// header removed, so one is found whenever the control flow is irreducible.
fn multi_entry_loop(blocks: &[Block], inside: &[bool]) -> Option<(Vec<bool>, Vec<usize>)> {
    let reach = |from: usize, forward: bool| {
        let mut seen = vec![false; blocks.len()];
        let mut stack = vec![from];
        while let Some(b) = stack.pop() {
            let next = if forward {
                blocks[b].exit.successors()
            } else {
                blocks[b].preds.clone()
            };
            for n in next {
                if inside[n] && !seen[n] {
                    seen[n] = true;
                    stack.push(n);
                }
            }
        }
        seen
    };
    let mut done = vec![false; blocks.len()];
    for v in (0..blocks.len()).filter(|&v| inside[v]) {
        if done[v] {
            continue;
        }
        // Blocks on a cycle through `v`; `v` itself only if there is one.
        let (ahead, behind) = (reach(v, true), reach(v, false));
        let region: Vec<bool> = ahead.iter().zip(&behind).map(|(a, b)| a & b).collect();
        if !region[v] {
            continue;
        }
        let members = (0..blocks.len()).filter(|&b| region[b]);
        for b in members.clone() {
            done[b] = true;
        }
        let entries: Vec<usize> = members
            .filter(|&b| blocks[b].preds.iter().any(|&p| !region[p]))
            .collect();
        match entries[..] {
            [] => {}
            [header] => {
                let mut nested = region;
                nested[header] = false;
                if let Some(found) = multi_entry_loop(blocks, &nested) {
                    return Some(found);
                }
            }
            _ => return Some((region, entries)),
        }
    }
    None
}

/// Registers live into and out of each block.
fn liveness(blocks: &[Block], order: &[usize]) -> (Vec<RegSet>, Vec<RegSet>) {
    let empty = [0; REGS.div_ceil(64)];
    let mut uses = vec![empty; blocks.len()];
    let mut defs = vec![empty; blocks.len()];
    for (b, block) in blocks.iter().enumerate() {
        for inst in &block.insts {
            for &(reg, _) in &inst.reads {
                if !contains(&defs[b], reg) {
                    insert(&mut uses[b], reg);
                }
            }
            for &(reg, _) in &inst.writes {
                insert(&mut defs[b], reg);
            }
        }
    }
    let mut live_in = vec![empty; blocks.len()];
    let mut live_out = vec![empty; blocks.len()];
    let mut changed = true;
    while changed {
        changed = false;
        for &b in order.iter().rev() {
            let mut out = empty;
            for s in blocks[b].exit.successors() {
                for (o, i) in out.iter_mut().zip(&live_in[s]) {
                    *o |= i;
                }
            }
            let mut inn = empty;
            for w in 0..inn.len() {
                inn[w] = uses[b][w] | out[w] & !defs[b][w];
            }
            if inn != live_in[b] || out != live_out[b] {
                (live_in[b], live_out[b]) = (inn, out);
                changed = true;
            }
        }
    }
    (live_in, live_out)
}

/// Phis are placed for every live register at every join; those whose
/// arguments are all one value (or the phi itself) are replaced by it.
fn remove_trivial_phis(blocks: &mut [Block], values: u32) {
    let mut replaced: Vec<Value> = (0..values).map(Value).collect();
    let find = |replaced: &[Value], mut v: Value| {
        while replaced[v.0 as usize] != v {
            v = replaced[v.0 as usize];
        }
        v
    };
    let mut changed = true;
    while changed {
        changed = false;
        for block in blocks.iter_mut() {
            block.phis.retain(|phi| {
                let mut only = None;
                for &a in &phi.args {
                    let a = find(&replaced, a);
                    if a == phi.value || Some(a) == only {
                        continue;
                    }
                    if only.is_some() {
                        return true;
                    }
                    only = Some(a);
                }
                let Some(v) = only else { return true };
                replaced[phi.value.0 as usize] = v;
                changed = true;
                false
            });
        }
    }
    for block in blocks {
        for phi in &mut block.phis {
            for a in &mut phi.args {
                *a = find(&replaced, *a);
            }
        }
        for inst in &mut block.insts {
            for (_, v) in &mut inst.reads {
                *v = find(&replaced, *v);
            }
        }
    }
}

/// Scheduling hints, performance counters and tracing: no effect on results.
const DROPPED: &[&str] = &[
    "s_nop",
    "s_waitcnt",
    "s_waitcnt_vscnt",
    "s_waitcnt_vmcnt",
    "s_waitcnt_expcnt",
    "s_waitcnt_lgkmcnt",
    "s_waitcnt_depctr",
    "s_wait_idle",
    "s_inst_prefetch",
    "s_clause",
    "s_setprio",
    "s_sleep",
    "s_ttracedata",
    "s_ttracedata_imm",
    "s_incperflevel",
    "s_decperflevel",
    "v_nop",
    "v_pipeflush",
    "v_clrexcp",
];

/// Registers read, registers written.
type Effects = (Vec<Reg>, Vec<Reg>);

/// Registers an instruction reads and writes, one per dword, or `None` if
/// the IR drops it.
fn effects(
    inst: &Instruction,
    fields: &Fields,
    name: &'static str,
    lanes: u8,
    vgprs: u16,
) -> Result<Option<Effects>, ShaderError> {
    if DROPPED.contains(&name) {
        return Ok(None);
    }
    let unsupported = || ShaderError::Unsupported {
        offset: inst.offset,
        what: name,
    };
    let mask = |code: u8| (0..lanes).map(move |i| Reg::S(code + i));
    let (mut r, mut w) = (Vec::new(), Vec::new());
    let regs = |op: &Operand, out: &mut Vec<Reg>| {
        add_regs(op, lanes, out).map_err(|_| ShaderError::Isa {
            offset: inst.offset,
            reason: "register range out of bounds",
        })
    };
    let vector = !matches!(
        fields,
        Fields::Sop { .. } | Fields::Sopp { .. } | Fields::Smem { .. }
    );
    match fields {
        Fields::Sop { dst, src, .. } => {
            if name.starts_with("s_movrel")
                || name.starts_with("s_subvector_loop")
                || matches!(name, "s_call_b64" | "s_rfe_b64")
            {
                return Err(unsupported());
            }
            for s in src {
                regs(s, &mut r)?;
            }
            if let Some(d) = dst {
                regs(d, &mut w)?;
                // These change (or may keep) the destination's old value.
                if name.starts_with("s_bitset")
                    || name.starts_with("s_cmov")
                    || matches!(name, "s_addk_i32" | "s_mulk_i32")
                {
                    regs(d, &mut r)?;
                }
            }
            if matches!(name, "s_addc_u32" | "s_subb_u32" | "s_cmovk_i32")
                || name.starts_with("s_cselect_")
                || name.starts_with("s_cmov_")
            {
                r.push(Reg::Scc);
            }
            if name.contains("saveexec") || name.contains("wrexec") {
                r.extend(mask(EXEC));
                w.extend(mask(EXEC));
            }
            if writes_scc(inst.encoding, name) {
                w.push(Reg::Scc);
            }
        }
        Fields::Sopp { .. } => match name {
            "s_cbranch_scc0" | "s_cbranch_scc1" => r.push(Reg::Scc),
            "s_cbranch_vccz" | "s_cbranch_vccnz" => r.extend(mask(VCC)),
            "s_cbranch_execz" | "s_cbranch_execnz" => r.extend(mask(EXEC)),
            "s_sendmsg" | "s_sendmsghalt" => r.push(Reg::S(M0)),
            "s_endpgm"
            | "s_endpgm_saved"
            | "s_endpgm_ordered_ps_done"
            | "s_code_end"
            | "s_branch"
            | "s_barrier"
            | "s_icache_inv"
            | "s_wakeup"
            | "s_round_mode"
            | "s_denorm_mode" => {}
            _ => return Err(unsupported()),
        },
        Fields::Smem {
            data,
            base,
            soffset,
            ..
        } => match name {
            "s_memtime" | "s_memrealtime" | "s_get_waveid_in_workgroup" => {
                regs(data.as_ref().ok_or_else(unsupported)?, &mut w)?;
            }
            "s_gl1_inv" | "s_dcache_inv" | "s_dcache_wb" => {}
            _ if name.contains("_load_") || name.contains("_store_") => {
                regs(base, &mut r)?;
                regs(soffset, &mut r)?;
                let data = data.as_ref().ok_or_else(unsupported)?;
                regs(
                    data,
                    if name.contains("_load_") {
                        &mut w
                    } else {
                        &mut r
                    },
                )?;
            }
            _ => return Err(unsupported()),
        },
        Fields::Valu(v) => {
            let (src_indexed, dst_indexed) = match name {
                "v_movrels_b32" => (true, false),
                "v_movreld_b32" => (false, true),
                "v_movrelsd_b32" | "v_movrelsd_2_b32" | "v_swaprel_b32" => (true, true),
                _ => (false, false),
            };
            // v[reg + M0] for any M0 that stays inside the program's VGPRs.
            let indexed = |op: &Operand, out: &mut Vec<Reg>| match *op {
                Operand::Vector { reg, .. } if u16::from(reg) < vgprs => {
                    out.extend((u16::from(reg)..vgprs).map(|r| Reg::V(r as u8)));
                    Ok(())
                }
                _ => Err(ShaderError::Isa {
                    offset: inst.offset,
                    reason: "relative VGPR base outside the program's VGPRs",
                }),
            };
            for (i, s) in v.src.iter().enumerate() {
                if i == 0 && src_indexed {
                    indexed(s, &mut r)?;
                } else {
                    regs(s, &mut r)?;
                }
            }
            // GFX10 compares into EXEC write nothing else.
            if name.starts_with("v_cmpx") {
                w.extend(mask(EXEC));
            } else if let Some(d) = &v.dst {
                if dst_indexed {
                    indexed(d, &mut w)?;
                } else {
                    regs(d, &mut w)?;
                }
            }
            if let Some(c) = &v.carry {
                regs(c, &mut w)?;
            }
            match name {
                "v_swap_b32" => {
                    regs(&v.src[0], &mut w)?;
                    if let Some(d) = &v.dst {
                        regs(d, &mut r)?;
                    }
                }
                "v_swaprel_b32" => {
                    indexed(&v.src[0], &mut w)?;
                    if let Some(d) = &v.dst {
                        indexed(d, &mut r)?;
                    }
                }
                "v_div_fmas_f32" | "v_div_fmas_f64" => r.extend(mask(VCC)),
                _ => {}
            }
            if name.starts_with("v_interp_") || src_indexed || dst_indexed {
                r.push(Reg::S(M0));
            }
        }
        Fields::Vintrp { dst, src, .. } => {
            regs(src, &mut r)?;
            r.push(Reg::S(M0));
            regs(dst, &mut w)?;
        }
        Fields::Ds {
            dst,
            addr,
            data,
            gds,
            ..
        } => {
            for o in addr.iter().chain(data) {
                regs(o, &mut r)?;
            }
            if let Some(d) = dst {
                // A 16-bit read keeps the other half of its VGPR.
                if name.starts_with("ds_read") && name.contains("_d16") {
                    regs(d, &mut r)?;
                }
                regs(d, &mut w)?;
            }
            if *gds
                || name.starts_with("ds_gws")
                || name.contains("addtid")
                || matches!(name, "ds_ordered_count" | "ds_append" | "ds_consume")
            {
                r.push(Reg::S(M0));
            }
        }
        Fields::Buffer {
            data,
            addr,
            rsrc,
            soffset,
            offen,
            idxen,
            lds,
            cache,
            ..
        } => {
            if *offen || *idxen {
                regs(addr, &mut r)?;
            }
            regs(rsrc, &mut r)?;
            regs(soffset, &mut r)?;
            if *lds {
                r.push(Reg::S(M0));
            }
            if let Some(d) = data {
                if name.contains("_load_") {
                    if !lds {
                        regs(d, &mut w)?;
                    }
                } else if name.contains("_store_") {
                    regs(d, &mut r)?;
                } else if name.contains("_atomic_") {
                    regs(d, &mut r)?;
                    if cache.glc {
                        regs(&atomic_result(d, name), &mut w)?;
                    }
                } else {
                    return Err(unsupported());
                }
            }
        }
        Fields::Image {
            data,
            addr,
            rsrc,
            sampler,
            cache,
            ..
        } => {
            for a in addr.iter().chain([rsrc]).chain(sampler) {
                regs(a, &mut r)?;
            }
            if name.contains("_store") {
                regs(data, &mut r)?;
            } else if name.contains("_atomic_") {
                regs(data, &mut r)?;
                if cache.glc {
                    regs(&atomic_result(data, name), &mut w)?;
                }
            } else {
                regs(data, &mut w)?;
            }
        }
        Fields::Flat {
            dst,
            addr,
            data,
            saddr,
            ..
        } => {
            for o in [addr, data, saddr].into_iter().flatten() {
                regs(o, &mut r)?;
            }
            if let Some(d) = dst {
                regs(d, &mut w)?;
            }
        }
        Fields::Exp { src, .. } => {
            for s in src.iter().flatten() {
                regs(s, &mut r)?;
            }
        }
    }
    if vector {
        r.extend(mask(EXEC));
        // Lanes disabled in EXEC keep the old value.
        r.extend(w.iter().filter(|w| matches!(w, Reg::V(_))));
    }
    let mut seen = Vec::with_capacity(r.len());
    r.retain(|reg| {
        let new = !seen.contains(reg);
        seen.push(*reg);
        new
    });
    Ok(Some((r, w)))
}

/// Scalar ALU opcodes that set SCC (carry, overflow, comparison, or "result
/// is not zero").
fn writes_scc(encoding: gfx10::Encoding, name: &str) -> bool {
    use gfx10::Encoding::*;
    match encoding {
        Sopc => true,
        Sopk => name.starts_with("s_cmpk_") || name == "s_addk_i32",
        _ => {
            let stem = name.split('_').nth(1).unwrap_or("");
            name.contains("saveexec")
                || name.contains("wrexec")
                || matches!(
                    stem,
                    "add"
                        | "sub"
                        | "addc"
                        | "subb"
                        | "min"
                        | "max"
                        | "and"
                        | "or"
                        | "xor"
                        | "andn2"
                        | "orn2"
                        | "nand"
                        | "nor"
                        | "xnor"
                        | "lshl"
                        | "lshr"
                        | "ashr"
                        | "bfe"
                        | "absdiff"
                        | "lshl1"
                        | "lshl2"
                        | "lshl3"
                        | "lshl4"
                        | "not"
                        | "wqm"
                        | "bcnt0"
                        | "bcnt1"
                        | "quadmask"
                        | "abs"
                )
        }
    }
}

/// What an atomic with `glc` returns: the old value (for compare-and-swap,
/// half the data operand).
fn atomic_result(data: &Operand, name: &str) -> Operand {
    match *data {
        Operand::Vector { reg, dwords } if name.contains("cmpswap") => Operand::Vector {
            reg,
            dwords: dwords / 2,
        },
        op => op,
    }
}

/// Appends the register dwords an operand names; reading `vccz`, `execz`,
/// `scc` or `lds_direct` reads the register behind it.
fn add_regs(op: &Operand, lanes: u8, out: &mut Vec<Reg>) -> Result<(), ()> {
    match *op {
        Operand::Scalar { code, .. } if code == gfx10::NULL => {}
        Operand::Scalar { code, dwords } => {
            let end = u16::from(code) + u16::from(dwords);
            if end > 128 {
                return Err(());
            }
            out.extend((code..end as u8).map(Reg::S));
        }
        Operand::Vector { reg, dwords } => {
            if u16::from(reg) + u16::from(dwords) > 256 {
                return Err(());
            }
            out.extend((0..dwords).map(|i| Reg::V(reg + i)));
        }
        Operand::Const(251) => out.extend((0..lanes).map(|i| Reg::S(VCC + i))),
        Operand::Const(252) => out.extend((0..lanes).map(|i| Reg::S(EXEC + i))),
        Operand::Const(253) => out.push(Reg::Scc),
        Operand::Const(254) => out.push(Reg::S(M0)),
        Operand::Const(_) | Operand::Literal(_) | Operand::Attr { .. } => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const S_ENDPGM: u32 = 0xBF81_0000;

    fn build_words(words: &[u32], wave64: bool) -> Program {
        let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        build(&code, wave64, 256).unwrap()
    }

    fn input(p: &Program, reg: Reg) -> Value {
        p.inputs.iter().find(|i| i.0 == reg).expect("input").1
    }

    /// Lanes of a VGPR used for nothing else are scalars of their own; a
    /// VGPR used otherwise too keeps its lane accesses.
    #[test]
    fn spilled_lanes_become_scalars() {
        let mut code = vec![
            0xD761_0005,
            0x0001_0609, // v_writelane_b32 v5, s9, 3
            0xD761_0005,
            0x0001_080A, // v_writelane_b32 v5, s10, 4
            0xD760_0002,
            0x0001_0705, // v_readlane_b32 s2, v5, 3
            S_ENDPGM,
        ];
        let p = build_words(&code, false);
        let i = &p.blocks[0].insts;
        assert!(i[..3].iter().all(|i| i.name() == "s_mov_b32"));
        assert_eq!(i[0].reads, [(Reg::S(9), input(&p, Reg::S(9)))]);
        assert_eq!(i[2].reads[0].1, i[0].writes[0].1);
        assert_ne!(i[0].writes[0].0, i[1].writes[0].0);
        // 64-lane waves keep them.
        let p = build_words(&code, true);
        assert_eq!(p.blocks[0].insts[0].name(), "v_writelane_b32");
        code.insert(6, 0x7E00_0305); // v_mov_b32 v0, v5
        let p = build_words(&code, false);
        assert_eq!(p.blocks[0].insts[0].name(), "v_writelane_b32");
        // Read before any write: zero.
        let p = build_words(&[0xD760_0002, 0x0001_0705, S_ENDPGM], false);
        let i = &p.blocks[0].insts;
        assert_eq!(i[1].reads[0].1, i[0].writes[0].1);
        assert!(p.inputs.iter().all(|i| !matches!(i.0, Reg::V(_))));
    }

    #[test]
    fn joins_get_phis_only_where_values_differ() {
        let code = [
            0xBF06_0100, // s_cmp_eq_u32 s0, s1
            0xBF85_0001, // s_cbranch_scc1 -> 12
            0xBE82_0380, // s_mov_b32 s2, 0
            0x7E00_0202, // 12: v_mov_b32 v0, s2
            0x7E02_0200, // v_mov_b32 v1, s0
            S_ENDPGM,
        ];
        for wave64 in [false, true] {
            let p = build_words(&code, wave64);
            let [b0, b1, b2] = &p.blocks[..] else {
                panic!("{:?}", p.blocks)
            };
            assert_eq!(
                b0.exit,
                Exit::Branch {
                    taken: 2,
                    not_taken: 1
                }
            );
            assert_eq!(b1.exit, Exit::Jump(2));
            assert_eq!(b2.preds, [0, 1]);
            assert_eq!(b0.insts[1].read(Reg::Scc), b0.insts[0].write(Reg::Scc));

            // s2 differs per path; s0, EXEC, v0 and v1 do not.
            let [phi] = &b2.phis[..] else {
                panic!("{:?}", b2.phis)
            };
            assert_eq!(phi.reg, Reg::S(2));
            assert_eq!(
                phi.args,
                [input(&p, Reg::S(2)), b1.insts[0].write(Reg::S(2)).unwrap()]
            );
            let [mov0, mov1, end] = &b2.insts[..] else {
                panic!()
            };
            assert_eq!(mov0.read(Reg::S(2)), Some(phi.value));
            assert_eq!(mov1.read(Reg::S(0)), Some(input(&p, Reg::S(0))));
            // The old v1 is read: lanes outside EXEC keep it.
            assert_eq!(mov1.read(Reg::V(1)), Some(input(&p, Reg::V(1))));
            assert_eq!(mov1.read(Reg::S(EXEC)), Some(input(&p, Reg::S(EXEC))));
            assert_eq!(mov1.read(Reg::S(EXEC + 1)).is_some(), wave64);
            assert_eq!((end.name(), b2.exit), ("s_endpgm", Exit::End));
        }
    }

    #[test]
    fn in_place_scalar_ops_read_their_destination() {
        let p = build_words(
            &[
                0xBE8D_1D94, // s_bitset1_b32 s13, 20
                0xB78E_0010, // s_addk_i32 s14, 0x10
                0xBE8F_0380, // s_mov_b32 s15, 0
                S_ENDPGM,
            ],
            false,
        );
        let [bitset, addk, mov, _] = &p.blocks[0].insts[..] else {
            panic!("{:?}", p.blocks[0].insts)
        };
        assert_eq!(bitset.read(Reg::S(13)), Some(input(&p, Reg::S(13))));
        assert_eq!(addk.read(Reg::S(14)), Some(input(&p, Reg::S(14))));
        assert_eq!(mov.read(Reg::S(15)), None);
    }

    #[test]
    fn loops_carry_values_through_phis() {
        let p = build_words(
            &[
                0xBE80_0380, // s_mov_b32 s0, 0
                0x8000_8100, // 4: s_add_u32 s0, s0, 1
                0xBF07_8400, // s_cmp_lg_u32 s0, 4
                0xBF85_FFFD, // s_cbranch_scc1 -> 4
                S_ENDPGM,
            ],
            true,
        );
        assert!(p.inputs.is_empty(), "{:?}", p.inputs);
        let [b0, b1, b2] = &p.blocks[..] else {
            panic!()
        };
        assert_eq!(b1.preds, [0, 1]);
        assert_eq!(
            b1.exit,
            Exit::Branch {
                taken: 1,
                not_taken: 2
            }
        );
        let [phi] = &b1.phis[..] else {
            panic!("{:?}", b1.phis)
        };
        let add = &b1.insts[0];
        assert_eq!(phi.reg, Reg::S(0));
        assert_eq!(
            phi.args,
            [
                b0.insts[0].write(Reg::S(0)).unwrap(),
                add.write(Reg::S(0)).unwrap()
            ]
        );
        assert_eq!(add.read(Reg::S(0)), Some(phi.value));
        assert_eq!(b1.insts[1].read(Reg::S(0)), add.write(Reg::S(0)));
        assert_eq!(b2.insts[0].name(), "s_endpgm");
    }

    #[test]
    fn loops_can_branch_to_offset_zero() {
        let p = build_words(
            &[
                0x8000_8100, // 0: s_add_u32 s0, s0, 1
                0xBF07_8400, // s_cmp_lg_u32 s0, 4
                0xBF85_FFFD, // s_cbranch_scc1 -> 0
                S_ENDPGM,
            ],
            false,
        );
        let [entry, header, end] = &p.blocks[..] else {
            panic!("{:?}", p.blocks);
        };
        assert!(entry.preds.is_empty());
        assert!(entry.insts.is_empty());
        assert_eq!(entry.exit, Exit::Jump(1));
        assert_eq!(header.offset, 0);
        assert_eq!(header.preds, [0, 1]);
        assert_eq!(
            header.exit,
            Exit::Branch {
                taken: 1,
                not_taken: 2,
            }
        );
        assert_eq!(end.exit, Exit::End);
        let [phi] = &header.phis[..] else {
            panic!("{:?}", header.phis);
        };
        let add = &header.insts[0];
        assert_eq!(phi.reg, Reg::S(0));
        assert_eq!(
            phi.args,
            [input(&p, Reg::S(0)), add.write(Reg::S(0)).unwrap()]
        );
        assert_eq!(add.read(Reg::S(0)), Some(phi.value));
        assert!(crate::structure::structure(&p).is_ok());
    }

    #[test]
    fn drops_scheduling_and_follows_far_jumps() {
        let p = build_words(
            &[
                0xBF80_0000, // s_nop 0
                0xBE88_1F00, // s_getpc_b64 s[8:9]
                0x8008_FF08,
                20,          // s_add_u32 s8, s8, 20  -> 8 + 20 = 28
                0x8209_8009, // s_addc_u32 s9, s9, 0
                0xBE80_2008, // s_setpc_b64 s[8:9]
                0xFFFF_FFFF, // not code
                S_ENDPGM,
            ],
            false,
        );
        let names: Vec<_> = p.blocks[0].insts.iter().map(Inst::name).collect();
        assert_eq!(
            names,
            ["s_getpc_b64", "s_add_u32", "s_addc_u32", "s_setpc_b64"]
        );
        assert_eq!(p.blocks[0].exit, Exit::Jump(1));
        assert_eq!(p.blocks[1].offset, 28);
        // s_addc_u32 reads the carry s_add_u32 left in SCC.
        let i = &p.blocks[0].insts;
        assert_eq!(i[2].read(Reg::Scc), i[1].write(Reg::Scc));
    }

    #[test]
    fn reports_what_it_cannot_express() {
        let code: Vec<u8> = [0xBE80_2E01_u32, S_ENDPGM] // s_movrels_b32 s0, s1
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect();
        assert_eq!(
            build(&code, false, 256),
            Err(ShaderError::Unsupported {
                offset: 0,
                what: "s_movrels_b32"
            })
        );
    }

    #[test]
    fn relative_vgpr_access_spans_up_to_the_vgpr_count() {
        let code: Vec<u8> = [
            0x7E04_8501_u32, // v_movreld_b32 v2, v1: v[2 + m0] = v1
            0x7E00_8704,     // v_movrels_b32 v0, v4: v0 = v[4 + m0]
            S_ENDPGM,
        ]
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .collect();
        let p = build(&code, false, 6).unwrap();
        let [movreld, movrels, _] = &p.blocks[0].insts[..] else {
            panic!()
        };
        let vgprs = |regs: &[(Reg, Value)]| -> Vec<u8> {
            regs.iter()
                .filter_map(|r| match r.0 {
                    Reg::V(v) => Some(v),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(vgprs(&movreld.writes), [2, 3, 4, 5]);
        // v1 is the source; v2-v5 are read for the lanes and indices not written.
        assert_eq!(vgprs(&movreld.reads), [1, 2, 3, 4, 5]);
        assert!(movreld.read(Reg::S(M0)).is_some());
        assert_eq!(vgprs(&movrels.writes), [0]);
        assert_eq!(vgprs(&movrels.reads), [4, 5, 0]);
        assert_eq!(movrels.read(Reg::V(4)), movreld.write(Reg::V(4)));
        assert!(build(&code, false, 4).is_err(), "v4 is past 4 VGPRs");
    }
}
