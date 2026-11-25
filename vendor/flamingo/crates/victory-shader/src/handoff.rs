//! The ES to GS handoff of a merged geometry stage ([`crate::merged`]), as
//! the code of the two halves shows it.
//!
//! The ES half fetches a vertex, writes it to LDS as a fixed-size record
//! (`ds_write_b32` .. `ds_write_b128` at one base address and constant
//! offsets), waits at `s_barrier` and leaves through `s_setpc_b64` into the
//! GS half. The GS half reads the records of the vertices of its primitive
//! (`ds_read_b32` .. `ds_read_b128`) at the offset the hardware gives it for
//! each vertex.
//!
//! A host vertex shader has no LDS and a host geometry shader no way to read
//! another stage's memory, so the handoff is modeled, not run: what the ES
//! half writes at record dword k is output k of the vertex, and what the GS
//! half reads at vertex v, record dword k is input k of vertex v. This module
//! finds those writes and reads. Emission is [`crate::emit`]'s.
//!
//! What is assumed (the ISA's convention for a merged stage, not checked
//! against a title program):
//!
//! * The GS half gets the ES-ring offsets of its vertices packed two to a
//!   VGPR: vertex 0 in the low half of v0, vertex 1 in its high half, vertex
//!   2 in the low half of v1 ([`VERTEX_OFFSETS`]). A read whose address is
//!   such a half (scaled, plus constants) is a read of that vertex's record;
//!   a read at any other address is the GS half's own output memory.
//! * The ES half's writes all use one base address (its vertex's slot) and
//!   constant offsets, the same units as the GS half's.

use std::collections::{BTreeSet, HashMap};

use crate::ShaderError::{self, Unsupported};
use crate::gfx10::{Fields, Operand};
use crate::ir::{Exit, Inst, Program, Reg, Value};

/// Where the GS half finds the ES-ring offset of input vertex `n`: the VGPR
/// and whether it is the high half.
pub const VERTEX_OFFSETS: [(u8, bool); 4] = [(0, false), (0, true), (1, false), (1, true)];

/// Triangles in.
pub const MAX_VERTICES: usize = 3;

type Site = (usize, usize);

/// What a value is, as `base * mul + add` in wrapping 32-bit arithmetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Base {
    /// No base: `add` is the whole value.
    Constant,
    /// A VGPR as it arrived.
    Input(u8),
    /// The low or high 16 bits of an arrived VGPR.
    Half { reg: u8, high: bool },
    /// Anything else, named by the value that holds it.
    Opaque(Value),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lin {
    pub base: Base,
    pub mul: u32,
    pub add: u32,
}

impl Lin {
    fn constant(add: u32) -> Lin {
        Lin { base: Base::Constant, mul: 0, add }
    }

    fn of(base: Base) -> Lin {
        Lin { base, mul: 1, add: 0 }
    }

    fn is_constant(&self) -> bool {
        self.base == Base::Constant
    }
}

struct Eval<'a> {
    p: &'a Program,
    defs: HashMap<Value, Site>,
    inputs: HashMap<Value, u8>,
}

impl<'a> Eval<'a> {
    fn new(p: &'a Program) -> Self {
        let mut defs = HashMap::new();
        for (b, block) in p.blocks.iter().enumerate() {
            for (i, inst) in block.insts.iter().enumerate() {
                for &(_, v) in &inst.writes {
                    defs.insert(v, (b, i));
                }
            }
        }
        let inputs = p
            .inputs
            .iter()
            .filter_map(|&(reg, v)| match reg {
                Reg::V(r) => Some((v, r)),
                _ => None,
            })
            .collect();
        Eval { p, defs, inputs }
    }

    fn value(&self, v: Value, depth: u32) -> Lin {
        if let Some(&r) = self.inputs.get(&v) {
            return Lin::of(Base::Input(r));
        }
        match self.defs.get(&v) {
            Some(&(b, i)) if depth < 32 => self.inst(&self.p.blocks[b].insts[i], v, depth),
            _ => Lin::of(Base::Opaque(v)),
        }
    }

