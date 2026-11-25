//! Whole-wave sections (decision 0003): code between `s_or_saveexec -1`
//! (or `s_nand_saveexec 0`) and the restore of the saved mask runs in every lane of the console's
//! wave, the inactive ones included, which is how the compiler expands
//! HLSL wave reductions and prefix sums (an identity put into the inactive
//! lanes with `v_cndmask`, then DPP, `v_permlane16`, `v_permlanex16`,
//! `v_readlane`/`v_readfirstlane`, `v_writelane` and scalar ALU steps). A host lane that is elsewhere cannot run that code, so each
//! section is simulated on the console's lanes with symbolic values, and a
//! value it leaves is lowered by what it turned out to be: the reduction or
//! prefix of one operation over the lanes' (masked) inputs, which DXIL's
//! wave operations compute over the host's active lanes. No instruction
//! sequence is matched; any step the simulation does not model leaves the
//! values it writes unknown (the lowering still computes those of steps
//! that read no other lane as it would outside a section).

use std::collections::HashMap;

use crate::gfx10::{Dpp, Fields, NULL, Operand};
use crate::ir::{EXEC, Inst, Program, Reg, Value};

/// An associative operation and its identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op {
    AddU,
    AddF,
    AddF16,
    MinU,
    MaxU,
    MinI,
    MaxI,
    MinF,
    MaxF,
    Or,
    And,
    Xor,
}

impl Op {
    fn of(name: &str) -> Option<Op> {
        Some(match name {
            "v_add_nc_u32" => Op::AddU,
            // shortcut: 64-bit sums as two dwords: the low ones are taken not
            // to pass 32 bits (they are counts).
            "v_add_co_u32" | "v_add_co_ci_u32" => Op::AddU,
            "v_add_f32" => Op::AddF,
            "v_add_f16" => Op::AddF16,
            "v_min_u32" => Op::MinU,
            "v_max_u32" => Op::MaxU,
            "v_min_i32" => Op::MinI,
            "v_max_i32" => Op::MaxI,
            "v_min_f32" => Op::MinF,
            "v_max_f32" => Op::MaxF,
            "v_or_b32" => Op::Or,
            "v_and_b32" => Op::And,
            "v_xor_b32" => Op::Xor,
            _ => return None,
        })
    }

    /// The scalar ALU form, on two uniform dwords.
    fn of_scalar(name: &str) -> Option<Op> {
        Some(match name {
            "s_add_u32" | "s_add_i32" => Op::AddU,
            "s_min_u32" => Op::MinU,
            "s_max_u32" => Op::MaxU,
            "s_min_i32" => Op::MinI,
            "s_max_i32" => Op::MaxI,
            "s_and_b32" => Op::And,
            "s_or_b32" => Op::Or,
            "s_xor_b32" => Op::Xor,
            _ => return None,
        })
    }

    /// Whether `k` leaves the other operand unchanged.
    fn identity(self, k: u32) -> bool {
        match self {
            Op::AddU | Op::Or | Op::Xor | Op::MaxU => k == 0,
            Op::AddF => k == 0 || k == 0x8000_0000,
            Op::AddF16 => k == 0 || k == 0x8000,
            Op::MinU | Op::And => k == u32::MAX,
            Op::MinI => k == 0x7FFF_FFFF,
            Op::MaxI => k == 0x8000_0000,
            Op::MinF => k == 0x7F80_0000,
            Op::MaxF => k == 0xFF80_0000,
        }
    }

    /// Counting a lane twice changes the result.
    fn counts(self) -> bool {
        matches!(self, Op::AddU | Op::AddF | Op::AddF16 | Op::Xor)
    }
}

/// A lane's value inside a section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sym {
    Unknown,
    K(u32),
    /// The lane's own value of a VGPR value from before the section.
    Own(Value),
    /// A uniform value from before the section.
    Uni(Value),
    /// `op` over `lanes` of `mask ? src : ident` (`op` none: one lane).
    Red {
        op: Option<Op>,
        src: Value,
        mask: Value,
        ident: u32,
        lanes: u64,
    },
    /// 64-bit float sum over `lanes` of `mask ? (lo, hi) : 0.0`.
    RedF64 {
        lo: Value,
        hi: Value,
        mask: Value,
        lanes: u64,
        is_hi: bool,
    },
}

/// What a value a section leaves holds in every lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shape {
    K(u32),
    Value(Value),
    /// The lane's own `mask ? src : ident`.
    Masked(Value, Value, u32),
    /// `op` over every lane.
    Active(Op, Value, Value, u32),
    /// 64-bit float sum over every lane (`lo`, `hi`, `mask`, `is_hi`).
    ActiveF64(Value, Value, Value, bool),
    /// `op` over the lanes below (`inclusive`: and including) each lane.
    Prefix(Op, Value, Value, u32, bool),
}

/// The section starting at instruction `start` of a block: its restore
/// index and the shapes of the values it writes.
pub(crate) struct Section {
    pub end: usize,
    pub shapes: HashMap<Value, Shape>,
    /// What each lane holds of vector values.
    lanes: HashMap<Value, Vec<Sym>>,
}

