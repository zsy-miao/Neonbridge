//! Recognize the NGG allocation and export sequence before lowering it to
//! SetMeshOutputCounts, triangle indices and vertex/primitive attributes.
use crate::rdna2::{Enc, Insn, Opnd};
use Opnd::{ConstInt as I, ExecLo, Literal, Scc, Sgpr as S, VccLo, Vgpr as V, M0};

pub(super) struct MeshExports {
    pub start: usize,
    pub lane_index: Opnd,
    pub vertices: Opnd,
    pub primitives: Opnd,
    pub connectivity: Opnd,
    pub guarded_connectivity: Opnd,
    pub connectivity_without_primitive: Opnd,
    pub vertex_exports: Vec<Insn>,
    pub primitive_exports: Vec<Insn>,
}

struct Sequence<'a> {
    code: &'a [Insn],
    at: usize,
}

fn is_wait(op: &Insn) -> bool {
    op.enc == Enc::Sopp && op.mnemonic == "s_waitcnt" && op.aux.imm & 0x80 == 0
}

impl<'a> Sequence<'a> {
    fn next(&mut self, name: &str) -> Result<&'a Insn, String> {
        // Native wait counters have no register side effects. The desktop
        // emitter orders resource reads by their dependencies.
        if name != "s_waitcnt" {
            while self.code.get(self.at).is_some_and(is_wait) {
                self.at += 1;
            }
        }
        let op = self
            .code
            .get(self.at)
            .ok_or("truncated mesh export sequence")?;
        if op.mnemonic != name
            || op.aux.sdwa.is_some()
            || (op.enc == Enc::Vop3 && (op.aux.imm != 0 || op.aux.imm2 != 0 || op.aux.mask != 0))
        {
            return Err(format!(
                "expected unmodified {name} in mesh exports at {:#x}",
                op.offset
            ));
        }
        self.at += 1;
        Ok(op)
    }

    fn assign(&mut self, name: &str, src: &[Opnd]) -> Result<Opnd, String> {
        let op = self.next(name)?;
        match op.dst[..] {
            [dst] if op.src == src => Ok(dst),
            _ => Err(format!(
                "unsupported operands for mesh {name} at {:#x}",
                op.offset
            )),
        }
    }

    fn write(&mut self, dst: Opnd, name: &str, src: &[Opnd]) -> Result<(), String> {
        if self.assign(name, src)? == dst {
            Ok(())
        } else {
            Err(format!("unexpected destination for mesh {name}"))
        }
    }

    fn imm(&mut self, name: &str, imm: i64) -> Result<&'a Insn, String> {
        let op = self.next(name)?;
        if op.aux.imm == imm {
            Ok(op)
        } else {
            Err(format!(
                "unexpected immediate for mesh {name} at {:#x}",
                op.offset
            ))
        }
    }

    fn branch(&mut self, name: &str) -> Result<usize, String> {
        let op = self.next(name)?;
        (op.offset + 4)
            .checked_add_signed(op.aux.imm as isize * 4)
            .ok_or_else(|| format!("invalid branch in mesh exports at {:#x}", op.offset))
    }

    fn arrive(&self, target: usize) -> Result<(), String> {
        if self.code.get(self.at).is_some_and(|op| op.offset == target) {
            Ok(())
        } else {
            Err(format!("unexpected mesh export branch to {target:#x}"))
        }
    }

    fn export(&mut self, target: i64, mask: u8, flags: i64) -> Result<&'a Insn, String> {
        let op = self.next("exp")?;
        if op.aux.imm == target && op.aux.mask == mask && op.aux.imm2 == flags {
            Ok(op)
        } else {
            Err(format!("unsupported mesh export at {:#x}", op.offset))
        }
    }

    fn count(&mut self) -> Result<Opnd, String> {
        let op = self.next("s_cmp_eq_u32")?;
        // Counts must survive the temporary mask and allocation registers s0..13.
        match op.src[..] {
            [n @ S(14..), I(0)] if op.dst == [Scc] => Ok(n),
            _ => Err("unsupported mesh count guard".into()),
        }
    }
}