    fn operand(&self, inst: &Inst, op: &Operand, depth: u32) -> Option<Lin> {
        match *op {
            Operand::Const(c @ 128..=192) => Some(Lin::constant(u32::from(c) - 128)),
            Operand::Const(c @ 193..=208) => Some(Lin::constant(0u32.wrapping_sub(u32::from(c) - 192))),
            Operand::Literal(l) => Some(Lin::constant(l)),
            Operand::Vector { reg, .. } => Some(self.value(inst.read(Reg::V(reg))?, depth + 1)),
            Operand::Scalar { code, .. } => Some(self.value(inst.read(Reg::S(code))?, depth + 1)),
            _ => None,
        }
    }

    fn inst(&self, inst: &Inst, v: Value, depth: u32) -> Lin {
        let opaque = Lin::of(Base::Opaque(v));
        let Fields::Valu(f) = &inst.fields else {
            return opaque;
        };
        let args: Option<Vec<Lin>> = f.src.iter().map(|s| self.operand(inst, s, depth)).collect();
        let Some(a) = args else { return opaque };
        let c = |l: &Lin| l.is_constant().then_some(l.add);
        let shifted = |x: Lin, s: u32| Lin {
            base: x.base,
            mul: x.mul.wrapping_shl(s),
            add: x.add.wrapping_shl(s),
        };
        let plain = |x: &Lin| x.mul == 1 && x.add == 0;
        match (inst.name(), a.as_slice()) {
            ("v_mov_b32", [x]) => *x,
            ("v_and_b32", [x, y]) | ("v_and_b32", [y, x]) if c(y) == Some(0xFFFF) => match x.base {
                Base::Input(reg) if plain(x) => Lin::of(Base::Half { reg, high: false }),
                Base::Half { .. } if plain(x) => *x,
                _ => opaque,
            },
            ("v_lshrrev_b32", [s, x]) if c(s) == Some(16) => match x.base {
                Base::Input(reg) if plain(x) => Lin::of(Base::Half { reg, high: true }),
                _ => opaque,
            },
            // (the hardware reads the offset in five bits: 48 is 16)
            ("v_bfe_u32", [x, off, width]) if c(width) == Some(16) && c(off).is_some_and(|o| (o & 31) % 16 == 0) => {
                match x.base {
                    Base::Input(reg) if plain(x) => Lin::of(Base::Half { reg, high: c(off).map(|o| o & 31) == Some(16) }),
                    _ => opaque,
                }
            }
            ("v_lshlrev_b32", [s, x]) if c(s).is_some_and(|s| s < 32) => {
                if x.is_constant() {
                    Lin::constant(x.add.wrapping_shl(c(s).unwrap()))
                } else {
                    shifted(*x, c(s).unwrap())
                }
            }
            ("v_add_nc_u32" | "v_add_u32" | "v_add_co_u32", [x, y]) => match (c(x), c(y)) {
                (Some(p), Some(q)) => Lin::constant(p.wrapping_add(q)),
                (Some(k), None) => Lin { add: y.add.wrapping_add(k), ..*y },
                (None, Some(k)) => Lin { add: x.add.wrapping_add(k), ..*x },
                _ => opaque,
            },
            ("v_mul_u32_u24", [x, y]) => match (c(x), c(y)) {
                (Some(p), Some(q)) => Lin::constant(p.wrapping_mul(q)),
                (None, Some(k)) | (Some(k), None) => {
                    let x = if c(x).is_some() { y } else { x };
                    Lin { base: x.base, mul: x.mul.wrapping_mul(k), add: x.add.wrapping_mul(k) }
                }
                _ => opaque,
            },
            _ => opaque,
        }
    }
}

/// How many dwords an LDS read or write moves, from its name.
fn width(name: &str) -> Option<u32> {
    match name {
        "ds_read_b32" | "ds_write_b32" => Some(1),
        "ds_read_b64" | "ds_write_b64" => Some(2),
        "ds_read_b96" | "ds_write_b96" => Some(3),
        "ds_read_b128" | "ds_write_b128" => Some(4),
        _ => None,
    }
}