impl Section {
    /// What lane `lane` of `v` holds, as a value any lane can make.
    pub fn lane_shape(&self, v: Value, lane: u32) -> Option<Shape> {
        let lanes = self.lanes.get(&v)?;
        let all = u64::MAX >> (64 - lanes.len().min(64));
        match *lanes.get(lane as usize)? {
            Sym::K(k) => Some(Shape::K(k)),
            Sym::Uni(x) => Some(Shape::Value(x)),
            Sym::Red {
                op: Some(op),
                src,
                mask,
                ident,
                lanes,
            } if lanes == all => Some(Shape::Active(op, src, mask, ident)),
            Sym::RedF64 {
                lo,
                hi,
                mask,
                lanes,
                is_hi,
            } if lanes == all => Some(Shape::ActiveF64(lo, hi, mask, is_hi)),
            _ => None,
        }
    }
}

/// Every whole-wave section of `block`, by start index.
pub(crate) fn sections(p: &Program, block: usize) -> HashMap<usize, Section> {
    let insts = &p.blocks[block].insts;
    let wave: u32 = if p.wave64 { 64 } else { 32 };
    let mut out = HashMap::new();
    let mut i = 0;
    while i < insts.len() {
        let inst = &insts[i];
        // EXEC becomes every lane: `s_or_saveexec -1` or `s_nand_saveexec 0`.
        let all = match inst.name() {
            "s_or_saveexec_b64" | "s_or_saveexec_b32" => Some(Operand::Const(193)),
            "s_nand_saveexec_b64" | "s_nand_saveexec_b32" => Some(Operand::Const(128)),
            _ => None,
        };
        let start = all.is_some_and(
            |k| matches!(&inst.fields, Fields::Sop { src, .. } if src.first() == Some(&k)),
        );
        let saved = match &inst.fields {
            Fields::Sop {
                dst: Some(Operand::Scalar { code, .. }),
                ..
            } if start => inst.write(Reg::S(*code)),
            _ => None,
        };
        let Some(saved) = saved else {
            i += 1;
            continue;
        };
        // The saved mask may come back from another register: copied through
        // `s_mov_b32` (such as wave32 VGPR-lane spills lowered in `ir::build`)
        // or forwarded from a reload (`crate::sgpr_spill`).
        let mut aliases = vec![saved];
        let mut end = None;
        for (j, inst) in insts.iter().enumerate().skip(i + 1) {
            if inst.write(Reg::S(EXEC)).is_some()
                && inst.reads.iter().any(|r| aliases.contains(&r.1))
            {
                end = Some(j);
                break;
            }
            if matches!(inst.name(), "s_mov_b32" | "s_mov_b64")
                && inst.reads.iter().any(|r| aliases.contains(&r.1))
                && let Some(&(_, w)) = inst.writes.first()
            {
                aliases.push(w);
            }
        }
        let Some(end) = end else {
            break;
        };
        let (shapes, lanes) = simulate(&insts[i + 1..end], wave, saved);
        out.insert(i, Section { end, shapes, lanes });
        i = end + 1;
    }
    out
}

/// The lane `l` reads with DPP, whether it reads 0 instead, and whether it
/// writes; `None` for patterns not modelled.
fn dpp_lane(d: Dpp, l: u32) -> Option<(u32, bool, bool)> {
    match d {
        Dpp::Lanes { sel, .. } => Some(((l & !7) | (sel >> (3 * (l & 7)) & 7), false, true)),
        Dpp::Row {
            ctrl,
            row_mask,
            bank_mask,
            bound_ctrl,
            ..
        } => {
            let (row, pos, n) = (l & !15, l & 15, u32::from(ctrl & 15));
            let enabled = row_mask >> (l >> 4 & 3) & 1 != 0 && bank_mask >> (l >> 2 & 3) & 1 != 0;
            let (src, inside) = match ctrl {
                0..=0xFF => ((l & !3) | (u32::from(ctrl) >> (2 * (l & 3)) & 3), true),
                0x101..=0x10F => (l + n, pos + n < 16),
                0x111..=0x11F => (l.wrapping_sub(n), pos >= n),
                0x121..=0x12F => (row | (pos.wrapping_sub(n) & 15), true),
                0x140 => (l ^ 15, true),
                0x141 => (l ^ 7, true),
                0x150..=0x15F => (row | n, true),
                0x160..=0x16F => (l ^ n, true),
                _ => return None,
            };
            match (inside, bound_ctrl) {
                (true, _) => Some((src, false, enabled)),
                (false, true) => Some((l, true, enabled)),
                (false, false) => Some((l, false, false)),
            }
        }
    }
}

/// A scalar operand: a constant, a uniform the section wrote, or one from
/// before it.
fn scalar_sym(inst: &Inst, op: &Operand, uni: &HashMap<Value, Sym>) -> Sym {
    match *op {
        Operand::Scalar { code, .. } if code != NULL => match inst.read(Reg::S(code)) {
            Some(x) => uni.get(&x).copied().unwrap_or(Sym::Uni(x)),
            None => Sym::Unknown,
        },
        Operand::Const(c) => crate::resource::inline(c).map_or(Sym::Unknown, Sym::K),
        Operand::Literal(x) => Sym::K(x),
        _ => Sym::Unknown,
    }
}

