//! Whether a wave64 VS / PS gives the console's result when a GPU runs each 32-lane half of its
//! console wave as a wave of its own. Classification only: nothing translates by it yet.
//!
//! Ported from xcl's crates/xcl/src/gpu/recomp/partition.rs, commits dfc5578 / 5796cc4 (the
//! classifier) and 43a5550 (a vote's side may narrow EXEC through `s_wqm` then `s_and exec`), by
//! emirduran707 (GPL-2.0, copied with credit). The analysis below is xcl's, rule for rule; what
//! differs is the register-use model (`Uses`): xcl reads operand tables (`x64_masks::uses`,
//! `rdna2::sgpr_uses` / `vgpr_uses`, `upper_lanes::vgpr_writes`), which our decoder
//! (`crate::rdna2::Insn`) does not carry, so the operands, their widths and the implicit
//! EXEC / VCC / M0 / SCC uses are derived here from the mnemonic and the raw words.
//!
//! The program's result must not depend on how its wave is cut: a value that depends on the
//! cut must not reach what the program leaves behind (an export, a store, a branch, EXEC).
//! - A wave-wide vote (`s_cbranch_vcc*`, `exec*`, `scc*` on a mask) is taken per half: the half
//!   with no lane for a side takes the other one, which the console wave did not run for those
//!   lanes. That is exact when the vote only skips lane-masked work: the side taken with the
//!   mask empty is the join (the branch's post-dominator), and for a VCC vote the other side
//!   turns EXEC off in that half before any lane work – or the compare is of values every lane
//!   has (each half then votes as the wave does).
//! - Lane counts, ranks and first-lane indices of a mask (`s_bcnt1`, `v_mbcnt`, `s_ff1`), a mask
//!   used as a number, and another lane's varying value (`v_readfirstlane`, `v_readlane`) are
//!   different in a half than in the wave; so is everything they reach.
//!
//! A forward data-flow pass over the program's blocks (`partition_cfg`) carries those as taint
//! bits per register; values every lane has are followed too, since reading one from another
//! lane is exact.

use std::collections::BTreeSet;

use crate::rdna2::{Enc, Insn, Opnd};

#[path = "partition_cfg.rs"]
mod cfg;

use cfg::{Cfg, PostDom};

const LANE_COUNT: u8 = 1;
const RANK: u8 = 2;
const FIRST_LANE: u8 = 4;
const LANE_ID: u8 = 8;
const MASK_DATA: u8 = 16;
const CROSS_LANE: u8 = 32;
const TAINTED_EXEC: u8 = 64;
const STALE_SCALAR: u8 = 128;

const VCC: usize = 106;
const M0: usize = 124;
const EXEC: usize = 126;

/// Classify a wave64 VS / PS (instructions in address order). Ok: what it leaves behind does
/// not depend on how the wave is cut (Exact). Err: it may (the reasons; the analysis is
/// conservative, unknown counts as this).
pub(super) fn classify(insns: &[Insn]) -> Result<(), Vec<String>> {
    let insts: Vec<D> = insns.iter().map(D::new).collect();
    let reasons = classify_decoded(&insts);
    if reasons.is_empty() {
        Ok(())
    } else {
        Err(reasons.into_iter().map(str::to_owned).collect())
    }
}

// ---------------------------------------------------------------------------------------------
// The instruction view: what xcl's `Decoded` gives the analysis
// ---------------------------------------------------------------------------------------------

/// SDWA / DPP form of a VOP1 / VOP2 / VOPC instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Variant {
    Plain,
    Sdwa,
    Dpp16,
    Dpp8,
}

/// One instruction with its upper-case name (xcl's spelling) and register uses.
struct D<'a> {
    insn: &'a Insn,
    name: String,
    enc: Enc,
    variant: Variant,
    uses: Uses,
}

impl<'a> D<'a> {
    fn new(insn: &'a Insn) -> D<'a> {
        let name = insn.mnemonic.to_ascii_uppercase();
        let w0 = insn.raw.first().copied().unwrap_or(0);
        let variant = match insn.enc {
            Enc::Vop1 | Enc::Vop2 | Enc::Vopc => match w0 & 0x1ff {
                249 => Variant::Sdwa,
                250 => Variant::Dpp16,
                233 | 234 => Variant::Dpp8,
                _ => Variant::Plain,
            },
            _ => Variant::Plain,
        };
        let uses = build_uses(insn, &name, variant);
        D { insn, name, enc: insn.enc, variant, uses }
    }

    fn w(&self, k: usize) -> u64 {
        u64::from(self.insn.raw.get(k).copied().unwrap_or(0))
    }

    /// A raw field, by xcl's field name; None where this encoding has no such field.
    fn f(&self, field: &str) -> Option<u64> {
        let w0 = self.w(0);
        Some(match (self.enc, field) {
            (Enc::Sop1 | Enc::Sop2 | Enc::Sopc, "SSRC0") => w0 & 0xff,
            (Enc::Sop2 | Enc::Sopc, "SSRC1") => w0 >> 8 & 0xff,
            (Enc::Sop1 | Enc::Sop2 | Enc::Sopk, "SDST") => w0 >> 16 & 0x7f,
            (Enc::Vop1 | Enc::Vop2 | Enc::Vopc, "SRC0") => w0 & 0x1ff,
            (Enc::Vop3, "SRC0") => self.w(1) & 0x1ff,
            (Enc::Vop3, "SRC1") => self.w(1) >> 9 & 0x1ff,
            (Enc::Vop3, "SRC2") => self.w(1) >> 18 & 0x1ff,
            // the DPP16 control dword
            (Enc::Vop1 | Enc::Vop2 | Enc::Vopc, "DPP_CTRL") if self.variant == Variant::Dpp16 => self.w(1) >> 8 & 0x1ff,
            (Enc::Vop1 | Enc::Vop2 | Enc::Vopc, "BANK_MASK") if self.variant == Variant::Dpp16 => self.w(1) >> 24 & 0xf,
            (Enc::Vop1 | Enc::Vop2 | Enc::Vopc, "ROW_MASK") if self.variant == Variant::Dpp16 => self.w(1) >> 28 & 0xf,
            (Enc::Exp, "TGT") => self.insn.aux.imm as u64,
            _ => return None,
        })
    }

    fn is_sop(&self) -> bool {
        matches!(self.enc, Enc::Sop1 | Enc::Sop2 | Enc::Sopc | Enc::Sopk | Enc::Sopp)
    }

    /// xcl's `enc.name.starts_with("ENC_S")`: scalar ALU, program control and scalar memory.
    fn is_scalar(&self) -> bool {
        self.is_sop() || self.enc == Enc::Smem
    }
}

// ---------------------------------------------------------------------------------------------
// Register uses (replaces xcl's x64_masks::uses, rdna2::sgpr_uses / vgpr_uses and
// upper_lanes::vgpr_writes, which read operand tables our decoder does not have)
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Reg {
    S(usize),
    V(usize),
    Scc,
}

/// What an instruction reads and writes.
#[derive(Default)]
struct Uses {
    /// registers whose whole value is read (destinations a write keeps part of, SDWA scalar
    /// sources, SMEM offsets, `src_vccz` / `src_execz`; SCC, also as `src_scc`; the VGPRs, NSA
    /// addresses included)
    reads: Vec<Reg>,
    /// the SGPRs read as an explicit operand (not an implicit EXEC / VCC / M0), a per-lane
    /// selector operand included
    explicit: Vec<usize>,
    /// a mask read lane by lane as a selector or carry (`v_cndmask`, `v_add_co_ci`, …)
    lane_mask: Vec<usize>,
    /// registers written (a write that keeps part of the old value reads it too); image loads
    /// by their DMASK
    writes: Vec<Reg>,
}

impl Uses {
    fn push_read(&mut self, r: Reg) {
        if !self.reads.contains(&r) {
            self.reads.push(r);
        }
    }

    fn push_write(&mut self, r: Reg) {
        if !self.writes.contains(&r) {
            self.writes.push(r);
        }
    }

    /// The SGPRs a scalar operand code names (`n` dwords from it): s0–s105, VCC, TTMPs, M0,
    /// EXEC; `src_vccz` / `src_execz` read the whole VCC / EXEC; NULL, constants and literals
    /// name none. None for `src_scc`.
    fn sgprs(code: u64, n: usize) -> Option<Vec<usize>> {
        Some(match code {
            251 => vec![VCC, VCC + 1],
            252 => vec![EXEC, EXEC + 1],
            253 => return None,
            125 => Vec::new(),
            c if c <= 127 => (c as usize..(c as usize + n.max(1)).min(128)).filter(|&s| s != 125).collect(),
            _ => Vec::new(),
        })
    }