/// Instructions that name LDS in the encoding but touch none.
fn no_memory(name: &str) -> bool {
    name.starts_with("ds_swizzle") || name.contains("permute") || name == "ds_nop"
}

fn unsupported(what: &'static str) -> ShaderError {
    Unsupported { offset: 0, what }
}

/// A record write of the ES half: VGPRs `reg..reg + count` go to record
/// dwords `dword..dword + count`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Store {
    pub site: Site,
    pub dword: u32,
    pub count: u32,
    pub reg: u8,
}

/// The ES half's record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EsRecord {
    pub stores: Vec<Store>,
    /// Dwords of the record: past the highest one written.
    pub dwords: u32,
    /// An `s_barrier` comes before the handover.
    pub barrier: bool,
}

impl EsRecord {
    /// Whether some store writes record dword `dword`.
    pub fn writes(&self, dword: u32) -> bool {
        self.stores.iter().any(|s| (s.dword..s.dword + s.count).contains(&dword))
    }
}

/// Whether an `s_barrier` precedes the handover on every path to it. Fails
/// unless every path from the entry ends in the handover (`s_setpc_b64`): a
/// reachable `s_endpgm` is a path whose record never reaches the GS half.
fn barrier_before_every_handover(p: &Program) -> Result<bool, ShaderError> {
    let n = p.blocks.len();
    let mut reachable = vec![false; n];
    let mut stack = vec![0usize];
    while let Some(b) = stack.pop() {
        if !std::mem::replace(&mut reachable[b], true) {
            stack.extend(p.blocks[b].exit.successors());
        }
    }
    let ends = |b: usize| p.blocks[b].exit == Exit::End;
    if (0..n).any(|b| reachable[b] && ends(b)) || !(0..n).any(|b| reachable[b] && p.blocks[b].exit == Exit::Indirect) {
        return Err(unsupported("an ES half that does not hand over to the GS half"));
    }
    let has = |b: usize| p.blocks[b].insts.iter().any(|i| i.name() == "s_barrier");
    // Must-analysis: a barrier is behind a block when it is behind every
    // reachable predecessor (greatest fixed point, the entry has none).
    let mut behind = vec![true; n];
    behind[0] = has(0);
    loop {
        let mut changed = false;
        for b in 0..n {
            if !reachable[b] {
                continue;
            }
            let now = has(b)
                || b != 0
                    && (0..n)
                        .filter(|&q| reachable[q] && p.blocks[q].exit.successors().contains(&b))
                        .all(|q| behind[q]);
            if now != behind[b] {
                behind[b] = now;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    Ok((0..n).all(|b| !(reachable[b] && p.blocks[b].exit == Exit::Indirect) || behind[b]))
}

/// The records an ES program writes.
///
/// Fails when it does not end by handing over to the GS half
/// (`s_setpc_b64`), touches LDS in any way but whole-dword record writes,
/// writes at more than one base, or writes nothing.
pub fn es_record(p: &Program) -> Result<EsRecord, ShaderError> {
    let barrier = barrier_before_every_handover(p)?;
    let eval = Eval::new(p);
    let mut stores = Vec::new();
    let mut at: Option<(Base, u32)> = None;
    for (b, block) in p.blocks.iter().enumerate() {
        for (i, inst) in block.insts.iter().enumerate() {
            let Fields::Ds { addr, data, offset0, offset1, gds, .. } = &inst.fields else {
                continue;
            };
            let name = inst.name();
            if no_memory(name) {
                continue;
            }
            let Some(count) = width(name).filter(|_| name.starts_with("ds_write") && !gds) else {
                return Err(unsupported("an LDS access of the ES half other than a record write"));
            };
            let Some(Operand::Vector { reg: a, .. }) = *addr else {
                return Err(unsupported("an LDS write without an address"));
            };
            let Some(Operand::Vector { reg: d, .. }) = data.first().copied() else {
                return Err(unsupported("an LDS write without data"));
            };
            let lin = eval.value(inst.read(Reg::V(a)).ok_or(unsupported("an unread LDS address"))?, 0);
            if lin.is_constant() {
                return Err(unsupported("a record write at a constant address"));
            }
            match at {
                None => at = Some((lin.base, lin.mul)),
                Some(first) if first == (lin.base, lin.mul) => {}
                Some(_) => return Err(unsupported("ES record writes at more than one base")),
            }
            let byte = lin.add.wrapping_add(u32::from(*offset0) | u32::from(*offset1) << 8);
            if !byte.is_multiple_of(4) {
                return Err(unsupported("a record write off a dword"));
            }
            stores.push(Store { site: (b, i), dword: byte / 4, count, reg: d });
        }
    }
    let dwords = stores.iter().map(|s| s.dword + s.count).max().ok_or(unsupported("an ES half that writes no record"))?;
    Ok(EsRecord { stores, dwords, barrier })
}

/// A read of one input vertex's record by the GS half: record dwords
/// `dword..dword + count` of vertex `vertex` go to VGPRs `reg..reg + count`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VertexRead {
    pub site: Site,
    pub vertex: u8,
    pub dword: u32,
    pub count: u32,
    pub reg: u8,
}

/// The reads of input vertices in a GS program, in code order. Reads at
/// other addresses are the program's own output memory and are left out.
pub fn vertex_reads(p: &Program) -> Result<Vec<VertexRead>, ShaderError> {
    let eval = Eval::new(p);
    let mut out = Vec::new();
    for (b, block) in p.blocks.iter().enumerate() {
        for (i, inst) in block.insts.iter().enumerate() {
            let Fields::Ds { dst, addr, offset0, offset1, gds, .. } = &inst.fields else {
                continue;
            };
            let name = inst.name();
            if no_memory(name) || *gds || !name.starts_with("ds_read") {
                continue;
            }
            let Some(Operand::Vector { reg: a, .. }) = *addr else { continue };
            let Some(v) = inst.read(Reg::V(a)) else { continue };
            let lin = eval.value(v, 0);
            let Base::Half { reg, high } = lin.base else { continue };
            let Some(vertex) = VERTEX_OFFSETS.iter().position(|&o| o == (reg, high)) else {
                return Err(unsupported("a read at the offset half of no input vertex"));
            };
            if vertex >= MAX_VERTICES {
                return Err(unsupported("a geometry input of more than three vertices"));
            }
            let Some(count) = width(name) else {
                return Err(unsupported("an LDS read of a vertex record that is not whole dwords"));
            };
            let Some(Operand::Vector { reg: d, .. }) = *dst else {
                return Err(unsupported("an LDS read without a destination"));
            };
            let byte = lin.add.wrapping_add(u32::from(*offset0) | u32::from(*offset1) << 8);
            if !byte.is_multiple_of(4) {
                return Err(unsupported("a vertex read off a dword"));
            }
            out.push(VertexRead { site: (b, i), vertex: vertex as u8, dword: byte / 4, count, reg: d });
        }
    }
    Ok(out)
}

/// A GS program's own LDS access: it writes its output records there before
/// it emits them, and may read them back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Access {
    pub site: Site,
    pub write: bool,
    /// The address is `base * mul + add` plus the instruction's offset
    /// (`byte`, in total, the constant part).
    pub base: Base,
    pub mul: u32,
    pub byte: u32,
    pub count: u32,
    /// The first VGPR of the data written or of the destination.
    pub reg: u8,
}

/// The LDS accesses of a GS program that are not reads of input vertices, in
/// code order. Fails on any other LDS operation (atomics, paired reads and
/// writes, bytes and halves).
pub fn own_accesses(p: &Program) -> Result<Vec<Access>, ShaderError> {
    let eval = Eval::new(p);
    let mut out = Vec::new();
    for (b, block) in p.blocks.iter().enumerate() {
        for (i, inst) in block.insts.iter().enumerate() {
            let Fields::Ds { dst, addr, data, offset0, offset1, gds } = &inst.fields else {
                continue;
            };
            let name = inst.name();
            if no_memory(name) {
                continue;
            }
            let write = name.starts_with("ds_write");
            let Some(count) = width(name).filter(|_| !gds) else {
                return Err(unsupported("an LDS operation of the GS half beyond whole-dword reads and writes"));
            };
            let Some(Operand::Vector { reg: a, .. }) = *addr else {
                return Err(unsupported("an LDS access without an address"));
            };
            let lin = eval.value(inst.read(Reg::V(a)).ok_or(unsupported("an unread LDS address"))?, 0);
            if matches!(lin.base, Base::Half { .. }) && !write {
                continue; // an input vertex ([`vertex_reads`])
            }
            let reg = match (write, data.first(), dst) {
                (true, Some(Operand::Vector { reg, .. }), _) | (false, _, Some(Operand::Vector { reg, .. })) => *reg,
                _ => return Err(unsupported("an LDS access without a register")),
            };
            let byte = lin.add.wrapping_add(u32::from(*offset0) | u32::from(*offset1) << 8);
            if !byte.is_multiple_of(4) {
                return Err(unsupported("an LDS access off a dword"));
            }
            out.push(Access { site: (b, i), write, base: lin.base, mul: lin.mul, byte, count, reg });
        }
    }
    Ok(out)
}

/// An ES record and the reads of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handoff {
    pub es: EsRecord,
    pub reads: Vec<VertexRead>,
    /// Input vertices the GS half reads (highest index + 1).
    pub vertices: u8,
}

