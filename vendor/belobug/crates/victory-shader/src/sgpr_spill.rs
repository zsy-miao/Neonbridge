//! SGPRs the console compiler spills into VGPR lanes.
//!
//! Short of SGPRs, the compiler saves a scalar dword into one lane of a VGPR
//! with `v_writelane_b32 vN, sK, lane` and loads it back later with
//! `v_readlane_b32 sM, vN, lane` at the same constant lane. Lane masks are
//! spilled like any other dword (a Wave64 mask as two lanes), and the
//! reloaded copy is used as a mask again (`s_andn2_b64 vcc, exec, s[M:M+1]`,
//! `s_and_saveexec`, an EXEC restore). Read as written, the spill reads the
//! mask as a number and the reload is a cross-lane read whose result no
//! longer names the mask the lane-mask analysis tracks.
//!
//! Both moves only copy the dword, whatever EXEC holds, so [`forward`]
//! gives every read of a reload the spilled value itself, directly in SSA:
//! the reloaded mask is the same tracked mask. The spill and reload stay in
//! the program (a later renaming from registers, e.g. jump threading, must
//! still see them; run [`forward`] again after it), and [`dead`] names
//! those whose results nothing reads any more, which lowering leaves out.
//!
//! Based on the spill resolution in #54 by alessiodeia.

use std::collections::{HashMap, HashSet};

use crate::gfx10::{Fields, NULL, Operand};
use crate::ir::{Exit, Inst, Program, Reg, Value};

/// Block and instruction index.
pub type Site = (usize, usize);

/// A constant lane index, in the wave's lane bits (lane 67 is lane 3 in
/// Wave64).
fn const_lane(op: Operand, wave64: bool) -> Option<u32> {
    let raw = match op {
        Operand::Const(c) => crate::resource::inline(c)?,
        Operand::Literal(x) => x,
        _ => return None,
    };
    Some(raw & if wave64 { 63 } else { 31 })
}

/// A lane move without modifiers: `v_readlane_b32` or `v_writelane_b32`
/// with the VGPR operand and the lane.
fn lane_move<'a>(inst: &'a Inst, name: &str) -> Option<&'a crate::gfx10::Valu> {
    if inst.name() != name {
        return None;
    }
    let Fields::Valu(v) = &inst.fields else {
        return None;
    };
    (v.abs == 0 && v.neg == 0 && v.opsel == 0 && v.sdwa.is_none() && v.dpp.is_none()).then_some(v)
}

struct Defs {
    /// Defining instruction of each value.
    site: HashMap<Value, Site>,
    /// Block and index of each phi value.
    phi: HashMap<Value, Site>,
}

impl Defs {
    fn new(p: &Program) -> Self {
        let mut d = Defs {
            site: HashMap::new(),
            phi: HashMap::new(),
        };
        for (b, block) in p.blocks.iter().enumerate() {
            for (i, phi) in block.phis.iter().enumerate() {
                d.phi.insert(phi.value, (b, i));
            }
            for (i, inst) in block.insts.iter().enumerate() {
                for &(_, w) in &inst.writes {
                    d.site.insert(w, (b, i));
                }
            }
        }
        d
    }
}

/// The scalar value lane `lane` of VGPR value `v` holds when it was put
/// there by `v_writelane_b32` (through other lanes' writes and joins that
/// all agree). `None`: anything else, or not known.
fn spilled(
    p: &Program,
    d: &Defs,
    v: Value,
    lane: u32,
    seen: &mut Vec<Value>,
) -> Option<Option<Value>> {
    if let Some(&(b, i)) = d.phi.get(&v) {
        if seen.contains(&v) {
            // A loop back to a join being followed adds nothing.
            return Some(None);
        }
        seen.push(v);
        let mut found = None;
        for &arg in &p.blocks[b].phis[i].args {
            if let Some(x) = spilled(p, d, arg, lane, seen)? {
                if found.is_some_and(|f| f != x) {
                    return None;
                }
                found = Some(x);
            }
        }
        return Some(found);
    }
    let &(b, i) = d.site.get(&v)?;
    let inst = &p.blocks[b].insts[i];
    let w = lane_move(inst, "v_writelane_b32")?;
    let (Some(Operand::Vector { reg, .. }), [value, at]) = (w.dst, &w.src[..]) else {
        return None;
    };
    if const_lane(*at, p.wave64)? != lane {
        return spilled(p, d, inst.read(Reg::V(reg))?, lane, seen);
    }
    match *value {
        Operand::Scalar { code, .. } if code != NULL => inst.read(Reg::S(code)).map(Some),
        // Constants have no value to forward: the reload stays.
        _ => None,
    }
}