    /// A scalar operand read in whole.
    fn read_code(&mut self, code: u64, n: usize, explicit: bool) {
        match Self::sgprs(code, n) {
            None => self.push_read(Reg::Scc),
            Some(regs) => {
                for s in regs {
                    self.push_read(Reg::S(s));
                    if explicit && !self.explicit.contains(&s) {
                        self.explicit.push(s);
                    }
                }
            }
        }
    }

    fn write_code(&mut self, code: u64, n: usize) {
        if let Some(regs) = Self::sgprs(code, n) {
            if code <= 127 {
                for s in regs {
                    self.push_write(Reg::S(s));
                }
            }
        }
    }

    fn read_v(&mut self, v: u8, n: usize) {
        for k in 0..n.max(1) {
            if let Some(x) = (v as usize).checked_add(k).filter(|&x| x < 256) {
                self.push_read(Reg::V(x));
            }
        }
    }

    fn write_v(&mut self, v: u8, n: usize) {
        for k in 0..n {
            if let Some(x) = (v as usize).checked_add(k).filter(|&x| x < 256) {
                self.push_write(Reg::V(x));
            }
        }
    }

    /// A decoded source operand, read in whole.
    fn read_opnd(&mut self, o: Opnd, n: usize) {
        match o {
            Opnd::Vgpr(v) => self.read_v(v, n),
            o => {
                if let Some(code) = opnd_code(o) {
                    self.read_code(code, n, true);
                }
            }
        }
    }
}

/// The scalar operand code of a decoded non-VGPR operand.
fn opnd_code(o: Opnd) -> Option<u64> {
    Some(match o {
        Opnd::Sgpr(s) => u64::from(s),
        Opnd::VccLo => 106,
        Opnd::VccHi => 107,
        Opnd::M0 => 124,
        Opnd::ExecLo => 126,
        Opnd::ExecHi => 127,
        Opnd::Scc => 253,
        Opnd::Raw(r @ (108..=123 | 251 | 252)) => u64::from(r),
        _ => return None,
    })
}

/// Instructions whose mask operand is read lane by lane.
fn reads_mask_per_lane(n: &str) -> bool {
    matches!(n, "V_CNDMASK_B32" | "V_ADD_CO_CI_U32" | "V_SUB_CO_CI_U32" | "V_SUBREV_CO_CI_U32")
}

/// Scalar instructions that keep (part of) their destination's old value.
fn keeps_destination(n: &str) -> bool {
    n.starts_with("S_CMOV") || matches!(n, "S_ADDK_I32" | "S_MULK_I32" | "S_BITSET0_B32" | "S_BITSET1_B32" | "S_BITSET0_B64" | "S_BITSET1_B64")
}

/// Scalar instructions that read SCC.
fn reads_scc(n: &str) -> bool {
    n.starts_with("S_CSELECT_") || n.starts_with("S_CMOV") || matches!(n, "S_ADDC_U32" | "S_SUBB_U32") || n.starts_with("S_CBRANCH_SCC")
}

/// Scalar instructions that write SCC.
fn writes_scc(enc: Enc, n: &str) -> bool {
    match enc {
        Enc::Sopc => true,
        Enc::Sop2 => !(n.starts_with("S_CSELECT_") || n.starts_with("S_BFM_") || n.starts_with("S_MUL_") || n.starts_with("S_PACK_")),
        Enc::Sop1 => {
            n.starts_with("S_NOT_")
                || n.starts_with("S_WQM_")
                || n.starts_with("S_BCNT0_")
                || n.starts_with("S_BCNT1_")
                || n.starts_with("S_QUADMASK_")
                || n == "S_ABS_I32"
                || n.contains("SAVEEXEC")
                || n.contains("WREXEC")
        }
        Enc::Sopk => n.starts_with("S_CMPK_") || n == "S_ADDK_I32",
        _ => false,
    }
}

fn wide_token(n: &str) -> bool {
    n.split('_').any(|t| matches!(t, "B64" | "I64" | "U64" | "F64"))
}

/// Dwords of a scalar ALU instruction's (destination, first source, second source).
fn salu_widths(n: &str) -> (usize, usize, usize) {
    if !wide_token(n) {
        return (1, 1, 1);
    }
    match n {
        // a count or index of a 64-bit value
        "S_BCNT0_I32_B64" | "S_BCNT1_I32_B64" | "S_FF0_I32_B64" | "S_FF1_I32_B64" | "S_FLBIT_I32_B64" | "S_FLBIT_I32_I64" => (1, 2, 1),
        "S_LSHL_B64" | "S_LSHR_B64" | "S_ASHR_I64" | "S_BFE_U64" | "S_BFE_I64" => (2, 2, 1),
        "S_BFM_B64" => (2, 1, 1),
        "S_BITSET0_B64" | "S_BITSET1_B64" => (2, 1, 1),
        "S_BITCMP0_B64" | "S_BITCMP1_B64" => (0, 2, 1),
        "S_BITREPLICATE_B64_B32" => (2, 1, 1),
        _ => (2, 2, 2),
    }
}

/// Dwords of a VALU source `k` (0, 1, 2).
fn valu_src_width(n: &str, k: usize) -> usize {
    if !wide_token(n) {
        return 1;
    }
    if let Some(rest) = n.strip_prefix("V_CVT_") {
        // v_cvt_<dst>_<src>
        return if rest.split('_').last().is_some_and(|t| t.ends_with("64")) { 2 } else { 1 };
    }
    match (n, k) {
        ("V_LSHLREV_B64" | "V_LSHRREV_B64" | "V_ASHRREV_I64", 0) => 1,
        ("V_MAD_U64_U32" | "V_MAD_I64_I32", 0 | 1) => 1,
        ("V_LDEXP_F64" | "V_TRIG_PREOP_F64", 1) => 1,
        (_, 1) if n.contains("CLASS_F64") => 1,
        ("V_QSAD_PK_U16_U8" | "V_MQSAD_PK_U16_U8", 1) => 1,
        ("V_MQSAD_U32_U8", 1) => 1,
        ("V_MQSAD_U32_U8", 2) => 4,
        _ => 2,
    }
}

/// Dwords of a VALU VGPR destination.
fn valu_dst_width(n: &str) -> usize {
    if !wide_token(n) {
        return 1;
    }
    if let Some(rest) = n.strip_prefix("V_CVT_") {
        return if rest.split('_').next().is_some_and(|t| t.ends_with("64")) { 2 } else { 1 };
    }
    match n {
        "V_FREXP_EXP_I32_F64" => 1,
        "V_MQSAD_U32_U8" => 4,
        _ => 2,
    }
}

/// Sources a VOP3 / VOP3P instruction has (the decoder gives three for every one).
fn vop3_src_count(op: u32, n: &str) -> usize {
    if op >= 0x400 {
        return if ["FMA", "MAD", "DOT", "MIX"].iter().any(|k| n.contains(k)) { 3 } else { 2 };
    }
    match op {
        0..=255 => 2,
        256..=383 => {
            if reads_mask_per_lane(n) {
                3
            } else {
                2
            }
        }
        384..=511 => usize::from(!matches!(n, "V_NOP" | "V_PIPEFLUSH" | "V_CLREXCP")),
        _ => {
            const TWO: &[&str] = &[
                "V_ADD_F64", "V_MUL_F64", "V_MIN_F64", "V_MAX_F64", "V_LDEXP_F64", "V_MUL_LO_U32", "V_MUL_HI_U32", "V_MUL_HI_I32",
                "V_TRIG_PREOP_F64", "V_LSHLREV_B64", "V_LSHRREV_B64", "V_ASHRREV_I64", "V_BCNT_U32_B32", "V_MBCNT_LO_U32_B32",
                "V_MBCNT_HI_U32_B32", "V_READLANE_B32", "V_WRITELANE_B32", "V_BFM_B32", "V_CVT_PKNORM_I16_F32",
                "V_CVT_PKNORM_U16_F32", "V_CVT_PKRTZ_F16_F32", "V_CVT_PK_U16_U32", "V_CVT_PK_I16_I32", "V_LDEXP_F32",
                "V_ADD_NC_U16", "V_SUB_NC_U16", "V_MUL_LO_U16", "V_LSHLREV_B16", "V_LSHRREV_B16", "V_ASHRREV_I16", "V_MAX_U16",
                "V_MAX_I16", "V_MIN_U16", "V_MIN_I16", "V_ADD_NC_I16", "V_SUB_NC_I16", "V_ADD_NC_I32", "V_SUB_NC_I32",
                "V_PACK_B32_F16", "V_CVT_PKNORM_I16_F16", "V_CVT_PKNORM_U16_F16", "V_ADD_CO_U32", "V_SUB_CO_U32",
                "V_SUBREV_CO_U32", "V_INTERP_P1LL_F16", "V_INTERP_P1_F32", "V_INTERP_MOV_F32",
            ];
            if TWO.contains(&n) {
                2
            } else {
                3
            }
        }
    }
}

/// Dwords of a memory instruction's data, from its name (format / width suffix).
fn mem_dwords(n: &str) -> usize {
    let d16 = n.contains("D16");
    let fmt = if n.contains("XYZW") {
        4
    } else if n.contains("XYZ") {
        3
    } else if n.contains("_XY") {
        2
    } else if n.contains("FORMAT_X") {
        1
    } else {
        0
    };
    if fmt > 0 {
        return if d16 { (fmt + 1) / 2 } else { fmt };
    }
    if n.contains("DWORDX4") || n.contains("B128") {
        4
    } else if n.contains("DWORDX3") || n.contains("B96") {
        3
    } else if n.contains("DWORDX2") || n.contains("_X2") || wide_token(n) {
        2
    } else {
        1
    }
}

/// VGPR addresses of a non-NSA image instruction (A16 not halved: an over-count only adds reads).
fn mimg_address_count(n: &str, dim: u64) -> usize {
    let coords = [1, 2, 3, 3, 2, 3, 3, 4][(dim & 7) as usize];
    let grads = [2, 4, 6, 4, 2, 4, 4, 4][(dim & 7) as usize];
    if n == "IMAGE_GET_RESINFO" {
        return 1;
    }
    let mut count = coords;
    let ops = n.strip_prefix("IMAGE_SAMPLE").or_else(|| n.strip_prefix("IMAGE_GATHER4")).or_else(|| n.strip_prefix("IMAGE_GET_LOD"));
    if let Some(ops) = ops {
        for t in ops.split('_') {
            count += match t {
                "O" | "B" | "C" | "L" | "CL" => 1,
                "D" | "CD" => grads,
                _ => 0,
            };
        }
    } else if n.contains("_MIP") {
        count += 1;
    }
    count
}

fn build_uses(i: &Insn, n: &str, variant: Variant) -> Uses {
    let mut u = Uses::default();
    let w0 = u64::from(i.raw.first().copied().unwrap_or(0));
    let w1 = u64::from(i.raw.get(1).copied().unwrap_or(0));
    match i.enc {
        Enc::Sop1 | Enc::Sop2 | Enc::Sopc => {
            let (dn, s0n, s1n) = salu_widths(n);
            let ssrc0 = w0 & 0xff;
            let ssrc1 = w0 >> 8 & 0xff;
            let sdst = w0 >> 16 & 0x7f;
            let movrels = n.starts_with("S_MOVRELS") || n == "S_MOVRELSD_2_B32";
            if movrels {
                // s[src + m0]: any register from the encoded one up
                let s = ssrc0.min(124);
                u.read_code(s, 124 - s as usize, true);
            } else if !matches!(n, "S_GETPC_B64") {
                u.read_code(ssrc0, s0n, true);
            }
            if i.enc != Enc::Sop1 {
                u.read_code(ssrc1, s1n, true);
            }
            if n.starts_with("S_MOVREL") {
                u.read_code(M0 as u64, 1, false);
            }
            if keeps_destination(n) {
                u.read_code(sdst, dn, true);
            }
            let exec_op = n.contains("SAVEEXEC") || n.contains("WREXEC");
            if exec_op {
                u.read_code(EXEC as u64, 2, false);
            }
            if reads_scc(n) {
                u.push_read(Reg::Scc);
            }
            if i.enc != Enc::Sopc && dn > 0 && !matches!(n, "S_SETPC_B64" | "S_RFE_B64") {
                u.write_code(sdst, dn);
            }
            if exec_op {
                u.write_code(EXEC as u64, 2);
            }
            if writes_scc(i.enc, n) {
                u.push_write(Reg::Scc);
            }
        }
        Enc::Sopk => {
            let sdst = w0 >> 16 & 0x7f;
            if n.starts_with("S_CMPK_") || matches!(n, "S_CMOVK_I32" | "S_ADDK_I32" | "S_MULK_I32" | "S_SETREG_B32") {
                u.read_code(sdst, 1, true);
            }
            if reads_scc(n) {
                u.push_read(Reg::Scc);
            }
            match n {
                "S_MOVK_I32" | "S_CMOVK_I32" | "S_ADDK_I32" | "S_MULK_I32" | "S_GETREG_B32" => u.write_code(sdst, 1),
                "S_CALL_B64" => u.write_code(sdst, 2),
                _ => {}
            }
            if writes_scc(i.enc, n) {
                u.push_write(Reg::Scc);
            }
        }
        Enc::Sopp => {
            if n.starts_with("S_CBRANCH_SCC") {
                u.push_read(Reg::Scc);
            } else if n.starts_with("S_CBRANCH_VCC") {
                u.read_code(VCC as u64, 2, false);
            } else if n.starts_with("S_CBRANCH_EXEC") {
                u.read_code(EXEC as u64, 2, false);
            } else if n.starts_with("S_SENDMSG") {
                u.read_code(M0 as u64, 1, false);
            }
        }
        Enc::Smem => {
            let sdata = w0 >> 6 & 0x7f;
            let sbase = (w0 & 63) * 2;
            let soffset = w1 >> 25;
            let width = if n.contains("DWORDX16") {
                16
            } else if n.contains("DWORDX8") {
                8
            } else {
                mem_dwords(n)
            };
            if matches!(n, "S_MEMTIME" | "S_MEMREALTIME") {
                u.write_code(sdata, 2);
            } else if n.contains("LOAD") || n.contains("STORE") || n.contains("ATOMIC") {
                u.read_code(sbase, if n.starts_with("S_BUFFER") { 4 } else { 2 }, true);
                u.read_code(soffset, 1, true);
                if n.contains("LOAD") {
                    u.write_code(sdata, width);
                } else {
                    u.read_code(sdata, if n.contains("CMPSWAP") { 2 * width } else { width }, true);
                    if n.contains("ATOMIC") && w0 >> 16 & 1 == 1 {
                        u.write_code(sdata, width);
                    }
                }
            }
        }
        Enc::Vop1 | Enc::Vop2 | Enc::Vopc | Enc::Vop3 => valu_uses(&mut u, i, n, variant, w0, w1),
        Enc::Vinterp => {
            u.read_opnd(i.src.first().copied().unwrap_or(Opnd::Null), 1);
            u.read_code(M0 as u64, 1, false);
            u.write_v((w0 >> 18 & 0xff) as u8, 1);
        }
        Enc::Ds => {
            let addr = (w1 & 0xff) as u8;
            let data0 = (w1 >> 8 & 0xff) as u8;
            let data1 = (w1 >> 16 & 0xff) as u8;
            let vdst = (w1 >> 24 & 0xff) as u8;
            let read = n.starts_with("DS_READ");
            let gws = n.starts_with("DS_GWS");
            let no_addr = gws;
            let no_data0 = read || matches!(n, "DS_NOP" | "DS_APPEND" | "DS_CONSUME" | "DS_SWIZZLE_B32" | "DS_GWS_SEMA_V" | "DS_GWS_SEMA_P" | "DS_GWS_SEMA_RELEASE_ALL");
            let has_data1 = n.contains("WRITE2") || n.contains("CMPST") || n.contains("MSKOR") || n.contains("WRXCHG2") || n.contains("CONDXCHG");
            let width = if n.contains("B128") {
                4
            } else if n.contains("B96") {
                3
            } else if wide_token(n) {
                2
            } else {
                1
            };
            if !no_addr && n != "DS_NOP" {
                u.read_v(addr, 1);
            }
            if !no_data0 {
                u.read_v(data0, width);
            }
            if has_data1 {
                u.read_v(data1, width);
            }
            if gws || matches!(n, "DS_ORDERED_COUNT" | "DS_APPEND" | "DS_CONSUME") {
                u.read_code(M0 as u64, 1, false);
            }
            let returns = read
                || n.contains("_RTN_")
                || matches!(n, "DS_SWIZZLE_B32" | "DS_PERMUTE_B32" | "DS_BPERMUTE_B32" | "DS_APPEND" | "DS_CONSUME" | "DS_ORDERED_COUNT");
            if returns {
                let rw = if n.contains("READ2") { 2 * width } else if n.contains("WRXCHG2") { 2 * width } else { width };
                u.write_v(vdst, rw);
            }
        }
        Enc::Mubuf | Enc::Mtbuf => {
            let vaddr = (w1 & 0xff) as u8;
            let vdata = (w1 >> 8 & 0xff) as u8;
            let srsrc = (w1 >> 16 & 31) * 4;
            let soffset = w1 >> 24;
            let (offen, idxen) = (w0 >> 12 & 1, w0 >> 13 & 1);
            let glc = i.enc == Enc::Mubuf && w0 >> 14 & 1 == 1;
            let tfe = w1 >> 23 & 1 == 1;
            if offen + idxen > 0 {
                u.read_v(vaddr, (offen + idxen) as usize);
            }
            u.read_code(srsrc, 4, true);
            u.read_code(soffset, 1, true);
            let width = mem_dwords(n);
            if n.contains("LOAD") {
                if w0 >> 16 & 1 == 0 || i.enc == Enc::Mtbuf {
                    u.write_v(vdata, width + usize::from(tfe));
                }
            } else if n.contains("STORE") {
                u.read_v(vdata, width);
            } else if n.contains("ATOMIC") {
                u.read_v(vdata, if n.contains("CMPSWAP") { 2 * width } else { width });
                if glc {
                    u.write_v(vdata, width);
                }
            }
        }
        Enc::Mimg => {
            let vaddr = (w1 & 0xff) as u8;
            let vdata = (w1 >> 8 & 0xff) as u8;
            let nsa = (w0 >> 1 & 3) as usize;
            let bvh = n.starts_with("IMAGE_BVH");
            // addresses (and the descriptor SGPRs the decoder lists)
            if nsa > 0 || bvh {
                for &o in &i.src {
                    match o {
                        Opnd::Vgpr(v) => u.read_v(v, 1),
                        Opnd::Sgpr(s) => u.read_code(u64::from(s), if bvh { 4 } else { 8 }, true),
                        _ => {}
                    }
                }
            } else {
                u.read_v(vaddr, mimg_address_count(n, w0 >> 3 & 7));
                u.read_code((w1 >> 16 & 31) * 4, 8, true);
            }
            if !bvh && (n.starts_with("IMAGE_SAMPLE") || n.starts_with("IMAGE_GATHER4") || n == "IMAGE_GET_LOD") {
                u.read_code((w1 >> 21 & 31) * 4, 4, true);
            }
            let dmask = w0 >> 8 & 15;
            let d16 = w1 >> 31 & 1 == 1;
            let tfe_lwe = (w0 >> 16 & 1) | (w0 >> 17 & 1);
            let mut data = if n.starts_with("IMAGE_GATHER4") { 4 } else { (dmask.count_ones().max(1)) as usize };
            if d16 {
                data = data.div_ceil(2);
            }
            if bvh {
                u.write_v(vdata, 4);
            } else if n.contains("STORE") {
                u.read_v(vdata, data);
            } else if n.contains("ATOMIC") {
                u.read_v(vdata, data);
                if w0 >> 13 & 1 == 1 {
                    u.write_v(vdata, if n.contains("CMPSWAP") { data / 2 } else { data }.max(1));
                }
            } else {
                u.write_v(vdata, data + tfe_lwe as usize);
            }
        }
        Enc::Flat => {
            let addr = (w1 & 0xff) as u8;
            let data = (w1 >> 8 & 0xff) as u8;
            let saddr = w1 >> 16 & 0x7f;
            let vdst = (w1 >> 24 & 0xff) as u8;
            let seg = w0 >> 14 & 3;
            let scalar_addr = seg != 0 && saddr != 125 && saddr != 127;
            // flat: a 64-bit VGPR address; global: that, or a 32-bit offset beside SADDR;
            // scratch: a VGPR offset, or none beside SADDR
            let addr_n = match seg {
                1 => usize::from(!scalar_addr),
                _ if scalar_addr => 1,
                _ => 2,
            };
            if addr_n > 0 {
                u.read_v(addr, addr_n);
            }
            if scalar_addr {
                u.read_code(saddr, if seg == 1 { 1 } else { 2 }, true);
            }
            let width = mem_dwords(n);
            if n.contains("LOAD") {
                u.write_v(vdst, width);
            } else if n.contains("STORE") {
                u.read_v(data, width);
            } else if n.contains("ATOMIC") {
                u.read_v(data, if n.contains("CMPSWAP") { 2 * width } else { width });
                if w0 >> 16 & 1 == 1 {
                    u.write_v(vdst, width);
                }
            }
        }
        Enc::Exp => {
            let en = w0 & 15;
            let compr = w0 >> 10 & 1 == 1;
            for (k, &o) in i.src.iter().enumerate().take(4) {
                let on = if compr { k < 2 && en & (3 << (2 * k)) != 0 } else { en >> k & 1 == 1 };
                if on {
                    u.read_opnd(o, 1);
                }
            }
        }
        Enc::Unknown => {}
    }
    u
}

fn valu_uses(u: &mut Uses, i: &Insn, n: &str, variant: Variant, w0: u64, w1: u64) {
    let per_lane = reads_mask_per_lane(n);
    // sources (DPP: the first one is a VGPR in the control dword)
    let mut srcs: Vec<Opnd> = i.src.clone();
    if matches!(variant, Variant::Dpp16 | Variant::Dpp8) {
        if let Some(s) = srcs.first_mut() {
            *s = Opnd::Vgpr((w1 & 0xff) as u8);
        }
    }
    let count = if i.enc == Enc::Vop3 {
        vop3_src_count(i.op, n)
    } else if i.enc == Enc::Vop1 {
        usize::from(!matches!(n, "V_NOP" | "V_PIPEFLUSH" | "V_CLREXCP"))
    } else {
        2
    };
    for (k, &o) in srcs.iter().enumerate().take(count) {
        if per_lane && k == 2 {
            // the selector / carry-in mask: read lane by lane (an explicit operand though)
            let s = opnd_code(o).unwrap_or(VCC as u64);
            let pair = if s < 126 { [s as usize, s as usize + 1] } else { [VCC, VCC + 1] };
            u.lane_mask.extend(pair);
            for x in Uses::sgprs(s, 2).unwrap_or_default() {
                if !u.explicit.contains(&x) {
                    u.explicit.push(x);
                }
            }
            continue;
        }
        u.read_opnd(o, valu_src_width(n, k));
    }
    if per_lane && i.enc == Enc::Vop2 {
        u.lane_mask.extend([VCC, VCC + 1]);
    }
    if n.starts_with("V_DIV_FMAS") {
        u.read_code(VCC as u64, 2, false);
    }
    if n.starts_with("V_INTERP") || n.starts_with("V_MOVREL") {
        u.read_code(M0 as u64, 1, false);
    }
    // destinations
    let vdst = match i.enc {
        Enc::Vop3 => (w0 & 0xff) as u8,
        _ => (w0 >> 17 & 0xff) as u8,
    };
    let compare = n.starts_with("V_CMP");
    if compare {
        if n.starts_with("V_CMPX") {
            u.write_code(EXEC as u64, 2);
        } else if i.enc == Enc::Vop3 {
            u.write_code(w0 & 0xff, 2);
        } else if variant == Variant::Sdwa && w1 >> 15 & 1 == 1 {
            u.write_code(w1 >> 8 & 0x7f, 2);
        } else {
            u.write_code(VCC as u64, 2);
        }
        return;
    }
    if matches!(n, "V_READFIRSTLANE_B32" | "V_READLANE_B32") {
        u.write_code(u64::from(vdst), 1);
        return;
    }
    if matches!(n, "V_NOP" | "V_PIPEFLUSH" | "V_CLREXCP") {
        return;
    }
    // read-modify-write accumulators, and the other lanes of a lane write
    let accumulates = matches!(n, "V_FMAC_F32" | "V_MAC_F32" | "V_FMAC_LEGACY_F32" | "V_MAC_LEGACY_F32" | "V_FMAC_F16" | "V_DOT2C_F32_F16" | "V_DOT4C_I32_I8" | "V_PK_FMAC_F16" | "V_WRITELANE_B32");
    let width = valu_dst_width(n);
    if accumulates {
        u.read_v(vdst, width);
    }
    // VOP3B carry-out / scale destination
    if i.enc == Enc::Vop3 && (n.contains("_CO_") || matches!(n, "V_MAD_U64_U32" | "V_MAD_I64_I32" | "V_DIV_SCALE_F32" | "V_DIV_SCALE_F64")) {
        u.write_code(w0 >> 8 & 0x7f, 2);
    }
    if per_lane && n.contains("_CO_CI_") && i.enc == Enc::Vop2 {
        u.write_code(VCC as u64, 2);
    }
    u.write_v(vdst, width);
}

// ---------------------------------------------------------------------------------------------
// The analysis (xcl's partition.rs)
// ---------------------------------------------------------------------------------------------

/// What SCC holds, for a branch on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SccVote {
    /// not a vote: a scalar compare / result
    No,
    /// whether the EXEC just written is not empty (`s_and_saveexec_b64`, `s_and_b64 exec, …`)
    Exec,
    /// whether some other mask is not empty (the register it was written to, if any)
    Mask(Option<usize>),
}