/// What lane `l` holds of `x`, read from another lane `from` (DPP,
/// `v_permlane16`, `v_readlane`): a lane's own value of an earlier VGPR is
/// that lane's only.
fn moved(x: Sym, from: u32, l: u32) -> Sym {
    match x {
        Sym::Own(_) if from != l => Sym::Unknown,
        _ => x,
    }
}

fn combine(op: Op, a: Sym, b: Sym) -> Sym {
    match (a, b) {
        (Sym::K(k), x) | (x, Sym::K(k)) if op.identity(k) => x,
        (
            Sym::Red {
                op: oa,
                src,
                mask,
                ident,
                lanes: la,
            },
            Sym::Red {
                op: ob,
                src: sb,
                mask: mb,
                ident: ib,
                lanes: lb,
            },
        ) if (src, mask, ident) == (sb, mb, ib)
            && op.identity(ident)
            && oa.is_none_or(|o| o == op)
            && ob.is_none_or(|o| o == op)
            && (!op.counts() || la & lb == 0) =>
        {
            Sym::Red {
                op: Some(op),
                src,
                mask,
                ident,
                lanes: la | lb,
            }
        }
        _ => Sym::Unknown,
    }
}

type Found = (HashMap<Value, Shape>, HashMap<Value, Vec<Sym>>);