impl Handoff {
    /// The record dwords any vertex read touches.
    pub fn read_dwords(&self) -> BTreeSet<u32> {
        self.reads.iter().flat_map(|r| r.dword..r.dword + r.count).collect()
    }
}

/// The handoff between an ES program and a GS program: the record the ES
/// half writes and the vertex reads of the GS half, which must read only
/// dwords the ES half writes.
pub fn handoff(es: &Program, gs: &Program) -> Result<Handoff, ShaderError> {
    let es = es_record(es)?;
    let reads = vertex_reads(gs)?;
    for r in &reads {
        if (r.dword..r.dword + r.count).any(|d| !es.writes(d)) {
            return Err(unsupported("a record dword the ES half never writes"));
        }
    }
    let vertices = reads.iter().map(|r| r.vertex + 1).max().unwrap_or(0);
    Ok(Handoff { es, reads, vertices })
}

#[cfg(any(test, feature = "synthetic"))]
pub(crate) mod tests {
    #![cfg_attr(not(test), allow(dead_code, unused))]
    use super::*;

    pub const S_ENDPGM: u32 = 0xBF81_0000;
    pub const S_BARRIER: u32 = 0xBF8A_0000;
    /// `s_setpc_b64 s[8:9]`.
    pub const S_SETPC: u32 = 0xBE80_2008;