#[derive(Clone, PartialEq, Eq)]
struct State {
    /// taint bits per SGPR (s0..s105, VCC, M0, EXEC: codes 0..127)
    s: Vec<u8>,
    scc: u8,
    scc_vote: SccVote,
    /// taint bits per VGPR
    v: Vec<u8>,
    /// the VGPR holds one value in every lane
    uniform: Vec<bool>,
    /// the SGPR holds a word of a lane mask
    mask: Vec<bool>,
    /// … from a compare of values every lane has, under the launch EXEC (each half votes as
    /// the wave does)
    uniform_mask: Vec<bool>,
    /// EXEC is what the program started with (or its whole-quad form)
    exec_launch: bool,
    /// the SGPR holds a word of that launch EXEC (saved by `s_mov_b64` / saveexec)
    launch: Vec<bool>,
    /// the SGPR holds a word of a set of lanes: a mask, or all / none (0 / -1, a scalar condition)
    lanes: Vec<bool>,
}

impl State {
    fn entry() -> State {
        let mut mask = vec![false; 128];
        mask[EXEC] = true;
        mask[EXEC + 1] = true;
        State {
            s: vec![0; 128],
            scc: 0,
            scc_vote: SccVote::No,
            v: vec![0; 256],
            uniform: vec![false; 256],
            uniform_mask: vec![false; 128],
            exec_launch: true,
            launch: vec![false; 128],
            lanes: mask.clone(),
            mask,
        }
    }