/// Gives every read of an SGPR reloaded from a VGPR lane the value spilled
/// there. Returns whether anything changed.
pub fn forward(p: &mut Program) -> bool {
    let d = Defs::new(p);
    let order = crate::ir::reverse_postorder(&p.blocks);
    let mut index = vec![usize::MAX; p.blocks.len()];
    for (n, &b) in order.iter().enumerate() {
        index[b] = n;
    }
    let idom = crate::ir::idoms(&p.blocks, &order, &index);
    let dominates = |a: usize, mut b: usize| {
        if index[a] == usize::MAX || index[b] == usize::MAX {
            return false;
        }
        while b != a {
            let up = idom[b];
            if up == b || up == usize::MAX {
                return false;
            }
            b = up;
        }
        true
    };
    // The value must be there wherever the reload is.
    let available = |x: Value, (b, i): Site| {
        if let Some(&(db, di)) = d.site.get(&x) {
            dominates(db, b) && (db != b || di < i)
        } else if let Some(&(db, _)) = d.phi.get(&x) {
            dominates(db, b)
        } else {
            p.inputs.iter().any(|&(_, v)| v == x)
        }
    };

    let mut replace: HashMap<Value, Value> = HashMap::new();
    for (b, block) in p.blocks.iter().enumerate() {
        for (i, inst) in block.insts.iter().enumerate() {
            let Some(v) = lane_move(inst, "v_readlane_b32") else {
                continue;
            };
            let (Some(Operand::Scalar { code, .. }), [Operand::Vector { reg, .. }, at]) =
                (v.dst, &v.src[..])
            else {
                continue;
            };
            let (Some(lane), Some(src), Some(out)) = (
                const_lane(*at, p.wave64),
                inst.read(Reg::V(*reg)),
                inst.write(Reg::S(code)),
            ) else {
                continue;
            };
            if let Some(Some(x)) = spilled(p, &d, src, lane, &mut Vec::new())
                && available(x, (b, i))
            {
                replace.insert(out, x);
            }
        }
    }
    if replace.is_empty() {
        return false;
    }
    // A value reloaded and spilled again: its first source.
    let find = |mut v: Value| {
        while let Some(&x) = replace.get(&v) {
            v = x;
        }
        v
    };
    let mut changed = false;
    for block in &mut p.blocks {
        for phi in &mut block.phis {
            for a in &mut phi.args {
                let x = find(*a);
                changed |= x != *a;
                *a = x;
            }
        }
        for inst in &mut block.insts {
            for r in &mut inst.reads {
                let x = find(r.1);
                changed |= x != r.1;
                r.1 = x;
            }
        }
    }
    changed
}