// The shader allocates one dummy vertex and one null primitive for an invalid
// group. This is equivalent to emitting no geometry in the desktop mesh API.
fn allocation(seq: &mut Sequence<'_>) -> Result<(Opnd, Opnd, Opnd, [Opnd; 3]), String> {
    let vertices = seq.count()?;
    seq.write(VccLo, "v_cmp_eq_u32", &[I(0), V(1)])?;
    seq.write(S(0), "s_cselect_b64", &[I(-1), I(0)])?;
    let primitives = seq.count()?;
    seq.write(S(2), "s_cselect_b64", &[I(-1), I(0)])?;
    for (count, max, mask) in [(vertices, 256, S(4)), (primitives, 128, S(6))] {
        if seq.imm("s_cmpk_gt_u32", max)?.dst != [count] {
            return Err("mesh count limit uses a different register".into());
        }
        seq.write(mask, "s_cselect_b64", &[I(-1), I(0)])?;
    }
    seq.write(S(0), "s_or_b64", &[S(0), S(2)])?;
    seq.write(S(2), "s_or_b64", &[S(4), S(6)])?;
    seq.write(S(0), "s_or_b64", &[S(2), S(0)])?;
    let bad = seq.assign("v_cndmask_b32", &[I(0), I(1), S(0)])?;
    seq.write(S(0), "v_readfirstlane_b32", &[bad])?;
    seq.write(S(0), "s_and_b32", &[I(1), S(0)])?;
    seq.write(Scc, "s_cmp_eq_u32", &[S(0), I(1)])?;
    let invalid = seq.assign("s_cselect_b64", &[I(-1), I(0)])?;
    let vert = seq.assign("v_cndmask_b32", &[vertices, I(1), invalid])?;
    let prim = seq.assign("v_cndmask_b32", &[primitives, I(1), invalid])?;
    let temps = [bad, vert, prim];
    if !matches!(invalid, S(0) | S(12))
        || vert == prim
        || !temps.iter().all(|op| matches!(op, V(n) if *n != 1))
    {
        return Err("unsupported temporary registers in mesh allocation".into());
    }
    seq.write(S(6), "v_readfirstlane_b32", &[vert])?;
    seq.write(S(4), "v_readfirstlane_b32", &[prim])?;
    seq.write(S(2), "s_and_saveexec_b64", &[VccLo])?;
    let restore = seq.branch("s_cbranch_execz")?;
    seq.write(S(7), "s_lshl_b32", &[S(4), I(12)])?;
    seq.write(S(4), "s_and_b64", &[ExecLo, invalid])?;
    seq.write(S(6), "s_or_b32", &[S(7), S(6)])?;
    seq.write(VccLo, "s_andn2_b64", &[ExecLo, S(4)])?;
    seq.write(M0, "s_mov_b32", &[S(6)])?;
    seq.imm("s_sendmsg", 9)?;
    seq.imm("s_waitcnt", 0)?;
    let valid = seq.branch("s_cbranch_vccnz")?;
    seq.imm("s_barrier", 0)?;
    let null = seq.assign("v_bfrev_b32", &[I(1)])?;
    seq.export(12, 0, 2)?;
    if seq.export(20, 1, 2)?.src.first() != Some(&null) {
        return Err("missing null primitive in mesh allocation".into());
    }
    seq.arrive(valid)?;
    seq.arrive(restore)?;
    seq.imm("s_waitcnt", -241)?;
    seq.write(ExecLo, "s_mov_b64", &[S(2)])?;
    seq.imm("s_barrier", 0)?;
    Ok((vertices, primitives, invalid, temps))
}