    pub fn vop2(op: u32, dst: u32, src0: u32, vsrc1: u32) -> u32 {
        op << 25 | dst << 17 | vsrc1 << 9 | src0
    }

    pub fn ds(op: u32, offset: u32) -> u32 {
        0xD800_0000 | op << 18 | offset
    }

    /// The second dword of a DS instruction.
    pub fn ds_regs(addr: u32, data0: u32, dst: u32) -> u32 {
        dst << 24 | data0 << 8 | addr
    }

    pub const V_AND: u32 = 0x1b;
    pub const V_LSHRREV: u32 = 0x16;
    pub const V_LSHLREV: u32 = 0x1a;
    pub const DS_WRITE_B32: u32 = 0x0d;
    pub const DS_WRITE_B128: u32 = 0x0df;
    pub const DS_READ_B32: u32 = 0x36;
    pub const DS_READ_B128: u32 = 0x0ff;

    pub fn program(words: &[u32]) -> Program {
        let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        crate::ir::build(&code, true, 64).unwrap()
    }

    /// An ES half: v1 = v0 << 5 (the vertex's slot of 32 bytes), a 16-byte
    /// write at +0 of v[4:7], a 32-bit one at +16 of v8, the barrier, the
    /// handover.
    pub fn es_words() -> Vec<u32> {
        vec![
            vop2(V_LSHLREV, 1, 133, 0), // v_lshlrev_b32 v1, 5, v0
            ds(DS_WRITE_B128, 0),
            ds_regs(1, 4, 0),
            ds(DS_WRITE_B32, 16),
            ds_regs(1, 8, 0),
            S_BARRIER,
            S_SETPC,
        ]
    }