fn simulate(insts: &[Inst], wave: u32, saved: Value) -> Found {
    let all = if wave == 64 {
        u64::MAX
    } else {
        u64::from(u32::MAX)
    };
    let mut exec = all;
    let mut vec: HashMap<Value, Vec<Sym>> = HashMap::new();
    let mut uni: HashMap<Value, Sym> = HashMap::new();
    // Masks the section derives from the saved EXEC (`s_and` with it).
    let mut masked = vec![saved];
    for inst in insts {
        let name = inst.name();
        if inst.write(Reg::S(EXEC)).is_some() {
            // Constant lane sets (prefix sums); anything else ends what
            // the simulation can follow.
            let lo = name
                .starts_with("s_mov_b")
                .then(|| constant(inst))
                .flatten();
            let Some(k) = lo else {
                return (shapes(vec.clone(), uni, wave), vec);
            };
            exec = if name == "s_mov_b64" {
                k
            } else {
                exec & !0xFFFF_FFFF | k & 0xFFFF_FFFF
            };
            continue;
        }
        if inst.write(Reg::S(EXEC + 1)).is_some() {
            let Some(k) = name
                .starts_with("s_mov_b")
                .then(|| constant(inst))
                .flatten()
            else {
                return (shapes(vec.clone(), uni, wave), vec);
            };
            exec = exec & 0xFFFF_FFFF | k << 32;
            continue;
        }
        if let Fields::Sop { dst, src, .. } = &inst.fields {
            let reads_masked = |o: &Operand| matches!(*o, Operand::Scalar { code, .. } if inst.read(Reg::S(code)).is_some_and(|v| masked.contains(&v)));
            // A mask ANDed with the saved one, or a copy of such a mask.
            if (name.starts_with("s_and_b") && src.iter().any(reads_masked)
                || name.starts_with("s_mov_b") && src.first().is_some_and(reads_masked))
                && let Some(&(_, w)) = inst.writes.iter().find(|w| matches!(w.0, Reg::S(_)))
            {
                masked.push(w);
            }
            // A uniform copied, or two combined (the halves of a Wave64
            // reduction read with `v_readlane`).
            let scalar = |k: usize| {
                src.get(k)
                    .map_or(Sym::Unknown, |o| scalar_sym(inst, o, &uni))
            };
            let result = match (name, Op::of_scalar(name)) {
                ("s_mov_b32", _) => scalar(0),
                (_, Some(op)) => combine(op, scalar(0), scalar(1)),
                _ => Sym::Unknown,
            };
            let out = match dst {
                Some(Operand::Scalar { code, .. }) if *code != NULL => inst.write(Reg::S(*code)),
                _ => None,
            };
            for &(_, w) in &inst.writes {
                uni.insert(w, if Some(w) == out { result } else { Sym::Unknown });
            }
            continue;
        }
        let Fields::Valu(v) = &inst.fields else {
            for &(_, w) in &inst.writes {
                vec.insert(w, vec![Sym::Unknown; wave as usize]);
            }
            continue;
        };
        let lanes = |op: &Operand,
                     vec: &HashMap<Value, Vec<Sym>>,
                     uni: &HashMap<Value, Sym>|
         -> Vec<Sym> {
            let one = |s: Sym| vec![s; wave as usize];
            match *op {
                Operand::Vector { reg, .. } => match inst.read(Reg::V(reg)) {
                    Some(x) => vec
                        .get(&x)
                        .cloned()
                        .unwrap_or_else(|| (0..wave).map(|_| Sym::Own(x)).collect()),
                    None => one(Sym::Unknown),
                },
                Operand::Scalar { code, .. } if code != NULL => match inst.read(Reg::S(code)) {
                    Some(x) => one(uni.get(&x).copied().unwrap_or(Sym::Uni(x))),
                    None => one(Sym::Unknown),
                },
                Operand::Const(c) => one(crate::resource::inline(c).map_or(Sym::Unknown, Sym::K)),
                Operand::Literal(x) => one(Sym::K(x)),
                _ => one(Sym::Unknown),
            }
        };
        let modifiers = v.abs != 0 || v.neg != 0 || v.clamp || v.omod != 0 || v.sdwa.is_some();
        // The lane `v_readlane_b32` reads or `v_writelane_b32` writes: the
        // low bits of a constant or of a uniform the simulation knows.
        let lane_sel = |k: usize| match v.src.get(k).map(|o| scalar_sym(inst, o, &uni)) {
            Some(Sym::K(l)) => Some(l & (wave - 1)),
            _ => None,
        };
        if matches!(name, "v_readlane_b32" | "v_readfirstlane_b32") {
            let (Some(Operand::Scalar { code, .. }), Some(src)) = (v.dst, v.src.first()) else {
                continue;
            };
            let l = if name == "v_readlane_b32" {
                lane_sel(1)
            } else {
                // The first lane EXEC holds.
                (exec != 0).then(|| exec.trailing_zeros())
            };
            let x = lanes(src, &vec, &uni);
            if let Some(w) = inst.write(Reg::S(code)) {
                // One lane's own value is no other lane's.
                let s = match l.map(|l| x[l as usize]) {
                    Some(Sym::Own(_)) | None => Sym::Unknown,
                    Some(s) => s,
                };
                uni.insert(w, s);
            }
            continue;
        }
        let Some(Operand::Vector { reg: dreg, .. }) = v.dst else {
            for &(_, w) in &inst.writes {
                uni.insert(w, Sym::Unknown);
            }
            continue;
        };
        let Some(dst) = inst.write(Reg::V(dreg)) else {
            continue;
        };
        let old = lanes(
            &Operand::Vector {
                reg: dreg,
                dwords: 1,
            },
            &vec,
            &uni,
        );
        if name == "v_add_f64" && !modifiers && v.dpp.is_none() {
            let pair_lanes = |k: usize| -> Option<(Vec<Sym>, Vec<Sym>)> {
                match *v.src.get(k)? {
                    Operand::Vector { reg, .. } => Some((
                        lanes(&Operand::Vector { reg, dwords: 1 }, &vec, &uni),
                        lanes(&Operand::Vector { reg: reg + 1, dwords: 1 }, &vec, &uni),
                    )),
                    Operand::Scalar { code, .. } if code != NULL => Some((
                        lanes(&Operand::Scalar { code, dwords: 1 }, &vec, &uni),
                        lanes(&Operand::Scalar { code: code + 1, dwords: 1 }, &vec, &uni),
                    )),
                    _ => None,
                }
            };
            let unpack = |lo: Sym, hi: Sym| -> Option<(Value, Value, Value, u64)> {
                match (lo, hi) {
                    (
                        Sym::Red { op: None, src: s_lo, mask: m_lo, ident: 0, lanes: l_lo },
                        Sym::Red { op: None, src: s_hi, mask: m_hi, ident, lanes: l_hi },
                    ) if m_lo == m_hi && l_lo == l_hi && (ident == 0 || ident == 0x8000_0000) => {
                        Some((s_lo, s_hi, m_lo, l_lo))
                    }
                    (
                        Sym::RedF64 { lo: s_lo, hi: s_hi, mask: m_lo, lanes: l_lo, is_hi: false },
                        Sym::RedF64 { lo: s_lo2, hi: s_hi2, mask: m_hi, lanes: l_hi, is_hi: true },
                    ) if (s_lo, s_hi, m_lo, l_lo) == (s_lo2, s_hi2, m_hi, l_hi) => {
                        Some((s_lo, s_hi, m_lo, l_lo))
                    }
                    _ => None,
                }
            };
            let (mut res_lo, mut res_hi) = (vec![Sym::Unknown; wave as usize], vec![Sym::Unknown; wave as usize]);
            if let (Some((a_lo, a_hi)), Some((b_lo, b_hi))) = (pair_lanes(0), pair_lanes(1)) {
                for l in 0..wave as usize {
                    if let (Some(a), Some(b)) = (unpack(a_lo[l], a_hi[l]), unpack(b_lo[l], b_hi[l]))
                        && (a.0, a.1, a.2) == (b.0, b.1, b.2)
                        && a.3 & b.3 == 0
                    {
                        let lanes = a.3 | b.3;
                        res_lo[l] = Sym::RedF64 { lo: a.0, hi: a.1, mask: a.2, lanes, is_hi: false };
                        res_hi[l] = Sym::RedF64 { lo: a.0, hi: a.1, mask: a.2, lanes, is_hi: true };
                    }
                }
            }
            let old_hi = lanes(&Operand::Vector { reg: dreg + 1, dwords: 1 }, &vec, &uni);
            let m_lo = (0..wave as usize).map(|l| if exec >> l & 1 != 0 { res_lo[l] } else { old[l] }).collect();
            let m_hi = (0..wave as usize).map(|l| if exec >> l & 1 != 0 { res_hi[l] } else { old_hi[l] }).collect();
            vec.insert(dst, m_lo);
            if let Some(dst_hi) = inst.write(Reg::V(dreg + 1)) {
                vec.insert(dst_hi, m_hi);
            }
            continue;
        }
        if name == "v_writelane_b32" {
            // The lane given takes the scalar whatever EXEC holds; the others
            // keep their value.
            let x = v
                .src
                .first()
                .map_or(Sym::Unknown, |o| scalar_sym(inst, o, &uni));
            let merged = match lane_sel(1) {
                Some(l) => {
                    let mut m = old;
                    m[l as usize] = x;
                    m
                }
                None => vec![Sym::Unknown; wave as usize],
            };
            vec.insert(dst, merged);
            continue;
        }
        // src0 as each lane fetches it (DPP), with the lanes that write.
        let src0 = v.src.first().map(|o| lanes(o, &vec, &uni));
        let mut writes = exec;
        let fetched: Option<Vec<Sym>> = match (v.dpp, &src0) {
            (Some(d), Some(s)) => {
                if exec != all {
                    vec.insert(dst, vec![Sym::Unknown; wave as usize]);
                    continue;
                }
                let mut f = Vec::with_capacity(wave as usize);
                let mut ok = true;
                for l in 0..wave {
                    match dpp_lane(d, l) {
                        Some((src, zero, on)) if src < wave => {
                            f.push(if zero {
                                Sym::K(0)
                            } else {
                                moved(s[src as usize], src, l)
                            });
                            if !on {
                                writes &= !(1u64 << l);
                            }
                        }
                        _ => ok = false,
                    }
                }
                ok.then_some(f)
            }
            (None, Some(s)) => Some(s.clone()),
            _ => None,
        };
        let src = |k: usize| v.src.get(k).map(|o| lanes(o, &vec, &uni));
        let result: Vec<Sym> = match (name, fetched) {
            _ if modifiers => vec![Sym::Unknown; wave as usize],
            ("v_mov_b32", Some(a)) => a,
            ("v_bfrev_b32", Some(a)) => a
                .into_iter()
                .map(|s| match s {
                    Sym::K(k) => Sym::K(k.reverse_bits()),
                    _ => Sym::Unknown,
                })
                .collect(),
            ("v_cndmask_b32", Some(a)) => {
                let mask = match v.src.get(2) {
                    Some(&Operand::Scalar { code, .. }) => inst.read(Reg::S(code)),
                    _ => None,
                };
                let b = src(1).unwrap_or_default();
                (0..wave as usize)
                    .map(|l| match (a[l], b.get(l).copied(), mask) {
                        (Sym::K(ident), Some(Sym::Own(x)), Some(m)) if masked.contains(&m) => {
                            Sym::Red {
                                op: None,
                                src: x,
                                mask: m,
                                ident,
                                lanes: 1 << l,
                            }
                        }
                        _ => Sym::Unknown,
                    })
                    .collect()
            }
            (n @ ("v_permlane16_b32" | "v_permlanex16_b32"), Some(a)) => {
                // Lane i of a row picks nibble i of {src2, src1}: from its
                // own row (permlane16) or from the other row of its half of
                // 32 lanes (permlanex16). Only with every lane active, or with
                // `fi` (opsel bit 0), which reads inactive lanes too.
                let sel = |k: usize| match v.src.get(k) {
                    Some(&Operand::Const(c)) => crate::resource::inline(c),
                    Some(&Operand::Literal(x)) => Some(x),
                    _ => None,
                };
                match (sel(1), sel(2)) {
                    (Some(s1), Some(s2)) if exec == all || v.opsel & 1 != 0 => {
                        let s = u64::from(s1) | u64::from(s2) << 32;
                        (0..wave)
                            .map(|l| {
                                let row = if n == "v_permlane16_b32" {
                                    l & !15
                                } else {
                                    (l & !31) | ((l & 16) ^ 16)
                                };
                                let pick = (s >> (4 * (l & 15)) & 15) as u32;
                                moved(a[(row | pick) as usize], row | pick, l)
                            })
                            .collect()
                    }
                    _ => vec![Sym::Unknown; wave as usize],
                }
            }
            ("v_sub_nc_u32", Some(a)) => {
                // A prefix sum less the lane's own term: the exclusive one.
                let b = src(1).unwrap_or_default();
                (0..wave as usize)
                    .map(|l| match (a[l], b.get(l).copied()) {
                        (
                            Sym::Red {
                                op: None | Some(Op::AddU),
                                src,
                                mask,
                                ident: 0,
                                lanes,
                            },
                            Some(Sym::Red {
                                op: None | Some(Op::AddU),
                                src: s2,
                                mask: m2,
                                ident: 0,
                                lanes: l2,
                            }),
                        ) if (src, mask) == (s2, m2) && lanes & l2 == l2 => {
                            let rest = lanes & !l2;
                            if rest == 0 {
                                Sym::K(0)
                            } else {
                                Sym::Red {
                                    op: Some(Op::AddU),
                                    src,
                                    mask,
                                    ident: 0,
                                    lanes: rest,
                                }
                            }
                        }
                        _ => Sym::Unknown,
                    })
                    .collect()
            }
            (n, Some(a)) => match (Op::of(n), src(1)) {
                (Some(op), Some(b)) => (0..wave as usize)
                    .map(|l| combine(op, a[l], b[l]))
                    .collect(),
                _ => vec![Sym::Unknown; wave as usize],
            },
            _ => vec![Sym::Unknown; wave as usize],
        };
        let merged = (0..wave as usize)
            .map(|l| {
                if writes >> l & 1 != 0 {
                    result[l]
                } else {
                    old[l]
                }
            })
            .collect();
        vec.insert(dst, merged);
    }
    (shapes(vec.clone(), uni, wave), vec)
}