fn allocation_128(seq: &mut Sequence<'_>) -> Result<(Opnd, Opnd, Opnd, [Opnd; 3]), String> {
    let vertices = seq.count()?;
    let limit = seq.imm("s_movk_i32", 128)?;
    let [limit @ (S(6) | S(14..))] = limit.dst[..] else {
        return Err("unsupported mesh count limit register".into());
    };
    seq.write(S(0), "s_cselect_b64", &[ExecLo, I(0)])?;
    let primitives = seq.count()?;
    if limit == vertices || limit == primitives {
        return Err("mesh limit overwrites an output count".into());
    }
    seq.write(S(2), "s_cselect_b64", &[ExecLo, I(0)])?;
    seq.write(Scc, "s_cmp_lt_u32", &[limit, vertices])?;
    seq.write(S(4), "s_cselect_b64", &[ExecLo, I(0)])?;
    seq.write(Scc, "s_cmp_lt_u32", &[limit, primitives])?;
    seq.write(S(6), "s_cselect_b64", &[ExecLo, I(0)])?;
    seq.write(S(0), "s_or_b64", &[S(0), S(2)])?;
    seq.write(S(2), "s_or_b64", &[S(4), S(6)])?;
    seq.write(S(0), "s_or_b64", &[S(2), S(0)])?;
    let bad = seq.assign("v_cndmask_b32", &[I(0), I(1), S(0)])?;
    seq.write(S(0), "v_readfirstlane_b32", &[bad])?;
    seq.write(S(0), "s_and_b32", &[I(1), S(0)])?;
    seq.write(Scc, "s_cmp_eq_u32", &[S(0), I(1)])?;
    seq.write(S(0), "s_mov_b64", &[ExecLo])?;
    let invalid = seq.assign("s_cselect_b64", &[ExecLo, I(0)])?;
    if !matches!(invalid, S(2) | S(14))
        || [vertices, primitives]
            .iter()
            .any(|reg| matches!(invalid, S(14)) && matches!(reg, S(14 | 15)))
    {
        return Err("unsupported mesh allocation mask register".into());
    }
    let vert = seq.assign("v_cndmask_b32", &[vertices, I(1), invalid])?;
    let prim = seq.assign("v_cndmask_b32", &[primitives, I(1), invalid])?;
    let temps = [bad, vert, prim];
    if vert == prim || !temps.iter().all(|op| matches!(op, V(1..))) {
        return Err("unsupported temporary registers in mesh allocation".into());
    }
    seq.write(S(4), "s_xor_b64", &[invalid, I(-1)])?;
    seq.write(S(6), "v_readfirstlane_b32", &[vert])?;
    seq.write(S(7), "v_readfirstlane_b32", &[prim])?;
    seq.write(VccLo, "v_cmpx_eq_u32", &[I(0), V(0)])?;
    let restore = seq.branch("s_cbranch_execz")?;
    seq.write(S(7), "s_lshl_b32", &[S(7), I(12)])?;
    seq.write(VccLo, "s_andn2_b64", &[ExecLo, invalid])?;
    seq.write(S(6), "s_or_b32", &[S(7), S(6)])?;
    seq.write(M0, "s_mov_b32", &[S(6)])?;
    seq.imm("s_sendmsg", 9)?;
    seq.imm("s_waitcnt", 0)?;
    let valid = seq.branch("s_cbranch_vccnz")?;
    seq.imm("s_barrier", 0)?;
    let null = seq.assign("v_bfrev_b32", &[I(1)])?;
    if null != bad {
        return Err("unexpected null primitive register".into());
    }
    seq.export(12, 0, 2)?;
    if seq.export(20, 1, 2)?.src.first() != Some(&null) {
        return Err("missing null primitive in mesh allocation".into());
    }
    seq.arrive(valid)?;
    seq.arrive(restore)?;
    seq.imm("s_waitcnt", -241)?;
    seq.write(ExecLo, "s_mov_b64", &[S(0)])?;
    seq.imm("s_barrier", 0)?;
    Ok((vertices, primitives, invalid, temps))
}