    fn join(&self, o: &State) -> State {
        let or = |a: &[u8], b: &[u8]| a.iter().zip(b).map(|(x, y)| x | y).collect();
        let and = |a: &[bool], b: &[bool]| a.iter().zip(b).map(|(x, y)| *x && *y).collect();
        let any = |a: &[bool], b: &[bool]| a.iter().zip(b).map(|(x, y)| *x || *y).collect();
        State {
            s: or(&self.s, &o.s),
            scc: self.scc | o.scc,
            scc_vote: if self.scc_vote == o.scc_vote { self.scc_vote } else { SccVote::Mask(None) },
            v: or(&self.v, &o.v),
            uniform: and(&self.uniform, &o.uniform),
            mask: any(&self.mask, &o.mask),
            uniform_mask: and(&self.uniform_mask, &o.uniform_mask),
            exec_launch: self.exec_launch && o.exec_launch,
            launch: and(&self.launch, &o.launch),
            lanes: and(&self.lanes, &o.lanes),
        }
    }

    fn exec_taint(&self) -> u8 {
        self.s[EXEC] | self.s[EXEC + 1]
    }
}

fn is_valu(d: &D) -> bool {
    matches!(d.enc, Enc::Vop1 | Enc::Vop2 | Enc::Vopc | Enc::Vop3)
}