/// Lane moves whose results nothing reads: reloads [`forward`] replaced,
/// and the spills (VGPR lane writes) that only those reloads, other such
/// spills and joins of them read. Empty when the program hands registers
/// over to another program (an indirect exit), which may read them.
pub fn dead(p: &Program) -> HashSet<Site> {
    let mut out = HashSet::new();
    if p.blocks.iter().any(|b| b.exit == Exit::Indirect) {
        return out;
    }
    let mut read: HashSet<Value> = HashSet::new();
    for block in &p.blocks {
        for phi in &block.phis {
            read.extend(&phi.args);
        }
        for inst in &block.insts {
            read.extend(inst.reads.iter().map(|r| r.1));
        }
    }
    for (b, block) in p.blocks.iter().enumerate() {
        for (i, inst) in block.insts.iter().enumerate() {
            if lane_move(inst, "v_readlane_b32").is_some()
                && inst.writes.iter().all(|w| !read.contains(&w.1))
            {
                out.insert((b, i));
            }
        }
    }

    // VGPR values that may hold only spilled lanes: lane writes and joins.
    let mut candidates: HashSet<Value> = HashSet::new();
    // Each lane write: its site, the value it writes, the value it keeps.
    let mut spills: Vec<(Site, Value, Option<Value>)> = Vec::new();
    for (b, block) in p.blocks.iter().enumerate() {
        for phi in &block.phis {
            if matches!(phi.reg, Reg::V(_)) {
                candidates.insert(phi.value);
            }
        }
        for (i, inst) in block.insts.iter().enumerate() {
            if let Some(w) = lane_move(inst, "v_writelane_b32")
                && let Some(Operand::Vector { reg, .. }) = w.dst
                && let Some(written) = inst.write(Reg::V(reg))
            {
                candidates.insert(written);
                spills.push(((b, i), written, inst.read(Reg::V(reg))));
            }
        }
    }
    // Read by anything else than a dead reload or a lane write keeping it.
    let mut escaped: HashSet<Value> = HashSet::new();
    for (b, block) in p.blocks.iter().enumerate() {
        for (i, inst) in block.insts.iter().enumerate() {
            if out.contains(&(b, i)) {
                continue;
            }
            let kept = match lane_move(inst, "v_writelane_b32").and_then(|w| w.dst) {
                Some(Operand::Vector { reg, .. }) => Some(Reg::V(reg)),
                _ => None,
            };
            for &(r, v) in &inst.reads {
                if Some(r) != kept && candidates.contains(&v) {
                    escaped.insert(v);
                }
            }
        }
    }
    loop {
        let before = escaped.len();
        for &(_, written, kept) in &spills {
            if escaped.contains(&written)
                && let Some(k) = kept
                && candidates.contains(&k)
            {
                escaped.insert(k);
            }
        }
        for block in &p.blocks {
            for phi in &block.phis {
                if escaped.contains(&phi.value) {
                    escaped.extend(phi.args.iter().filter(|a| candidates.contains(a)));
                }
            }
        }
        if escaped.len() == before {
            break;
        }
    }
    out.extend(
        spills
            .into_iter()
            .filter(|(_, written, _)| !escaped.contains(written))
            .map(|(site, _, _)| site),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::{dead, forward};
    use crate::ir::{Program, Reg};
    use crate::native::{ComputeIo, StageIo};
    use crate::{Container, HwStage, NativeProgram, ShaderError, io, ir, resource, structure};

    fn build(words: &[u32], wave64: bool) -> Program {
        let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        ir::build(&code, wave64, 8).unwrap()
    }

    /// Translates a Wave64 compute program, with or without forwarding
    /// reloads first.
    fn translate(words: &[u32], fwd: bool) -> Result<(), ShaderError> {
        let native = NativeProgram {
            stage: HwStage::Cs,
            wave64: true,
            vgprs: 8,
            layout: vec![],
            used: vec![],
            io: StageIo::Cs(ComputeIo {
                rsrc2: 0,
                threads: [64, 1, 1],
            }),
            code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
        };
        let mut p = ir::build(&native.patched_code(), true, native.vgprs)?;
        if fwd {
            forward(&mut p);
        }
        let res = resource::track(&p, &native.layout, native.stage)?;
        let io = io::raise(&p, &native)?;
        let st = structure::structure(&p)?;
        let container = crate::dxbc::build(&[]);
        let c = Container::parse(&container).unwrap();
        crate::emit::emit(&c, &native, &p, &st, &io, &res, crate::emit::Target::ANY).map(|_| ())
    }

    /// Assembled with llvm-mc (gfx1030, wave64): an `if` whose else-mask
    /// (`s_xor` of EXEC with the saved one: lanes elsewhere) is spilled to
    /// lanes 0 and 1 of v1 while s[2:3] is reused, then reloaded into
    /// s[4:5]. `uses` follow the reload.
    fn else_mask_spilled(uses: &[u32]) -> Vec<u32> {
        let mut words = vec![
            0x7E04_0280, // v_mov_b32_e32 v2, 0
            0x7D88_0085, // v_cmp_gt_u32_e32 vcc, 5, v0
            0xBE82_246A, // s_and_saveexec_b64 s[2:3], vcc
            0x8982_027E, // s_xor_b64 s[2:3], exec, s[2:3]
            0xD761_0001,
            0x0001_0002, // v_writelane_b32 v1, s2, 0
            0xD761_0001,
            0x0001_0203, // v_writelane_b32 v1, s3, 1
            0x7E04_0281, // v_mov_b32_e32 v2, 1
            0xBE82_0480, // s_mov_b64 s[2:3], 0
            0xD760_0004,
            0x0001_0101, // v_readlane_b32 s4, v1, 0
            0xD760_0005,
            0x0001_0301, // v_readlane_b32 s5, v1, 1
        ];
        words.extend(uses);
        words
    }

    #[test]
    fn a_reloaded_mask_is_the_spilled_mask() {
        let mut p = build(
            &else_mask_spilled(&[
                0x8AEA_047E, // s_andn2_b64 vcc, exec, s[4:5]
                0xBF81_0000, // s_endpgm
            ]),
            true,
        );
        assert!(forward(&mut p));
        let insts = &p.blocks[0].insts;
        let (lo, hi) = (
            insts[3].write(Reg::S(2)).unwrap(),
            insts[3].write(Reg::S(3)).unwrap(),
        );
        // The mask operation reads the spilled dwords themselves.
        assert_eq!(insts[10].read(Reg::S(4)), Some(lo));
        assert_eq!(insts[10].read(Reg::S(5)), Some(hi));
        let st = structure::structure(&p).unwrap();
        assert_eq!(st.highs.get(&hi), Some(&lo));
        assert!(
            st.masks
                .contains_key(&insts[10].write(Reg::S(106)).unwrap())
        );
        // Spills and reloads are left out; nothing else is.
        let mut gone: Vec<_> = dead(&p).into_iter().collect();
        gone.sort_unstable();
        assert_eq!(gone, [(0, 4), (0, 5), (0, 8), (0, 9)]);
    }

    #[test]
    fn a_spilled_mask_holding_lanes_elsewhere_is_used_as_a_mask_again() {
        let idioms: [&[u32]; 4] = [
            // The else branch: EXEC from the reloaded mask.
            &[
                0xBE84_2504, // s_or_saveexec_b64 s[4:5], s[4:5]
                0x89FE_047E, // s_xor_b64 exec, exec, s[4:5]
                0xBF88_0001, // s_cbranch_execz join
                0x7E04_0282, // v_mov_b32_e32 v2, 2
                0x88FE_047E, // join: s_or_b64 exec, exec, s[4:5]
                0xBF81_0000, // s_endpgm
            ],
            &[
                0x8AEA_047E, // s_andn2_b64 vcc, exec, s[4:5]
                0xBF87_0001, // s_cbranch_vccnz end
                0x7E04_0282, // v_mov_b32_e32 v2, 2
                0xBF81_0000, // end: s_endpgm
            ],
            &[
                0x8786_7E04, // s_and_b64 s[6:7], s[4:5], exec
                0x85EA_807E, // s_cselect_b64 vcc, exec, 0
                0xBF87_0001, // s_cbranch_vccnz end
                0x7E04_0282, // v_mov_b32_e32 v2, 2
                0xBF81_0000, // end: s_endpgm
            ],
            &[
                0xBE86_2404, // s_and_saveexec_b64 s[6:7], s[4:5]
                0xBF88_0001, // s_cbranch_execz join
                0x7E04_0282, // v_mov_b32_e32 v2, 2
                0x88FE_067E, // join: s_or_b64 exec, exec, s[6:7]
                0xBF81_0000, // s_endpgm
            ],
        ];
        for uses in idioms {
            let words = else_mask_spilled(uses);
            // The spill reads the mask as a number where its lanes are not:
            // the else-mask is a set operation, whose number comes from its
            // sources' numbers (#269), so the program translates unforwarded
            // too.
            translate(&words, false).unwrap();
            translate(&words, true).unwrap();
        }
    }

    #[test]
    fn the_scc_of_a_mask_operation_is_whether_a_lane_is_set() {
        // Assembled with llvm-mc (gfx1030, wave64): `s_and` of a mask with
        // EXEC, its SCC turned into a uniform branch with `s_cselect`. Was
        // "a lane mask read as data".
        let words = [
            0x7D88_0085, // v_cmp_gt_u32_e32 vcc, 5, v0
            0xBE82_046A, // s_mov_b64 s[2:3], vcc
            0x7E04_0281, // v_mov_b32_e32 v2, 1
            0x7D88_0089, // v_cmp_gt_u32_e32 vcc, 9, v0
            0x8784_7E02, // s_and_b64 s[4:5], s[2:3], exec
            0x85EA_807E, // s_cselect_b64 vcc, exec, 0
            0xBF87_0001, // s_cbranch_vccnz end
            0x7E04_0283, // v_mov_b32_e32 v2, 3
            0xBF81_0000, // end: s_endpgm
        ];
        translate(&words, false).unwrap();
        // The same mask spilled and reloaded in between.
        let words = else_mask_spilled(&[
            0x8786_7E04, // s_and_b64 s[6:7], s[4:5], exec
            0x85EA_807E, // s_cselect_b64 vcc, exec, 0
            0xBF87_0001, // s_cbranch_vccnz end
            0x7E04_0282, // v_mov_b32_e32 v2, 2
            0xBF81_0000, // end: s_endpgm
        ]);
        translate(&words, true).unwrap();
    }

    #[test]
    fn only_the_same_lane_written_by_a_lane_write_is_forwarded() {
        // Assembled with llvm-mc (gfx1030, wave32): lane 35 is lane 3.
        let mut p = build(
            &[
                0x7D88_0085, // v_cmp_gt_u32_e32 vcc_lo, 5, v0
                0xD761_0001,
                0x0001_466A, // v_writelane_b32 v1, vcc_lo, 35
                0xD761_0001,
                0x0001_0887, // v_writelane_b32 v1, 7, 4
                0x7D88_0089, // v_cmp_gt_u32_e32 vcc_lo, 9, v0
                0xD760_0002,
                0x0001_0701, // v_readlane_b32 s2, v1, 3
                0xD760_0003,
                0x0001_0901, // v_readlane_b32 s3, v1, 4
                0xD760_0004,
                0x0001_0B01, // v_readlane_b32 s4, v1, 5
                0xBE85_0302, // s_mov_b32 s5, s2
                0xBE86_0303, // s_mov_b32 s6, s3
                0xBE87_0304, // s_mov_b32 s7, s4
                0xBF81_0000, // s_endpgm
            ],
            false,
        );
        let before = p.clone();
        assert!(forward(&mut p));
        let insts = &p.blocks[0].insts;
        // Lane 3, through the write of lane 4: the first compare.
        assert_eq!(insts[7].read(Reg::S(2)), insts[0].write(Reg::S(106)));
        // A constant has no value to forward; lane 5 was never written.
        assert_eq!(insts[8].reads, before.blocks[0].insts[8].reads);
        assert_eq!(insts[9].reads, before.blocks[0].insts[9].reads);
        // The lane writes stay: the other reloads still read v1.
        let mut gone: Vec<_> = dead(&p).into_iter().collect();
        gone.sort_unstable();
        assert_eq!(gone, [(0, 4)]);
    }

    #[test]
    fn a_reload_needs_the_value_wherever_it_runs() {
        // Assembled with llvm-mc (gfx1030, wave32): the lane is written on
        // one path only; the join keeps v1's value from before otherwise.
        let mut p = build(
            &[
                0xBF06_8000, // s_cmp_eq_u32 s0, 0
                0xBF85_0002, // s_cbranch_scc1 join
                0xD761_0001,
                0x0001_0001, // v_writelane_b32 v1, s1, 0
                0xD760_0002,
                0x0001_0101, // join: v_readlane_b32 s2, v1, 0
                0xBE83_0302, // s_mov_b32 s3, s2
                0xBF81_0000, // s_endpgm
            ],
            false,
        );
        let before = p.clone();
        assert!(!forward(&mut p));
        assert_eq!(p, before);
        assert!(dead(&p).is_empty());
    }
}