fn constant(inst: &Inst) -> Option<u64> {
    let Fields::Sop { src, .. } = &inst.fields else {
        return None;
    };
    match *src.first()? {
        Operand::Const(c) => {
            let x = crate::resource::inline(c)?;
            Some(if (x as i32) < 0 {
                u64::MAX
            } else {
                u64::from(x)
            })
        }
        Operand::Literal(x) => Some(u64::from(x)),
        _ => None,
    }
}

/// The shape of each value, where every lane agrees on one.
fn shapes(
    vec: HashMap<Value, Vec<Sym>>,
    uni: HashMap<Value, Sym>,
    wave: u32,
) -> HashMap<Value, Shape> {
    let all = if wave == 64 {
        u64::MAX
    } else {
        u64::from(u32::MAX)
    };
    let mut out = HashMap::new();
    let single = |s: Sym| match s {
        Sym::K(k) => Some(Shape::K(k)),
        Sym::Own(x) | Sym::Uni(x) => Some(Shape::Value(x)),
        Sym::Red {
            op: Some(op),
            src,
            mask,
            ident,
            lanes,
        } if lanes == all => Some(Shape::Active(op, src, mask, ident)),
        Sym::RedF64 {
            lo,
            hi,
            mask,
            lanes,
            is_hi,
        } if lanes == all => Some(Shape::ActiveF64(lo, hi, mask, is_hi)),
        _ => None,
    };
    for (v, s) in uni {
        if let Some(sh) = single(s) {
            out.insert(v, sh);
        }
    }
    for (v, lanes) in vec {
        if let Some(sh) = per_lane(&lanes, all) {
            out.insert(v, sh);
        }
    }
    out
}