fn is_vcmp(d: &D) -> bool {
    d.name.starts_with("V_CMP")
}

/// Reads a lane mask as a mask (bit by bit, or as a whole): not as a number.
fn mask_op(n: &str) -> bool {
    mask_result(n)
        || n.starts_with("S_CMP_") && n.ends_with("_U64")
        || n.starts_with("S_CBRANCH")
        || n.starts_with("V_CNDMASK")
        || n.contains("_CO_CI_")
        || matches!(n, "S_BCNT0_I32_B64" | "S_BCNT1_I32_B64" | "S_FF1_I32_B64" | "S_FF0_I32_B64" | "S_FLBIT_I32_B64")
}

/// Makes a lane mask (or a word of one) of lane masks (bitwise, moves, selects, saveexec): a
/// number of one is not.
fn mask_result(n: &str) -> bool {
    let bitwise = ["S_AND_", "S_OR_", "S_XOR_", "S_ANDN1_", "S_ANDN2_", "S_ORN1_", "S_ORN2_", "S_NAND_", "S_NOR_", "S_XNOR_"];
    let word = n.ends_with("_B64") || n.ends_with("_B32");
    (word && (bitwise.iter().any(|p| n.starts_with(p)) || n.starts_with("S_NOT_") || n.starts_with("S_MOV_B") || n.starts_with("S_CSELECT_B") || n.starts_with("S_CMOV_B") || n == "S_WQM_B64"))
        || n.contains("SAVEEXEC")
}

/// The partition-dependent value an instruction makes of its own (not from its sources).
fn source_taint(d: &D, st: &State, u: &Uses) -> u8 {
    let n = d.name.as_str();
    let reads = &u.reads;
    let mut t = 0;
    // explicit operands only: a vector or memory instruction's implicit EXEC is its lane mask
    let reads_mask = u.explicit.iter().any(|&s| st.mask[s]);
    // a vote's outcome used as a number (s_cselect, s_addc, … on SCC; not the branch on it)
    if reads.contains(&Reg::Scc) && st.scc_vote != SccVote::No && !n.starts_with("S_CBRANCH") {
        t |= MASK_DATA;
    }
    if matches!(n, "S_BCNT0_I32_B64" | "S_BCNT1_I32_B64") || (reads_mask && matches!(n, "S_BCNT0_I32_B32" | "S_BCNT1_I32_B32")) {
        t |= LANE_COUNT;
    }
    if matches!(n, "S_FF1_I32_B64" | "S_FF0_I32_B64" | "S_FLBIT_I32_B64" | "S_FLBIT_I32_I64") || (reads_mask && matches!(n, "S_FF1_I32_B32" | "S_FLBIT_I32_B32")) {
        t |= FIRST_LANE;
    }
    if n.starts_with("V_MBCNT") {
        // a constant SRC0 (-1): the lane's index in the wave; a mask: its rank in it
        let constant = matches!(d.f("SRC0"), Some(128..=208 | 240..=248 | 255));
        t |= if constant { LANE_ID } else { RANK };
    }
    // DPP16: a row or bank mask names rows of the wave (a 32-lane wave has rows 0 and 1 only);
    // row_bcast15 / row_bcast31 read across rows
    if d.variant == Variant::Dpp16 {
        if d.f("ROW_MASK").is_some_and(|m| m != 0xF) || d.f("BANK_MASK").is_some_and(|m| m != 0xF) {
            t |= LANE_ID;
        }
        if matches!(d.f("DPP_CTRL"), Some(0x142 | 0x143)) {
            t |= CROSS_LANE;
        }
    }
    // one lane written: lane k of each half is a different console lane
    if n == "V_WRITELANE_B32" {
        t |= LANE_ID;
    }
    // another lane's value: exact only for a value every lane has, read from the first active
    // lane (a given lane may be one the half did not launch)
    if (n == "V_READFIRSTLANE_B32" && reads.iter().any(|r| matches!(r, Reg::V(v) if !st.uniform[*v]))) || n == "V_READLANE_B32" {
        t |= CROSS_LANE;
    }
    if reads_mask && !mask_op(n) && !is_vcmp(d) {
        t |= MASK_DATA;
    }
    // a number combined with a mask (`s_and_b64 exec, exec, 1`): lane k of each half is a
    // different console lane. Masks, and 0 / -1 (a scalar condition as a mask), are not numbers
    let bitwise = ["S_AND_", "S_OR_", "S_XOR_", "S_ANDN1_", "S_ANDN2_", "S_ORN1_", "S_ORN2_", "S_NAND_", "S_NOR_", "S_XNOR_"].iter().any(|p| n.starts_with(p));
    if bitwise && (reads_mask || n.contains("SAVEEXEC")) {
        let wide = usize::from(n.ends_with("_B64"));
        let set = |a: Option<u64>| match a {
            None | Some(128 | 193) => true,
            Some(s @ 0..=127) => (s as usize..=s as usize + wide).all(|x| x < 128 && st.lanes[x]),
            _ => false,
        };
        if !set(d.f("SSRC0")) || !set(d.f("SSRC1")) {
            t |= MASK_DATA;
        }
    }
    t
}

fn reasons_of(t: u8, out: &mut BTreeSet<&'static str>) {
    for (bit, name) in [
        (LANE_COUNT, "lane-count"),
        (RANK, "rank"),
        (FIRST_LANE, "first-lane"),
        (LANE_ID, "lane-id"),
        (MASK_DATA, "mask-data"),
        (CROSS_LANE, "cross-lane"),
        (TAINTED_EXEC, "tainted-exec"),
        (STALE_SCALAR, "stale-scalar"),
    ] {
        if t & bit != 0 {
            out.insert(name);
        }
    }
}

/// What the program leaves behind: exports, memory writes (LDS / GDS ones included).
fn is_store(n: &str) -> bool {
    let ds_read = n.starts_with("DS_READ") || matches!(n, "DS_SWIZZLE_B32" | "DS_PERMUTE_B32" | "DS_BPERMUTE_B32" | "DS_NOP");
    n == "EXP" || n.contains("STORE") || n.contains("ATOMIC") || (n.starts_with("DS_") && !ds_read) || n.starts_with("S_SENDMSG")
}