pub(super) fn recognize(
    insns: &[Insn],
    max_vertices: u16,
    clip_mask: Option<u8>,
) -> Result<MeshExports, String> {
    let compact = max_vertices == 128;
    let start = insns
        .iter()
        .position(|op| op.mnemonic == "s_sendmsg")
        .and_then(|send| {
            insns[..send]
                .iter()
                .enumerate()
                .rev()
                .filter(|(_, op)| !is_wait(op))
                .nth(if compact { 28 } else { 27 })
                .map(|(index, _)| index)
        })
        .ok_or("missing mesh allocation request")?;
    let mut seq = Sequence {
        code: insns,
        at: start,
    };
    let (vertices, primitives, invalid, temps) = if compact {
        allocation_128(&mut seq)?
    } else {
        allocation(&mut seq)?
    };
    let lane_index = V(if compact { 0 } else { 1 });
    seq.write(VccLo, "v_cmp_gt_u32", &[primitives, lane_index])?;
    if compact {
        seq.write(S(0), "s_and_b64", &[VccLo, S(4)])?;
    } else {
        seq.write(S(4), "s_xor_b64", &[invalid, I(-1)])?;
        seq.write(S(0), "s_and_b64", &[ExecLo, S(4)])?;
        seq.write(S(0), "s_and_b64", &[VccLo, S(0)])?;
    }
    seq.write(S(6), "s_and_saveexec_b64", &[S(0)])?;
    let end_primitives = seq.branch("s_cbranch_execz")?;

    // PRIM exports encode three 10-bit indices. Bit 31 marks a culled
    // primitive. The Xbox code replaces all indices with zero on overflow.
    let extract = seq.next("v_bfe_u32")?;
    let ([y], [packed @ V(_), I(10), I(10)]) = (&extract.dst[..], &extract.src[..]) else {
        return Err("unsupported mesh triangle index encoding".into());
    };
    let (y, packed) = (*y, *packed);
    let z = seq.assign("v_bfe_u32", &[packed, I(20), I(10)])?;
    let x = seq.assign("v_and_b32", &[Literal(0x3ff), packed])?;
    let indices = [x, y, z];
    if temps.contains(&packed)
        || packed == lane_index
        || indices.contains(&packed)
        || x == y
        || y == z
        || x == z
        || !indices
            .iter()
            .all(|op| matches!(op, V(_)) && *op != lane_index)
    {
        return Err("mesh triangle index register overwritten".into());
    }
    seq.write(VccLo, "v_cmp_le_u32", &[vertices, y])?;
    seq.write(S(2), "v_cmp_le_u32", &[vertices, z, S(0)])?;
    seq.write(S(0), "v_cmp_le_u32", &[vertices, x, S(0)])?;
    seq.write(S(2), "s_or_b64", &[VccLo, S(2)])?;
    seq.write(S(0), "s_or_b64", &[S(0), S(2)])?;
    let guarded = seq.assign("v_cndmask_b32", &[packed, Literal(0x80000000), S(0)])?;
    if !(if compact {
        matches!(guarded, V(1..))
    } else {
        guarded == V(0)
    }) {
        return Err("unsupported guarded triangle register".into());
    }
    if seq.export(20, 1, 2)?.src.first() != Some(&guarded) {
        return Err("missing guarded triangle export".into());
    }
    let preserved = |reg: &Opnd| !temps.contains(reg) && !indices.contains(reg) && *reg != guarded;
    let primitive = if seq.code.get(seq.at).is_some_and(|op| op.enc == Enc::Exp) {
        let export = seq.next("exp")?;
        if !(32..64).contains(&export.aux.imm)
            || export.aux.mask != 15
            || export.aux.imm2 != 0
            || !export.src.iter().all(preserved)
        {
            return Err("unsupported mesh primitive attribute export".into());
        }
        Some(export)
    } else {
        None
    };
    seq.arrive(end_primitives)?;
    seq.imm("s_waitcnt", -241)?;
    seq.write(ExecLo, "s_mov_b64", &[S(6)])?;
    seq.write(VccLo, "v_cmp_gt_u32", &[vertices, lane_index])?;
    if compact {
        seq.write(S(0), "s_and_b64", &[VccLo, S(4)])?;
    } else {
        seq.write(S(0), "s_and_b64", &[ExecLo, S(4)])?;
        seq.write(S(0), "s_and_b64", &[VccLo, S(0)])?;
    }
    seq.write(S(0), "s_and_saveexec_b64", &[S(0)])?;
    let end_vertices = seq.branch("s_cbranch_execz")?;
    let mut vertex_exports = Vec::new();
    let mut targets = 0u64;
    let mut zero_lane_register = false;
    while let Some(op) = seq.code.get(seq.at) {
        if compact
            && !zero_lane_register
            && targets & (1 << 12) != 0
            && op.mnemonic == "v_mov_b32"
            && op.enc == Enc::Vop1
            && op.dst == [lane_index]
        {
            seq.write(lane_index, "v_mov_b32", &[I(0)])?;
            zero_lane_register = true;
            continue;
        }
        if op.enc != Enc::Exp {
            break;
        }
        let export = seq.next("exp")?;
        let target = export.aux.imm;
        let valid_target = match target {
            12 => {
                export.aux.mask == 15 && export.aux.imm2 == if clip_mask.is_some() { 0 } else { 2 }
            }
            13 => {
                clip_mask.is_some_and(|mask| mask & !export.aux.mask == 0)
                    && targets & (1 << 12) != 0
                    && export.aux.imm2 == 2
            }
            32..64 => export.aux.imm2 == 0,
            _ => false,
        };
        if !valid_target
            || export.aux.mask == 0
            || !export.src.iter().enumerate().all(|(channel, reg)| {
                export.aux.mask & (1 << channel) == 0 || *reg == guarded || preserved(reg)
            })
            || targets & (1 << target) != 0
            || primitive.is_some_and(|export| target == export.aux.imm)
        {
            return Err(format!(
                "unsupported mesh vertex export at {:#x}",
                export.offset
            ));
        }
        targets |= 1 << target;
        let mut export = export.clone();
        if zero_lane_register {
            for source in &mut export.src {
                if *source == lane_index {
                    *source = I(0);
                }
            }
        }
        vertex_exports.push(export);
    }
    seq.arrive(end_vertices)?;
    seq.imm("s_endpgm", 0)?;
    if seq.at != insns.len()
        || targets & (1 << 12) == 0
        || clip_mask.is_some() && targets & (1 << 13) == 0
    {
        return Err("missing mesh position export or unexpected trailing code".into());
    }
    let connectivity_without_primitive = if temps[2] == guarded {
        primitives
    } else if temps[1] == guarded {
        vertices
    } else if temps[0] == guarded {
        I(0)
    } else {
        guarded
    };
    Ok(MeshExports {
        start,
        lane_index,
        vertices,
        primitives,
        connectivity: packed,
        guarded_connectivity: guarded,
        connectivity_without_primitive,
        vertex_exports,
        primitive_exports: primitive.cloned().into_iter().collect(),
    })
}