    #[test]
    fn the_es_half_writes_a_record_at_one_base() {
        let r = es_record(&program(&es_words())).unwrap();
        assert_eq!(r.dwords, 5);
        assert!(r.barrier);
        let at: Vec<_> = r.stores.iter().map(|s| (s.dword, s.count, s.reg)).collect();
        assert_eq!(at, [(0, 4, 4), (4, 1, 8)]);
        assert!(r.writes(3) && r.writes(4) && !r.writes(5));
    }

    #[test]
    fn an_es_half_that_does_not_hand_over_or_writes_oddly_is_refused() {
        let mut ends = es_words();
        *ends.last_mut().unwrap() = S_ENDPGM;
        assert!(es_record(&program(&ends)).is_err(), "no handover");
        // A read of LDS in the ES half.
        let mut reads = es_words();
        reads.splice(1..1, [ds(DS_READ_B32, 0), ds_regs(1, 0, 9)]);
        assert!(es_record(&program(&reads)).is_err(), "an LDS read");
        // Writes at two bases: the second at v0 itself.
        let mut bases = es_words();
        bases[4] = ds_regs(0, 8, 0);
        assert!(es_record(&program(&bases)).is_err(), "two bases");
        // No record at all.
        assert!(es_record(&program(&[S_BARRIER, S_SETPC])).is_err(), "no write");
    }

    const S_CMP_EQ_U32_S0_0: u32 = 0xBF06_8000;

    #[test]
    fn every_reachable_path_has_to_hand_over() {
        // v_lshlrev, s_cmp_eq_u32 s0, 0, s_cbranch_scc1 over the writes, the
        // barrier and the handover to a plain end: that path loses the record.
        let w = es_words();
        let mut forked = vec![w[0], S_CMP_EQ_U32_S0_0, 0xBF85_0006];
        forked.extend_from_slice(&w[1..]);
        forked.push(S_ENDPGM);
        assert!(es_record(&program(&forked)).is_err(), "a path that ends");
        // An end nothing reaches does not matter.
        let mut dead = es_words();
        dead.push(S_ENDPGM);
        assert!(es_record(&program(&dead)).is_ok(), "an unreachable end");
    }

    #[test]
    fn the_barrier_has_to_be_on_every_path_to_the_handover() {
        // The branch skips the barrier: one path has none.
        let w = es_words();
        let mut skipped = vec![w[0], S_CMP_EQ_U32_S0_0, 0xBF85_0001, S_BARRIER];
        skipped.extend_from_slice(&w[1..5]);
        skipped.push(S_SETPC);
        assert!(!es_record(&program(&skipped)).unwrap().barrier, "a path without it");
        // A barrier in code nothing reaches is no barrier.
        let mut dead = es_words();
        dead.retain(|&x| x != S_BARRIER);
        dead.extend([S_BARRIER, S_ENDPGM]);
        assert!(!es_record(&program(&dead)).unwrap().barrier, "an unreachable barrier");
        assert!(es_record(&program(&es_words())).unwrap().barrier);
    }

    /// `v_bfe_u32 v3, v0, <offset>, 16`, `v_lshlrev_b32 v3, 2, v3` and a read
    /// at that address.
    fn bfe_read(offset: u32) -> Vec<VertexRead> {
        vertex_reads(&program(&[
            0xD548_0003,
            256 | (128 + offset) << 9 | 144 << 18,
            vop2(V_LSHLREV, 3, 130, 3),
            ds(DS_READ_B32, 0),
            ds_regs(3, 0, 4),
            S_ENDPGM,
        ]))
        .unwrap()
    }

    #[test]
    fn a_bit_field_offset_is_read_in_five_bits() {
        assert_eq!(bfe_read(16)[0].vertex, 1);
        // 48 is 16 and 32 is 0 for the hardware.
        assert_eq!(bfe_read(48)[0].vertex, 1);
        assert_eq!(bfe_read(32)[0].vertex, 0);
        assert_eq!(bfe_read(0)[0].vertex, 0);
    }