/// One instruction: the state after it; what it leaves behind that depends on the cut goes to
/// `out`.
fn step(d: &D, st: &mut State, out: &mut BTreeSet<&'static str>) {
    let n = d.name.as_str();
    let u = &d.uses;
    let (reads, writes) = (&u.reads, &u.writes);
    let mut t = source_taint(d, st, u);
    for r in reads {
        t |= match *r {
            Reg::S(s) => st.s[s],
            Reg::V(v) => st.v[v],
            Reg::Scc => st.scc,
        };
    }
    let valu = is_valu(d) || !d.is_scalar();
    if valu && st.exec_taint() != 0 {
        t |= TAINTED_EXEC | st.exec_taint();
    }
    if is_store(n) && t != 0 {
        reasons_of(t, out);
    }
    if n.starts_with("S_CBRANCH") && t != 0 {
        reasons_of(t, out);
    }
    let writes_exec = writes.contains(&Reg::S(EXEC)) || writes.contains(&Reg::S(EXEC + 1));
    if writes_exec && t != 0 {
        reasons_of(t, out);
    }
    // one word of EXEC written on its own: a 32-lane half's lanes are all under its low word
    if writes.contains(&Reg::S(EXEC)) != writes.contains(&Reg::S(EXEC + 1)) {
        out.insert("mask-data");
    }
    // a number written to EXEC (not all lanes, not none): lane k of each half is a different
    // console lane
    if writes_exec && matches!(n, "S_MOV_B64" | "S_MOV_B32") && !matches!(d.f("SSRC0"), Some(128 | 193) | Some(0..=127)) {
        out.insert("mask-data");
    }
    let reads_mask_or_exec = reads.iter().any(|r| matches!(r, Reg::S(s) if st.mask[*s])) || u.lane_mask.iter().any(|&s| st.mask[s]);
    // a per-lane selector counts as a source: it is every lane's only if all or none of them
    let all_uniform = reads.iter().all(|r| !matches!(r, Reg::V(v) if !st.uniform[*v])) && u.lane_mask.iter().all(|&s| st.uniform_mask[s]);
    // a mask that is all or none of the active lanes (EXEC itself, a compare of values every lane
    // has, a scalar condition selected into a mask): a half with active lanes votes on it as the
    // wave does. A half with none may take the other side: it writes no lane there, and its
    // scalars are followed as stale where the sides join. Kept by these ops, not by complements.
    let vcmp_uniform = is_vcmp(d) && all_uniform;
    let keeps_uniform = matches!(n, "S_AND_B64" | "S_OR_B64" | "S_ANDN2_B64" | "S_MOV_B64" | "S_CSELECT_B64" | "S_WQM_B64") || n.contains("SAVEEXEC");
    let sgpr_reads: Vec<usize> = reads.iter().filter_map(|r| if let Reg::S(s) = r { Some(*s) } else { None }).collect();
    let is_launch = |s: usize| if s == EXEC || s == EXEC + 1 { st.exec_launch } else { st.launch[s] };
    let mask_uniform = keeps_uniform && sgpr_reads.iter().all(|&s| !st.mask[s] || st.uniform_mask[s] || s == EXEC || s == EXEC + 1 || is_launch(s));
    let from_launch = matches!(n, "S_MOV_B64" | "S_WQM_B64") && !sgpr_reads.is_empty() && sgpr_reads.iter().all(|&s| is_launch(s));
    let or_launch = n == "S_OR_B64" && sgpr_reads.iter().any(|&s| is_launch(s));
    let saved_exec = n.contains("SAVEEXEC") && st.exec_launch;
    // a set of lanes made of sets of lanes and 0 / -1
    let lane_src = |a: Option<u64>| match a {
        None | Some(128 | 193) => true,
        Some(s @ 0..=127) => st.lanes[s as usize] && (s == 127 || st.lanes[s as usize + 1] || !n.ends_with("_B64")),
        _ => false,
    };
    let lanes_made = mask_result(n) && lane_src(d.f("SSRC0")) && lane_src(d.f("SSRC1"));
    let mut new_scc = None;
    for w in writes {
        match *w {
            Reg::S(s) => {
                st.s[s] = t;
                // (a carry-out is a mask too)
                st.mask[s] = s == EXEC || s == EXEC + 1 || is_vcmp(d) || (is_valu(d) && n.contains("_CO_")) || (mask_result(n) && reads_mask_or_exec);
                st.uniform_mask[s] = vcmp_uniform || (st.mask[s] && mask_uniform);
                st.lanes[s] = st.mask[s] || lanes_made;
                st.launch[s] = s != EXEC && s != EXEC + 1 && (from_launch || (saved_exec && Some(&Reg::S(s)) != writes.iter().find(|w| **w == Reg::S(EXEC))));
            }
            Reg::V(v) => {
                st.v[v] = t;
                // a value every lane has: written under the launch EXEC (a narrowed one leaves the
                // other lanes their old value) by a plain VALU op of such values (not DPP / SDWA,
                // which read other lanes or parts), or a load from an address every lane has
                let load = matches!(d.enc, Enc::Mubuf | Enc::Mtbuf | Enc::Mimg) && !n.contains("ATOMIC");
                let plain = is_valu(d) && d.variant == Variant::Plain && !n.starts_with("V_PERMLANE");
                st.uniform[v] = st.exec_launch && all_uniform && t & (RANK | LANE_ID | CROSS_LANE) == 0 && n != "V_WRITELANE_B32" && (plain || load);
            }
            Reg::Scc => {
                new_scc = Some(t);
            }
        }
    }
    // SCC no longer says whether the mask it was about is empty once that mask is rewritten
    if new_scc.is_none() {
        let stale_vote = match st.scc_vote {
            SccVote::Exec => writes_exec,
            SccVote::Mask(Some(m)) => writes.contains(&Reg::S(m)) || writes.contains(&Reg::S(m + 1)),
            _ => false,
        };
        if stale_vote {
            st.scc_vote = SccVote::Mask(None);
        }
    }
    if let Some(sc) = new_scc {
        st.scc = sc;
        st.scc_vote = if writes_exec {
            SccVote::Exec
        } else if reads_mask_or_exec && (mask_result(n) || n.starts_with("S_CMP")) {
            let to = writes.iter().find_map(|w| if let Reg::S(s) = w { Some(*s) } else { None });
            SccVote::Mask(if mask_result(n) { to } else { None })
        } else {
            SccVote::No
        };
    }
    if writes_exec {
        // EXEC back at the launch mask: its whole-quad form, a copy of it, or OR-ed with one
        st.exec_launch = from_launch || or_launch;
        // whole-quad mode turns on helper lanes, which got none of the values written before it
        if n == "S_WQM_B64" {
            st.uniform.iter_mut().for_each(|u| *u = false);
            st.uniform_mask.iter_mut().for_each(|u| *u = false);
        }
    }
}

/// The scalar registers an instruction writes, as a mask (bit s: register s).
fn scalar_writes(d: &D) -> u128 {
    d.uses.writes.iter().fold(0, |m, w| match *w {
        Reg::S(s) if s < 128 => m | 1 << s,
        _ => m,
    })
}

/// Whether the 64-bit mask in registers `m`, `m + 1` is in `z`.
fn has_pair(z: u128, m: usize) -> bool {
    m < 127 && z & 3 << m == 3 << m
}

/// What a half whose lanes a vote's side does not run knows of its scalar registers (EXEC and
/// VCC included), as masks of registers: `z`, they hold zero there; `r`, they hold the EXEC it
/// had at the vote.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Half {
    z: u128,
    r: u128,
}

impl Half {
    fn meet(self, o: Half) -> Half {
        Half { z: self.z & o.z, r: self.r & o.r }
    }
}

