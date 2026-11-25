//! Static requirements for emulating a logical wave64 mesh program on wave32 hardware.
//!
//! This module only describes the required operations. It does not enable a lowering.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::gpu::rdna2::{self, Decoded, F_CONDITIONAL, F_TERMINATOR};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum BranchMask {
    ExecZero,
    ExecNonZero,
    VccZero,
    VccNonZero,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaskBranch {
    pub pc: u64,
    pub target: u64,
    pub mask: BranchMask,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvalidBranchTargetReason {
    Misaligned,
    OutsideProgram,
    NotInstructionStart,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidBranchTarget {
    pub pc: u64,
    pub target: u64,
    pub reason: InvalidBranchTargetReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LaneReadKind {
    ReadFirst,
    ReadLane,
    WriteLane,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LaneValueProvenance {
    Entry,
    Memory,
    MaskSelect,
    Move,
    PrefixCount,
    Arithmetic,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaneRead {
    pub pc: u64,
    pub kind: LaneReadKind,
    pub source_vgpr: Option<u64>,
    /// Category of the closest preceding lexical definition. A lowering must
    /// still preserve the program's control-flow reaching definition.
    pub provenance: LaneValueProvenance,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MeshWaveReport {
    pub instructions: usize,
    pub decoded_bytes: usize,
    pub undecoded_at: Option<u64>,
    pub invalid_branch_targets: Vec<InvalidBranchTarget>,
    pub whole_mask_branches: Vec<MaskBranch>,
    pub mask_scc_branches: Vec<u64>,
    pub saveexec: Vec<u64>,
    pub wrexec: Vec<u64>,
    pub mbcnt_lo: Vec<u64>,
    pub mbcnt_hi: Vec<u64>,
    pub lane_reads: Vec<LaneRead>,
    pub allocation_messages: Vec<u64>,
    pub barriers: Vec<u64>,
    pub exports: Vec<u64>,
    pub backedges: Vec<(u64, u64)>,
    pub loop_headers: BTreeSet<u64>,
}

fn invalid_branch_target_reason(
    target: u64,
    decoded_bytes: usize,
    pcs: &BTreeMap<u64, usize>,
) -> Option<InvalidBranchTargetReason> {
    if target & 3 != 0 {
        Some(InvalidBranchTargetReason::Misaligned)
    } else if target >= decoded_bytes as u64 {
        Some(InvalidBranchTargetReason::OutsideProgram)
    } else if !pcs.contains_key(&target) {
        Some(InvalidBranchTargetReason::NotInstructionStart)
    } else {
        None
    }
}

#[derive(Clone, PartialEq, Eq)]
struct MaskState {
    sgpr: [bool; 128],
    scc: bool,
}

impl MaskState {
    fn entry() -> Self {
        let mut state = Self {
            sgpr: [false; 128],
            scc: false,
        };
        for register in [106usize, 107, 126, 127] {
            state.sgpr[register] = true;
        }
        state
    }

    fn merge(&mut self, other: &Self) -> bool {
        let before = self.clone();
        for (left, right) in self.sgpr.iter_mut().zip(other.sgpr) {
            *left |= right;
        }
        self.scc |= other.scc;
        *self != before
    }
}

fn writes_scc(decoded: &Decoded) -> bool {
    decoded
        .ienc
        .operands
        .iter()
        .any(|operand| operand.output && operand.ty.contains("SCC"))
}

fn transfer(decoded: &Decoded, mut state: MaskState) -> MaskState {
    let (reads, writes) = rdna2::sgpr_uses(decoded);
    let reads_mask = reads
        .iter()
        .any(|&register| state.sgpr.get(register as usize).copied().unwrap_or(false));
    // a v_cmp result is a lane mask too (ANDed with EXEC), also in an SGPR pair
    let implicit_exec = decoded.inst.name.contains("SAVEEXEC")
        || decoded.inst.name.contains("WREXEC")
        || decoded.inst.name.starts_with("V_CMP");
    // Explicit destinations are definite overwrites. Taint is retained only
    // when a value is computed from a mask-derived input (including EXEC).
    for &register in &writes {
        if let Some(taint) = state.sgpr.get_mut(register as usize) {
            *taint = false;
        }
    }
    if reads_mask || implicit_exec {
        for &register in &writes {
            if let Some(taint) = state.sgpr.get_mut(register as usize) {
                *taint = true;
            }
        }
    }
    // EXEC and VCC remain masks even when replaced.
    for register in [106usize, 107, 126, 127] {
        state.sgpr[register] = true;
    }
    if writes_scc(decoded) {
        state.scc = reads_mask || implicit_exec;
    }
    state
}

fn successors(decoded: &Decoded, next_pc: Option<u64>, pcs: &BTreeMap<u64, usize>) -> Vec<usize> {
    let mut out = Vec::new();
    if let Some(target_pc) = rdna2::branch_target(decoded) {
        if let Some(&target) = pcs.get(&target_pc) {
            out.push(target);
        }
        if decoded.inst.flags & F_CONDITIONAL != 0 {
            if let Some(next) = next_pc.and_then(|pc| pcs.get(&pc).copied()) {
                out.push(next);
            }
        }
    } else if decoded.inst.flags & F_TERMINATOR == 0 {
        if let Some(next) = next_pc.and_then(|pc| pcs.get(&pc).copied()) {
            out.push(next);
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

fn provenance(writer: Option<&str>) -> LaneValueProvenance {
    let Some(writer) = writer else {
        return LaneValueProvenance::Entry;
    };
    if writer.contains("LOAD") {
        LaneValueProvenance::Memory
    } else if writer == "V_CNDMASK_B32" {
        LaneValueProvenance::MaskSelect
    } else if writer.starts_with("V_MOV_B32") {
        LaneValueProvenance::Move
    } else if writer.starts_with("V_MBCNT") {
        LaneValueProvenance::PrefixCount
    } else {
        LaneValueProvenance::Arithmetic
    }
}

pub fn analyze(code: &[u8]) -> MeshWaveReport {
    let mut decoded = Vec::new();
    let mut offset = 0usize;
    let mut undecoded_at = None;
    while offset + 4 <= code.len() {
        let instruction = match rdna2::decode(&code[offset..], offset as u64) {
            Ok(instruction) => instruction,
            Err(_) => {
                undecoded_at = Some(offset as u64);
                break;
            }
        };
        offset += instruction.len;
        decoded.push(instruction);
    }
    if undecoded_at.is_none() && offset != code.len() {
        undecoded_at = Some(offset as u64);
    }
    rdna2::resolve_far_jumps(&mut decoded);
    let pcs: BTreeMap<u64, usize> = decoded
        .iter()
        .enumerate()
        .map(|(index, d)| (d.pc, index))
        .collect();
    // Inspect every decoded direct branch, including unreachable instructions.
    // A malformed target must not disappear merely because it cannot become a
    // CFG successor.
    let invalid_branch_targets = decoded
        .iter()
        .filter_map(|instruction| {
            let target = rdna2::branch_target(instruction)?;
            let reason = invalid_branch_target_reason(target, offset, &pcs)?;
            Some(InvalidBranchTarget {
                pc: instruction.pc,
                target,
                reason,
            })
        })
        .collect();

    // Forward may-analysis over the real CFG. A definite overwrite kills old
    // taint on each path; a join retains taint from any predecessor.
    let mut states: Vec<Option<MaskState>> = vec![None; decoded.len()];
    let mut queue = VecDeque::new();
    if !decoded.is_empty() {
        states[0] = Some(MaskState::entry());
        queue.push_back(0usize);
    }
    while let Some(index) = queue.pop_front() {
        let output = transfer(&decoded[index], states[index].clone().unwrap());
        let next_pc = decoded.get(index + 1).map(|d| d.pc);
        for successor in successors(&decoded[index], next_pc, &pcs) {
            let changed = match &mut states[successor] {
                Some(state) => state.merge(&output),
                slot @ None => {
                    *slot = Some(output.clone());
                    true
                }
            };
            if changed {
                queue.push_back(successor);
            }
        }
    }

    let mut report = MeshWaveReport {
        instructions: decoded.len(),
        decoded_bytes: offset,
        undecoded_at,
        invalid_branch_targets,
        ..Default::default()
    };
    let mut last_vgpr_writer: BTreeMap<u64, &'static str> = BTreeMap::new();
    for (index, instruction) in decoded.iter().enumerate() {
        let name = instruction.inst.name;
        let target = rdna2::branch_target(instruction);
        let mask = match name {
            "S_CBRANCH_EXECZ" => Some(BranchMask::ExecZero),
            "S_CBRANCH_EXECNZ" => Some(BranchMask::ExecNonZero),
            "S_CBRANCH_VCCZ" => Some(BranchMask::VccZero),
            "S_CBRANCH_VCCNZ" => Some(BranchMask::VccNonZero),
            _ => None,
        };
        if let (Some(mask), Some(target)) = (mask, target) {
            report.whole_mask_branches.push(MaskBranch {
                pc: instruction.pc,
                target,
                mask,
            });
        }
        if matches!(name, "S_CBRANCH_SCC0" | "S_CBRANCH_SCC1")
            && states[index].as_ref().is_some_and(|state| state.scc)
        {
            report.mask_scc_branches.push(instruction.pc);
        }
        if name.contains("SAVEEXEC") {
            report.saveexec.push(instruction.pc);
        }
        if name.contains("WREXEC") {
            report.wrexec.push(instruction.pc);
        }
        match name {
            "V_MBCNT_LO_U32_B32" => report.mbcnt_lo.push(instruction.pc),
            "V_MBCNT_HI_U32_B32" => report.mbcnt_hi.push(instruction.pc),
            "S_BARRIER" => report.barriers.push(instruction.pc),
            "EXP" => report.exports.push(instruction.pc),
            "S_SENDMSG"
                if rdna2::field_value(instruction.enc, &instruction.words, "SIMM16")
                    .unwrap_or(0)
                    & 0xf
                    == 9 =>
            {
                report.allocation_messages.push(instruction.pc)
            }
            _ => {}
        }
        let kind = match name {
            "V_READFIRSTLANE_B32" => Some(LaneReadKind::ReadFirst),
            "V_READLANE_B32" => Some(LaneReadKind::ReadLane),
            "V_WRITELANE_B32" => Some(LaneReadKind::WriteLane),
            _ => None,
        };
        let (vgpr_reads, vgpr_writes) = rdna2::vgpr_uses(instruction);
        if let Some(kind) = kind {
            let source_vgpr = vgpr_reads.first().copied();
            let source = source_vgpr.and_then(|register| last_vgpr_writer.get(&register).copied());
            report.lane_reads.push(LaneRead {
                pc: instruction.pc,
                kind,
                source_vgpr,
                provenance: provenance(source),
            });
        }
        for register in vgpr_writes {
            last_vgpr_writer.insert(register, name);
        }
        if let Some(target) = target.filter(|&target| target <= instruction.pc) {
            report.backedges.push((instruction.pc, target));
            report.loop_headers.insert(target);
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::rdna2::encode;

    fn one(name: &str, encoding: &str, fields: &[(&str, u64)]) -> Vec<u8> {
        encode(name, encoding, fields, None).unwrap()
    }

    #[test]
    fn mask_taint_propagates_to_scc_branches() {
        let mut code = one("S_MOV_B64", "ENC_SOP1", &[("SDST", 8), ("SSRC0", 126)]);
        code.extend(one(
            "S_AND_B64",
            "ENC_SOP2",
            &[("SDST", 10), ("SSRC0", 8), ("SSRC1", 106)],
        ));
        code.extend(one("S_CBRANCH_SCC1", "ENC_SOPP", &[("SIMM16", 0)]));
        assert_eq!(analyze(&code).mask_scc_branches, vec![8]);
    }

    #[test]
    fn vcmp_sgpr_destination_is_a_mask() {
        let mut code = one(
            "V_CMP_GT_U32",
            "ENC_VOP3",
            &[("VDST", 0), ("SRC0", 168), ("SRC1", 256)],
        );
        code.extend(one("S_CMP_EQ_U32", "ENC_SOPC", &[("SSRC0", 0), ("SSRC1", 128)]));
        code.extend(one("S_CBRANCH_SCC1", "ENC_SOPP", &[("SIMM16", 0)]));
        assert_eq!(analyze(&code).mask_scc_branches, vec![12]);
    }

    #[test]
    fn definite_overwrites_kill_register_and_scc_taint() {
        let mut code = one("S_MOV_B64", "ENC_SOP1", &[("SDST", 8), ("SSRC0", 126)]);
        code.extend(one("S_MOV_B64", "ENC_SOP1", &[("SDST", 8), ("SSRC0", 128)]));
        code.extend(one(
            "S_AND_B64",
            "ENC_SOP2",
            &[("SDST", 10), ("SSRC0", 8), ("SSRC1", 8)],
        ));
        code.extend(one("S_CBRANCH_SCC1", "ENC_SOPP", &[("SIMM16", 0)]));
        code.extend(one(
            "S_CMP_EQ_U32",
            "ENC_SOPC",
            &[("SSRC0", 10), ("SSRC1", 10)],
        ));
        code.extend(one("S_CBRANCH_SCC0", "ENC_SOPP", &[("SIMM16", 0)]));
        assert!(analyze(&code).mask_scc_branches.is_empty());
    }

    #[test]
    fn reports_backedges_and_loop_headers() {
        let mut code = one("S_NOP", "ENC_SOPP", &[("SIMM16", 0)]);
        code.extend(one("S_BRANCH", "ENC_SOPP", &[("SIMM16", 0xfffe)]));
        let report = analyze(&code);
        assert_eq!(report.backedges, vec![(4, 0)]);
        assert_eq!(report.loop_headers, BTreeSet::from([0]));
    }

    #[test]
    fn accepts_forward_and_backward_instruction_targets() {
        let mut code = one("S_BRANCH", "ENC_SOPP", &[("SIMM16", 1)]);
        code.extend(one("S_NOP", "ENC_SOPP", &[("SIMM16", 0)]));
        code.extend(one("S_BRANCH", "ENC_SOPP", &[("SIMM16", 0xfffd)]));
        let report = analyze(&code);
        assert!(report.invalid_branch_targets.is_empty());
        assert_eq!(report.backedges, vec![(8, 0)]);
        assert_eq!(report.loop_headers, BTreeSet::from([0]));
    }

    #[test]
    fn rejects_target_into_literal_word() {
        let mut code = one("S_BRANCH", "ENC_SOPP", &[("SIMM16", 1)]);
        code.extend(
            encode(
                "V_MOV_B32",
                "ENC_VOP1",
                &[("VDST", 0), ("SRC0", 255)],
                Some(0x1234_5678),
            )
            .unwrap(),
        );
        assert_eq!(
            analyze(&code).invalid_branch_targets,
            vec![InvalidBranchTarget {
                pc: 0,
                target: 8,
                reason: InvalidBranchTargetReason::NotInstructionStart,
            }]
        );
        assert!(crate::gpu::recomp::geometry_wave32_hazards(&code)
            .contains("invalid geometry branch target"));
    }

    #[test]
    fn classifies_misaligned_and_outside_targets() {
        let pcs = BTreeMap::from([(0, 0), (4, 1)]);
        assert_eq!(
            invalid_branch_target_reason(2, 8, &pcs),
            Some(InvalidBranchTargetReason::Misaligned)
        );

        let mut code = one("S_BRANCH", "ENC_SOPP", &[("SIMM16", 0xfffe)]);
        code.extend(one("S_BRANCH", "ENC_SOPP", &[("SIMM16", 1)]));
        let report = analyze(&code);
        assert_eq!(report.invalid_branch_targets.len(), 2);
        assert!(report
            .invalid_branch_targets
            .iter()
            .all(|target| target.reason == InvalidBranchTargetReason::OutsideProgram));
        assert_eq!(report.invalid_branch_targets[0].pc, 0);
        assert_eq!(
            report.invalid_branch_targets[0],
            InvalidBranchTarget {
                pc: 0,
                target: u64::MAX - 3,
                reason: InvalidBranchTargetReason::OutsideProgram,
            }
        );
        assert_eq!(
            report.invalid_branch_targets[1],
            InvalidBranchTarget {
                pc: 4,
                target: 12,
                reason: InvalidBranchTargetReason::OutsideProgram,
            }
        );
    }

    #[test]
    fn reports_invalid_branch_even_when_it_is_unreachable() {
        let mut code = one("S_BRANCH", "ENC_SOPP", &[("SIMM16", 0)]);
        code.extend(one("S_ENDPGM", "ENC_SOPP", &[]));
        code.extend(one("S_BRANCH", "ENC_SOPP", &[("SIMM16", 1)]));
        assert_eq!(
            analyze(&code).invalid_branch_targets,
            vec![InvalidBranchTarget {
                pc: 8,
                target: 16,
                reason: InvalidBranchTargetReason::OutsideProgram,
            }]
        );
    }

    #[test]
    fn cfg_join_retains_taint_from_either_predecessor() {
        let mut code = one("S_CBRANCH_SCC0", "ENC_SOPP", &[("SIMM16", 2)]);
        code.extend(one("S_MOV_B64", "ENC_SOP1", &[("SDST", 8), ("SSRC0", 126)]));
        code.extend(one("S_BRANCH", "ENC_SOPP", &[("SIMM16", 1)]));
        code.extend(one("S_MOV_B64", "ENC_SOP1", &[("SDST", 8), ("SSRC0", 128)]));
        code.extend(one(
            "S_AND_B64",
            "ENC_SOP2",
            &[("SDST", 10), ("SSRC0", 8), ("SSRC1", 8)],
        ));
        code.extend(one("S_CBRANCH_SCC1", "ENC_SOPP", &[("SIMM16", 0)]));
        assert_eq!(analyze(&code).mask_scc_branches, vec![20]);
    }

    #[test]
    fn loop_fixed_point_propagates_mask_taint() {
        let mut code = one("S_MOV_B64", "ENC_SOP1", &[("SDST", 8), ("SSRC0", 128)]);
        code.extend(one("S_CBRANCH_SCC0", "ENC_SOPP", &[("SIMM16", 2)]));
        code.extend(one(
            "S_AND_B64",
            "ENC_SOP2",
            &[("SDST", 10), ("SSRC0", 8), ("SSRC1", 8)],
        ));
        code.extend(one("S_CBRANCH_SCC1", "ENC_SOPP", &[("SIMM16", 0)]));
        code.extend(one("S_MOV_B64", "ENC_SOP1", &[("SDST", 8), ("SSRC0", 126)]));
        code.extend(one("S_BRANCH", "ENC_SOPP", &[("SIMM16", 0xfffb)]));
        let report = analyze(&code);
        assert_eq!(report.mask_scc_branches, vec![4, 12]);
        assert_eq!(report.backedges, vec![(20, 4)]);
    }

    #[test]
    fn reports_operation_counts_and_lane_provenance() {
        let mut code = one("V_MOV_B32", "ENC_VOP1", &[("VDST", 2), ("SRC0", 128)]);
        code.extend(one(
            "V_READFIRSTLANE_B32",
            "ENC_VOP1",
            &[("VDST", 4), ("SRC0", 258)],
        ));
        code.extend(one(
            "V_READLANE_B32",
            "ENC_VOP3",
            &[("VDST", 5), ("SRC0", 258), ("SRC1", 0)],
        ));
        code.extend(one(
            "V_MBCNT_LO_U32_B32",
            "ENC_VOP3",
            &[("VDST", 0), ("SRC0", 193), ("SRC1", 128)],
        ));
        code.extend(one(
            "V_MBCNT_HI_U32_B32",
            "ENC_VOP3",
            &[("VDST", 0), ("SRC0", 193), ("SRC1", 256)],
        ));
        code.extend(one(
            "S_AND_SAVEEXEC_B64",
            "ENC_SOP1",
            &[("SDST", 8), ("SSRC0", 106)],
        ));
        code.extend(one(
            "S_ANDN2_WREXEC_B64",
            "ENC_SOP1",
            &[("SDST", 8), ("SSRC0", 8)],
        ));
        code.extend(one("S_CBRANCH_EXECZ", "ENC_SOPP", &[("SIMM16", 0)]));
        code.extend(one("S_CBRANCH_VCCNZ", "ENC_SOPP", &[("SIMM16", 0)]));
        code.extend(one("S_SENDMSG", "ENC_SOPP", &[("SIMM16", 9)]));
        code.extend(one("S_BARRIER", "ENC_SOPP", &[("SIMM16", 0)]));
        code.extend(one(
            "EXP",
            "ENC_EXP",
            &[("TGT", 12), ("EN", 1), ("VSRC0", 0)],
        ));
        let report = analyze(&code);
        assert_eq!((report.mbcnt_lo.len(), report.mbcnt_hi.len()), (1, 1));
        assert_eq!((report.saveexec.len(), report.wrexec.len()), (1, 1));
        assert_eq!(
            (
                report.whole_mask_branches.len(),
                report.allocation_messages.len()
            ),
            (2, 1)
        );
        assert_eq!((report.barriers.len(), report.exports.len()), (1, 1));
        assert_eq!(report.lane_reads.len(), 2);
        assert!(report
            .lane_reads
            .iter()
            .all(|read| read.provenance == LaneValueProvenance::Move));
    }

    #[test]
    fn reports_incomplete_trailing_instruction() {
        let mut code = one("S_NOP", "ENC_SOPP", &[("SIMM16", 0)]);
        code.extend([0x01, 0x02]);
        let report = analyze(&code);
        assert_eq!(report.instructions, 1);
        assert_eq!(report.decoded_bytes, 4);
        assert_eq!(report.undecoded_at, Some(4));
    }

    #[test]
    fn reports_unknown_encoding_without_panicking() {
        let report = analyze(&0x0000_003fu32.to_le_bytes());
        assert_eq!(report.instructions, 0);
        assert_eq!(report.decoded_bytes, 0);
        assert_eq!(report.undecoded_at, Some(0));
        assert!(
            crate::gpu::recomp::geometry_wave32_hazards(&0x0000_003fu32.to_le_bytes())
                .contains("incomplete geometry program decode")
        );
    }
}