    /// The GS half: vertex 0's offset is the low half of v0 (`v_and_b32`
    /// with 0xffff, scaled by 4), vertex 1's the high half (`v_lshrrev_b32
    /// 16`, scaled); each read 16 bytes at +0, vertex 1 also 4 bytes at +16.
    pub fn gs_words() -> Vec<u32> {
        vec![
            vop2(V_AND, 2, 255, 0), // v_and_b32 v2, 0xffff, v0
            0xFFFF,
            vop2(V_LSHLREV, 2, 130, 2), // v_lshlrev_b32 v2, 2, v2
            ds(DS_READ_B128, 0),
            ds_regs(2, 0, 4),
            vop2(V_LSHRREV, 3, 144, 0), // v_lshrrev_b32 v3, 16, v0
            vop2(V_LSHLREV, 3, 130, 3), // v_lshlrev_b32 v3, 2, v3
            ds(DS_READ_B32, 16),
            ds_regs(3, 0, 8),
            S_ENDPGM,
        ]
    }

    #[test]
    fn the_gs_half_reads_vertices_by_the_halves_of_its_offsets() {
        let r = vertex_reads(&program(&gs_words())).unwrap();
        let at: Vec<_> = r.iter().map(|r| (r.vertex, r.dword, r.count, r.reg)).collect();
        assert_eq!(at, [(0, 0, 4, 4), (1, 4, 1, 8)]);
    }

    #[test]
    fn reads_at_other_addresses_are_the_programs_own_memory() {
        // v_lshlrev_b32 v2, 2, v1 (not an input offset), then a read.
        let r = vertex_reads(&program(&[
            vop2(V_LSHLREV, 2, 130, 5),
            ds(DS_READ_B32, 0),
            ds_regs(2, 0, 4),
            S_ENDPGM,
        ]))
        .unwrap();
        assert!(r.is_empty());
        // A fourth vertex (the high half of v1) is not a triangle's.
        let words = [
            vop2(V_LSHRREV, 3, 144, 1),
            ds(DS_READ_B32, 0),
            ds_regs(3, 0, 4),
            S_ENDPGM,
        ];
        assert!(vertex_reads(&program(&words)).is_err());
    }

    #[test]
    fn the_gs_halfs_own_memory_is_its_writes_and_reads_back() {
        // v_lshlrev_b32 v2, 2, v1 (a slot of its own), a 16-byte write at
        // +8 of v[4:7], a 4-byte read back at +12 into v9.
        let words = [
            vop2(V_LSHLREV, 2, 130, 5),
            ds(DS_WRITE_B128, 8),
            ds_regs(2, 4, 0),
            ds(DS_READ_B32, 12),
            ds_regs(2, 0, 9),
            S_ENDPGM,
        ];
        let a = own_accesses(&program(&words)).unwrap();
        assert_eq!(a.len(), 2);
        assert_eq!((a[0].write, a[0].byte, a[0].count, a[0].reg), (true, 8, 4, 4));
        assert_eq!((a[1].write, a[1].byte, a[1].count, a[1].reg), (false, 12, 1, 9));
        assert_eq!((a[0].base, a[0].mul), (a[1].base, a[1].mul));
        // An input vertex read is not among them.
        assert!(own_accesses(&program(&gs_words())).unwrap().is_empty());
    }

    #[test]
    fn the_handoff_pairs_reads_with_writes() {
        let h = handoff(&program(&es_words()), &program(&gs_words())).unwrap();
        assert_eq!(h.vertices, 2);
        assert_eq!(h.read_dwords().into_iter().collect::<Vec<_>>(), [0, 1, 2, 3, 4]);
        // A GS that reads a dword past the record: 16 bytes at +8.
        let mut far = gs_words();
        far[3] = ds(DS_READ_B128, 8);
        let err = handoff(&program(&es_words()), &program(&far)).unwrap_err();
        assert!(matches!(err, Unsupported { what, .. } if what == "a record dword the ES half never writes"));
    }
}