/// Updates `h` for one instruction (`writes`: what it writes).
fn zero_step(d: &D, writes: u128, h: &mut Half) {
    if writes == 0 {
        return;
    }
    let n = d.name.as_str();
    // (the constant 0 is zero)
    let zero = |a: Option<u64>| a == Some(128) || a.is_some_and(|s| has_pair(h.z, s as usize));
    let vote = |a: Option<u64>| a.is_some_and(|s| has_pair(h.r, s as usize));
    let (ez, er) = (has_pair(h.z, EXEC), has_pair(h.r, EXEC));
    let (z0, z1) = (zero(d.f("SSRC0")), zero(d.f("SSRC1")));
    let (r0, r1) = (vote(d.f("SSRC0")), vote(d.f("SSRC1")));
    // x | y and x ^ y are the vote's EXEC when one is and the other is zero (x | x too)
    let or = |xr: bool, xz: bool, yr: bool, yz: bool| xr && (yr || yz) || yr && xz;
    let xor = |xr: bool, xz: bool, yr: bool, yz: bool| xr && yz || yr && xz;
    // (zero, the vote's EXEC) for the destination, and for a write of EXEC beside it, EXEC
    let (dst, exec) = match n {
        "S_MOV_B64" => ((z0, r0), None),
        "S_WQM_B64" => ((z0, false), None),
        "S_ANDN2_B64" => ((z0, r0 && z1), None),
        "S_AND_B64" => ((z0 || z1, r0 && r1), None),
        "S_OR_B64" => ((z0 && z1, or(r0, z0, r1, z1)), None),
        "S_XOR_B64" => ((z0 && z1, xor(r0, z0, r1, z1)), None),
        // saveexec: the destination is the old EXEC
        "S_AND_SAVEEXEC_B64" => ((ez, er), Some((z0 || ez, r0 && er))),
        "S_OR_SAVEEXEC_B64" => ((ez, er), Some((z0 && ez, or(r0, z0, er, ez)))),
        "S_XOR_SAVEEXEC_B64" => ((ez, er), Some((z0 && ez, xor(r0, z0, er, ez)))),
        // EXEC = s0 & ~EXEC (and the destination); EXEC = ~s0 & EXEC
        "S_ANDN2_SAVEEXEC_B64" => ((ez, er), Some((z0, r0 && ez))),
        "S_ANDN1_SAVEEXEC_B64" => ((ez, er), Some((ez, false))),
        "S_ANDN2_WREXEC_B64" => ((z0, r0 && ez), Some((z0, r0 && ez))),
        "S_ANDN1_WREXEC_B64" => ((ez, false), Some((ez, false))),
        // a compare writes 0 for the lanes not in EXEC
        _ if n.starts_with("V_CMPX") => ((false, false), Some((ez, false))),
        _ if n.starts_with("V_CMP") => ((ez, false), None),
        _ => ((false, false), None),
    };
    let exec_bits = 3u128 << EXEC;
    let (ex_z, ex_r) = exec.unwrap_or(dst);
    let set = |d: bool, e: bool| (if d { writes & !exec_bits } else { 0 }) | (if e { writes & exec_bits } else { 0 });
    h.z = h.z & !writes | set(dst.0, ex_z);
    h.r = h.r & !writes | set(dst.1, ex_r);
}

/// What a half that skips the blocks from `run` up to `stop` (exclusive; None: the program's end)
/// knows on entering each of them (None: not one of them), from `at_vote` on entering `run` from
/// the vote's block `from`, to a fixed point. (A wave that entered them stays in them until
/// `stop`: an edge from another block is not on its way.)
fn zero_entries(insts: &[D], sw: &[u128], g: &Cfg, from: usize, run: usize, stop: Option<usize>, at_vote: Half) -> Vec<Option<Half>> {
    let nb = g.start.len();
    let mut inside = vec![false; nb];
    let mut todo = vec![run];
    while let Some(x) = todo.pop() {
        if Some(x) != stop && !inside[x] {
            inside[x] = true;
            todo.extend(g.succ[x].iter().copied());
        }
    }
    let entry = |x: usize, out: &[Option<Half>]| {
        let ins = g.pred[x].iter().filter_map(|&p| if x == run && p == from { Some(at_vote) } else { out[p] });
        ins.reduce(Half::meet)
    };
    // (only what holds on every way in; the sets only shrink)
    let mut out: Vec<Option<Half>> = vec![None; nb];
    loop {
        let mut changed = false;
        for x in (0..nb).filter(|&x| inside[x]) {
            let Some(mut h) = entry(x, &out) else { continue };
            for i in g.range(x) {
                zero_step(&insts[i], sw[i], &mut h);
            }
            if out[x] != Some(h) {
                out[x] = Some(h);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    (0..nb).map(|x| if inside[x] { entry(x, &out) } else { None }).collect()
}

/// A vote's side, from `run` up to `stop`, differs for a half that skips it: it writes EXEC a value
/// not zero in that half (a write on the way into the join with no lane work after it is the
/// join's restore), or reaches the join with an EXEC other than the vote's (zero too, if
/// `exec_zero`: the half had no lane at the vote). `at_vote`: what the half knows there.
#[allow(clippy::too_many_arguments)]
fn widens_after(insts: &[D], sw: &[u128], g: &Cfg, from: usize, run: usize, stop: Option<usize>, at_vote: Half, exec_zero: bool) -> bool {
    for (x, h) in zero_entries(insts, sw, g, from, run, stop, at_vote).into_iter().enumerate() {
        let Some(mut h) = h else { continue };
        let range: Vec<usize> = g.range(x).collect();
        for (k, &i) in range.iter().enumerate() {
            zero_step(&insts[i], sw[i], &mut h);
            if sw[i] & 3 << EXEC == 0 || has_pair(h.z, EXEC) {
                continue;
            }
            let trailing = stop.is_some() && g.succ[x].iter().all(|&s| Some(s) == stop) && range[k + 1..].iter().all(|&j| insts[j].is_sop());
            if !trailing {
                return true;
            }
        }
        if stop.is_some_and(|s| g.succ[x].contains(&s)) && !(has_pair(h.r, EXEC) || exec_zero && has_pair(h.z, EXEC)) {
            return true;
        }
    }
    false
}

/// What a half whose mask `m` (a 64-bit register pair) is zero at the end of the vote's block `b`
/// knows there: `m` is zero, and what is written from it after its last write in the block; EXEC
/// is the vote's, and what is copied from it after its last write.
fn zero_at_vote(insts: &[D], sw: &[u128], g: &Cfg, b: usize, m: usize) -> Half {
    let range = g.range(b);
    let branch = range.end - 1;
    let after_last = |reg: usize| (range.start..branch).rev().find(|&i| sw[i] & 3 << reg != 0).map_or(range.start, |i| i + 1);
    let mut z = Half { z: 3 << m, r: 0 };
    for i in after_last(m)..branch {
        zero_step(&insts[i], sw[i], &mut z);
    }
    let mut r = Half { z: 0, r: 3 << EXEC };
    for i in after_last(EXEC)..branch {
        zero_step(&insts[i], sw[i], &mut r);
    }
    Half { z: z.z, r: r.r }
}

/// A conditional branch's sides: (the side taken with the mask empty, the other side, the mask's
/// register; None for EXEC). None if it is not a vote on a known mask.
fn sides(g: &Cfg, b: usize, d: &D, st: &State) -> Option<(usize, usize, Option<usize>)> {
    let n = d.name.as_str();
    // a conditional branch's successors: its target, then the next block (one if they are the same)
    let [t, next] = g.succ[b][..] else { return None };
    let mask = match n {
        "S_CBRANCH_EXECZ" | "S_CBRANCH_EXECNZ" => None,
        "S_CBRANCH_VCCZ" | "S_CBRANCH_VCCNZ" => Some(VCC),
        "S_CBRANCH_SCC0" | "S_CBRANCH_SCC1" => match st.scc_vote {
            SccVote::Exec => None,
            // SCC from some other mask: the register it was written to is the one to narrow EXEC by
            SccVote::Mask(Some(m)) => Some(m),
            _ => return None,
        },
        _ => return None,
    };
    let empty_is_target = matches!(n, "S_CBRANCH_EXECZ" | "S_CBRANCH_VCCZ" | "S_CBRANCH_SCC0");
    Some(if empty_is_target { (t, next, mask) } else { (next, t, mask) })
}

/// Block `b` ends the program and leaves nothing behind: at most a null export (a pixel shader's
/// way out when every pixel is discarded) – a half whose mask is empty that goes there writes
/// nothing, as its lanes write nothing on the console.
fn leaves_nothing(insts: &[D], g: &Cfg, b: usize) -> bool {
    g.succ[b].is_empty()
        && g.range(b).all(|i| {
            let d = &insts[i];
            let n = d.name.as_str();
            if n == "EXP" {
                return d.f("TGT") == Some(9);
            }
            !is_store(n) && (d.is_sop() || n == "S_ENDPGM")
        })
}

/// Block `b`, a vote's side, turns EXEC off in a half whose mask is empty before it does any lane
/// work: from what that half knows at the vote (`at_vote`), EXEC is zero in it once the block's
/// leading scalar instructions are done (`s_and_saveexec_b64 s, m`, `s_wqm_b64 exec, m`, or a
/// compiler's `s_wqm_b64 s, m; s_and_b64 exec, exec, s`). Before that only scalar ALU ops, scalar
/// loads and waits: their writes are the wave's, followed as stale where the sides join.
fn narrows_before_lane_work(insts: &[D], sw: &[u128], g: &Cfg, b: usize, at_vote: Half) -> bool {
    let mut h = at_vote;
    for i in g.range(b) {
        if has_pair(h.z, EXEC) {
            return true;
        }
        let d = &insts[i];
        let n = d.name.as_str();
        let scalar = match d.enc {
            Enc::Sop1 | Enc::Sop2 | Enc::Sopc | Enc::Sopk => !n.starts_with("S_SETREG") && !n.starts_with("S_GETREG") && !n.contains("PC_B64"),
            Enc::Smem => n.contains("LOAD"),
            Enc::Sopp => n.starts_with("S_WAITCNT") || n == "S_NOP",
            _ => false,
        };
        if !scalar {
            return false;
        }
        zero_step(d, sw[i], &mut h);
    }
    has_pair(h.z, EXEC)
}

/// xcl's `classify`, over the instruction views: the reasons (empty: Exact).
fn classify_decoded(insts: &[D]) -> BTreeSet<&'static str> {
    let mut reasons = BTreeSet::new();
    let Some(g) = Cfg::new(insts) else {
        reasons.insert("no-graph");
        return reasons;
    };
    let nb = g.start.len();
    if nb == 0 {
        return reasons;
    }
    let ipdom = g.post_dominators();
    let sw: Vec<u128> = insts.iter().map(scalar_writes).collect();
    // first without stale scalars, to find the votes; then with the scalars each vote's
    // dependent blocks write marked stale where its sides join
    let none = vec![Vec::new(); nb];
    let at = fixed_point(insts, &g, &none);
    let stale = stale_at_joins(insts, &sw, &g, &ipdom, &at);
    let at = fixed_point(insts, &g, &stale);
    // the sinks, and the votes, from the fixed point
    for b in 0..nb {
        let Some(mut st) = at[b].clone() else { continue };
        mark_stale(&mut st, &stale[b]);
        let range = g.range(b);
        let last = range.end - 1;
        for i in range {
            if i == last {
                vote(insts, &sw, &g, &ipdom, b, &insts[i], &st, &mut reasons);
            }
            step(&insts[i], &mut st, &mut reasons);
        }
    }
    reasons
}

/// Marks scalar registers (and SCC) stale.
fn mark_stale(st: &mut State, regs: &[Reg]) {
    for r in regs {
        match *r {
            Reg::S(s) => st.s[s] |= STALE_SCALAR,
            Reg::Scc => st.scc |= STALE_SCALAR,
            Reg::V(_) => {}
        }
    }
}

/// Block-entry states to a fixed point (the bits only grow, the flags only fall), with the
/// registers of `stale[b]` marked stale on entering block `b`.
fn fixed_point(insts: &[D], g: &Cfg, stale: &[Vec<Reg>]) -> Vec<Option<State>> {
    let nb = g.start.len();
    let mut at: Vec<Option<State>> = vec![None; nb];
    at[0] = Some(State::entry());
    let mut work = BTreeSet::from([0usize]);
    let mut scratch = BTreeSet::new();
    while let Some(b) = work.pop_first() {
        let mut st = at[b].clone().expect("queued with a state");
        mark_stale(&mut st, &stale[b]);
        for i in g.range(b) {
            step(&insts[i], &mut st, &mut scratch);
        }
        for &s in &g.succ[b] {
            let new = match &at[s] {
                None => st.clone(),
                Some(old) => old.join(&st),
            };
            if at[s].as_ref() != Some(&new) {
                at[s] = Some(new);
                work.insert(s);
            }
        }
    }
    at
}

/// Whether the conditional branch `d` (state `st` before it) is a vote a half can take
/// differently from the wave: on a mask, unless every half has lanes and votes as the wave does
/// (the launch EXEC, or a mask that is all or none of its lanes).
fn is_vote(d: &D, st: &State) -> bool {
    let mask = match d.name.as_str() {
        "S_CBRANCH_EXECZ" | "S_CBRANCH_EXECNZ" => EXEC,
        "S_CBRANCH_VCCZ" | "S_CBRANCH_VCCNZ" => VCC,
        "S_CBRANCH_SCC0" | "S_CBRANCH_SCC1" => match st.scc_vote {
            SccVote::No => return false,
            SccVote::Exec => EXEC,
            SccVote::Mask(Some(m)) => m,
            SccVote::Mask(None) => return true,
        },
        _ => return false,
    };
    let uniform = mask == EXEC || (mask + 1 < 128 && st.uniform_mask[mask] && st.uniform_mask[mask + 1]);
    !(st.exec_launch && uniform)
}

/// For each block, the scalars (SGPRs, VCC, M0, SCC; not EXEC, which the join restores) that
/// the blocks a vote decides write, when the vote's sides join there: a half that took the
/// other side has not written them, so what it reads there is not the wave's.
fn stale_at_joins(insts: &[D], sw: &[u128], g: &Cfg, ipdom: &[PostDom], at: &[Option<State>]) -> Vec<Vec<Reg>> {
    let nb = g.start.len();
    let cd = g.control_dependence(ipdom);
    // branch block -> the blocks that depend on it
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); nb];
    for (x, on) in cd.iter().enumerate() {
        for &c in on {
            dependents[c].push(x);
        }
    }
    let mut stale: Vec<Vec<Reg>> = vec![Vec::new(); nb];
    for b in 0..nb {
        let (Some(st), PostDom::Block(join)) = (&at[b], ipdom[b]) else { continue };
        let mut st = st.clone();
        let range = g.range(b);
        let last = range.end - 1;
        let mut scratch = BTreeSet::new();
        for i in range.start..last {
            step(&insts[i], &mut st, &mut scratch);
        }
        if g.succ[b].len() < 2 || !is_vote(&insts[last], &st) {
            continue;
        }
        // what is zero, on the side the wave runs, in a half that skipped it
        let run_side = sides(g, b, &insts[last], &st)
            .filter(|&(_, run, _)| run != join)
            .map(|(_, run, mask)| zero_entries(insts, sw, g, b, run, Some(join), zero_at_vote(insts, sw, g, b, mask.unwrap_or(EXEC))));
        // the blocks it decides, nested branches' blocks included
        let mut region: BTreeSet<usize> = BTreeSet::new();
        let mut todo = dependents[b].clone();
        while let Some(x) = todo.pop() {
            if region.insert(x) {
                todo.extend(dependents[x].iter().copied());
            }
        }
        for &x in &region {
            let mut z = run_side.as_ref().and_then(|e| e[x]);
            for i in g.range(x) {
                let d = &insts[i];
                // s &= ~m, s |= m with m zero in that half: its bits stay what they were
                let accumulates = matches!(d.name.as_str(), "S_ANDN2_B64" | "S_OR_B64")
                    && d.f("SDST") == d.f("SSRC0")
                    && d.f("SSRC1").zip(z).is_some_and(|(m, h)| has_pair(h.z, m as usize));
                if let Some(z) = z.as_mut() {
                    zero_step(d, sw[i], z);
                }
                for &w in &d.uses.writes {
                    if accumulates && matches!(w, Reg::S(_)) {
                        continue;
                    }
                    let scalar = matches!(w, Reg::Scc) || matches!(w, Reg::S(s) if s != EXEC && s != EXEC + 1);
                    if scalar && !stale[join].contains(&w) {
                        stale[join].push(w);
                    }
                }
            }
        }
    }
    stale
}