fn per_lane(lanes: &[Sym], all: u64) -> Option<Shape> {
    let first = lanes[0];
    let own = |l: usize, s: Sym| match s {
        Sym::Red {
            op: None,
            src,
            mask,
            ident,
            lanes,
        } if lanes == 1 << l => Some((src, mask, ident)),
        _ => None,
    };
    if let Some(m) = own(0, first)
        && lanes.iter().enumerate().all(|(l, &s)| own(l, s) == Some(m))
    {
        return Some(Shape::Masked(m.0, m.1, m.2));
    }
    if lanes.iter().all(|&s| s == first) {
        return match first {
            Sym::K(k) => Some(Shape::K(k)),
            Sym::Uni(x) | Sym::Own(x) => Some(Shape::Value(x)),
            Sym::Red {
                op: Some(op),
                src,
                mask,
                ident,
                lanes,
            } if lanes == all => Some(Shape::Active(op, src, mask, ident)),
            Sym::RedF64 {
                lo,
                hi,
                mask,
                lanes,
                is_hi,
            } if lanes == all => Some(Shape::ActiveF64(lo, hi, mask, is_hi)),
            _ => None,
        };
    }
    // Prefixes: lane l holds lanes 0..l (exclusive) or 0..=l (inclusive).
    let mut found: Option<(Op, Value, Value, u32, bool)> = None;
    for (l, &s) in lanes.iter().enumerate() {
        let below = (1u64 << l) - 1;
        let (op, src, mask, ident, set) = match s {
            Sym::Red {
                op,
                src,
                mask,
                ident,
                lanes,
            } => (op, src, mask, ident, lanes),
            // Lane 0 of an exclusive prefix holds the identity.
            Sym::K(_) if l == 0 => continue,
            _ => return None,
        };
        let inclusive = set == below | 1 << l;
        if !inclusive && set != below {
            return None;
        }
        let op = match (op, found) {
            (Some(o), _) => o,
            (None, Some(f)) => f.0,
            (None, None) => continue,
        };
        let me = (op, src, mask, ident, inclusive);
        match found {
            None => found = Some(me),
            Some(f) if f == me => {}
            _ => return None,
        }
    }
    let (op, src, mask, ident, inclusive) = found?;
    if let Sym::K(k) = lanes[0]
        && (inclusive || k != ident)
    {
        return None;
    }
    Some(Shape::Prefix(op, src, mask, ident, inclusive))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpp_lanes_follow_the_isa() {
        // row_shr:1 with bound_ctrl: lane 16 reads 0, lane 17 lane 16.
        let shr = Dpp::Row {
            ctrl: 0x111,
            row_mask: 0xF,
            bank_mask: 0xF,
            bound_ctrl: true,
            fi: false,
        };
        assert_eq!(dpp_lane(shr, 16), Some((16, true, true)));
        assert_eq!(dpp_lane(shr, 17), Some((16, false, true)));
        // row_mirror: lane 3 reads lane 12.
        let mirror = Dpp::Row {
            ctrl: 0x140,
            row_mask: 0xF,
            bank_mask: 0xF,
            bound_ctrl: true,
            fi: false,
        };
        assert_eq!(dpp_lane(mirror, 3), Some((12, false, true)));
        // dpp8 [1,0,3,2,5,4,7,6] swaps neighbours.
        let swap = Dpp::Lanes {
            sel: 1 | 3 << 6 | 2 << 9 | 5 << 12 | 4 << 15 | 7 << 18 | 6 << 21,
            fi: false,
        };
        assert_eq!(dpp_lane(swap, 8), Some((9, false, true)));
    }

    #[test]
    fn prefixes_are_recognised() {
        let (x, m) = (Value(1), Value(2));
        let red = |lanes: u64| Sym::Red {
            op: Some(Op::AddU),
            src: x,
            mask: m,
            ident: 0,
            lanes,
        };
        let inclusive: Vec<Sym> = (0..64).map(|l| red(u64::MAX >> (63 - l))).collect();
        assert_eq!(
            per_lane(&inclusive, u64::MAX),
            Some(Shape::Prefix(Op::AddU, x, m, 0, true))
        );
        let mut exclusive: Vec<Sym> = (0..64).map(|l| red((1u64 << l) - 1)).collect();
        exclusive[0] = Sym::K(0);
        assert_eq!(
            per_lane(&exclusive, u64::MAX),
            Some(Shape::Prefix(Op::AddU, x, m, 0, false))
        );
        assert_eq!(
            per_lane(&vec![red(u64::MAX); 64], u64::MAX),
            Some(Shape::Active(Op::AddU, x, m, 0))
        );
    }

    /// Assembled with llvm-mc (gfx1030, wave32): a sum over all 32 lanes as
    /// the console expands it, a DPP reduction of each row of 16, then each
    /// row total read from lane 15 of its own row and of the other row.
    const ROW_SUM_WORDS: [u32; 15] = [
        0xBE80_3DC1, // s_or_saveexec_b32 s0, -1
        0xD501_0001,
        0x0002_0080, // v_cndmask_b32_e64 v1, 0, v0, s0
        0x4A02_02FA,
        0xFF01_6101, // v_add_nc_u32_dpp v1, v1, v1 row_xmask:1
        0x4A02_02FA,
        0xFF01_6201, // v_add_nc_u32_dpp v1, v1, v1 row_xmask:2
        0x4A02_02FA,
        0xFF01_6401, // v_add_nc_u32_dpp v1, v1, v1 row_xmask:4
        0x4A02_02FA,
        0xFF01_6801, // v_add_nc_u32_dpp v1, v1, v1 row_xmask:8
        0xD777_0002,
        0x0305_8301, // v_permlane16_b32 v2, v1, -1, -1
        0xD778_0003,
        0x0305_8302, // v_permlanex16_b32 v3, v2, -1, -1
    ];

    #[test]
    fn permlane16_reads_the_lanes_own_row() {
        let mut words = ROW_SUM_WORDS.to_vec();
        words.extend([
            0x4A08_0702, // v_add_nc_u32_e32 v4, v2, v3
            0xBEFE_0300, // s_mov_b32 exec_lo, s0
            0xBF81_0000, // s_endpgm
        ]);
        let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let p = crate::ir::build(&code, false, 8).unwrap();
        let insts = &p.blocks[0].insts;
        let (saved, x) = (
            insts[0].write(Reg::S(0)).unwrap(),
            insts[1].read(Reg::V(0)).unwrap(),
        );
        let total = insts[8].write(Reg::V(4)).unwrap();
        let found = sections(&p, 0);
        assert_eq!(
            found[&0].shapes.get(&total),
            Some(&Shape::Active(Op::AddU, x, saved, 0))
        );
    }

    fn build(words: &[u32], wave64: bool) -> Program {
        let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        crate::ir::build(&code, wave64, 8).unwrap()
    }

    #[test]
    fn wave64_halves_combine_in_a_scalar_add() {
        // Assembled with llvm-mc (gfx1030, wave64): each half of 32 lanes
        // summed with DPP and `v_permlanex16`, then lanes 0 and 32 read
        // and added with `s_add_u32`.
        let p = build(
            &[
                0xBE80_25C1, // s_or_saveexec_b64 s[0:1], -1
                0xD501_0001,
                0x0002_0080, // v_cndmask_b32_e64 v1, 0, v0, s[0:1]
                0x4A02_02FA,
                0xFF01_6101, // v_add_nc_u32_dpp v1, v1, v1 row_xmask:1
                0x4A02_02FA,
                0xFF01_6201, // v_add_nc_u32_dpp v1, v1, v1 row_xmask:2
                0x4A02_02FA,
                0xFF01_6401, // v_add_nc_u32_dpp v1, v1, v1 row_xmask:4
                0x4A02_02FA,
                0xFF01_6801, // v_add_nc_u32_dpp v1, v1, v1 row_xmask:8
                0xD778_0002,
                0x0305_8301, // v_permlanex16_b32 v2, v1, -1, -1
                0x4A02_0501, // v_add_nc_u32_e32 v1, v1, v2
                0xD760_0002,
                0x0001_0101, // v_readlane_b32 s2, v1, 0
                0xD760_0003,
                0x0001_4101, // v_readlane_b32 s3, v1, 32
                0x8002_0302, // s_add_u32 s2, s2, s3
                0xBEFE_0400, // s_mov_b64 exec, s[0:1]
                0xBF81_0000, // s_endpgm
            ],
            true,
        );
        let insts = &p.blocks[0].insts;
        let (saved, x) = (
            insts[0].write(Reg::S(0)).unwrap(),
            insts[1].read(Reg::V(0)).unwrap(),
        );
        let found = sections(&p, 0);
        let shapes = &found[&0].shapes;
        // Each read alone is a half.
        assert_eq!(shapes.get(&insts[8].write(Reg::S(2)).unwrap()), None);
        let total = insts[10].write(Reg::S(2)).unwrap();
        assert_eq!(
            shapes.get(&total),
            Some(&Shape::Active(Op::AddU, x, saved, 0))
        );
    }

    #[test]
    fn writelane_carries_a_row_total_into_an_exclusive_prefix() {
        // Assembled with llvm-mc (gfx1030, wave32): an inclusive prefix sum
        // (row_shr steps, then lane 15 brought to the second row with
        // `v_permlanex16` and a row-masked DPP move), shifted right by one
        // lane: row_shr:1 within rows, lane 15 to lane 16 with
        // `v_readlane`/`v_writelane`.
        let p = build(
            &[
                0xBE80_3DC1, // s_or_saveexec_b32 s0, -1
                0xD501_0001,
                0x0002_0080, // v_cndmask_b32_e64 v1, 0, v0, s0
                0x4A02_02FA,
                0xFF01_1101, // v_add_nc_u32_dpp v1, v1, v1 row_shr:1
                0x4A02_02FA,
                0xFF01_1201, // v_add_nc_u32_dpp v1, v1, v1 row_shr:2
                0x4A02_02FA,
                0xFF01_1401, // v_add_nc_u32_dpp v1, v1, v1 row_shr:4
                0x4A02_02FA,
                0xFF01_1801, // v_add_nc_u32_dpp v1, v1, v1 row_shr:8
                0xD778_0002,
                0x0305_8301, // v_permlanex16_b32 v2, v1, -1, -1
                0x7E06_0280, // v_mov_b32_e32 v3, 0
                0x7E06_02FA,
                0xAF00_E402, // v_mov_b32_dpp v3, v2 quad_perm:[0,1,2,3] row_mask:0xa
                0x4A02_0701, // v_add_nc_u32_e32 v1, v1, v3
                0x7E08_0280, // v_mov_b32_e32 v4, 0
                0x7E08_02FA,
                0xFF01_1101, // v_mov_b32_dpp v4, v1 row_shr:1
                0xD760_0001,
                0x0001_1F01, // v_readlane_b32 s1, v1, 15
                0xD761_0004,
                0x0001_2001, // v_writelane_b32 v4, s1, 16
                0xBEFE_0300, // s_mov_b32 exec_lo, s0
                0xBF81_0000, // s_endpgm
            ],
            false,
        );
        let insts = &p.blocks[0].insts;
        let (saved, x) = (
            insts[0].write(Reg::S(0)).unwrap(),
            insts[1].read(Reg::V(0)).unwrap(),
        );
        let found = sections(&p, 0);
        let shapes = &found[&0].shapes;
        let inclusive = insts[9].write(Reg::V(1)).unwrap();
        assert_eq!(
            shapes.get(&inclusive),
            Some(&Shape::Prefix(Op::AddU, x, saved, 0, true))
        );
        let exclusive = insts[13].write(Reg::V(4)).unwrap();
        assert_eq!(
            shapes.get(&exclusive),
            Some(&Shape::Prefix(Op::AddU, x, saved, 0, false))
        );
    }

    #[test]
    fn a_section_restores_exec_from_a_reload_of_the_saved_mask() {
        // Assembled with llvm-mc (gfx1030, wave32): the saved EXEC spilled
        // to lane 2 of v5, s0 reused, and EXEC restored from its reload.
        let mut p = build(
            &[
                0xBE80_3DC1, // s_or_saveexec_b32 s0, -1
                0xD761_0005,
                0x0001_0400, // v_writelane_b32 v5, s0, 2
                0xD501_0001,
                0x0002_0080, // v_cndmask_b32_e64 v1, 0, v0, s0
                0xBE80_0380, // s_mov_b32 s0, 0
                0x4A02_02FA,
                0xFF01_6101, // v_add_nc_u32_dpp v1, v1, v1 row_xmask:1
                0xD760_0001,
                0x0001_0505, // v_readlane_b32 s1, v5, 2
                0xBEFE_0301, // s_mov_b32 exec_lo, s1
                0xBF81_0000, // s_endpgm
            ],
            false,
        );
        crate::sgpr_spill::forward(&mut p);
        let found = sections(&p, 0);
        assert_eq!(found[&0].end, 6);
    }

    #[test]
    fn another_lanes_own_value_is_not_the_lanes_own() {
        // A VGPR from before the section moved one lane over with DPP, or
        // read from lane 3: no lane's own value.
        let p = build(
            &[
                0xBE80_3DC1, // s_or_saveexec_b32 s0, -1
                0x7E02_02FA,
                0xFF09_1100, // v_mov_b32_dpp v1, v0 row_shr:1 bound_ctrl:1
                0xD760_0001,
                0x0001_0700, // v_readlane_b32 s1, v0, 3
                0xBEFE_0300, // s_mov_b32 exec_lo, s0
                0xBF81_0000, // s_endpgm
            ],
            false,
        );
        let insts = &p.blocks[0].insts;
        let found = sections(&p, 0);
        let shapes = &found[&0].shapes;
        assert_eq!(shapes.get(&insts[1].write(Reg::V(1)).unwrap()), None);
        assert_eq!(shapes.get(&insts[2].write(Reg::S(1)).unwrap()), None);
    }
}