/// A conditional branch at the end of block `b` (state `st` before it): a vote that may run a
/// side the console wave did not run for the half's lanes.
#[allow(clippy::too_many_arguments)]
fn vote(insts: &[D], sw: &[u128], g: &Cfg, ipdom: &[PostDom], b: usize, d: &D, st: &State, out: &mut BTreeSet<&'static str>) {
    let Some((empty, run, mask)) = sides(g, b, d, st) else {
        // SCC from a mask in no register: counted as a vote that may differ
        if g.succ[b].len() == 2 && matches!(d.name.as_str(), "S_CBRANCH_SCC0" | "S_CBRANCH_SCC1") && st.scc_vote == SccVote::Mask(None) {
            out.insert("vote-branch");
        }
        return;
    };
    // a mask every half votes on as the wave does: all or none of the active lanes. Under the
    // launch EXEC every half has lanes; under a narrowed one a half may have none and take the
    // other side, which is exact only if neither side turns lanes on again before the join
    let m = mask.unwrap_or(EXEC);
    let uniform = m + 1 < 128 && st.uniform_mask[m] && st.uniform_mask[m + 1];
    if mask.is_none() && st.exec_launch {
        return;
    }
    if uniform {
        if st.exec_launch {
            return;
        }
        if let PostDom::Block(join) = ipdom[b] {
            let zero = zero_at_vote(insts, sw, g, b, EXEC);
            let side = |s: usize| s == join || !widens_after(insts, sw, g, b, s, Some(join), zero, true);
            if side(run) && side(empty) {
                return;
            }
        }
    }
    let joins = ipdom[b] == PostDom::Block(empty);
    let skips = joins || leaves_nothing(insts, g, empty);
    // … and keeps it narrowed: up to the join, or for a way out that ends the program, to the end
    // (a half whose mask is empty must not miss lanes the wave turns on again)
    let at_vote = zero_at_vote(insts, sw, g, b, mask.unwrap_or(EXEC));
    let narrows = (mask.is_none() || narrows_before_lane_work(insts, sw, g, run, at_vote))
        && !widens_after(insts, sw, g, b, run, if joins { Some(empty) } else { None }, at_vote, mask.is_none());
    if !(skips && narrows) {
        out.insert(if mask.is_none() { "unsafe-execz-branch" } else { "vote-branch" });
    }
}
