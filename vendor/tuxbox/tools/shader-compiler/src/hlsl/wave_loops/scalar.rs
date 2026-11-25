//! Scalar operations that preserve wave uniformity in the full-mask emitter.
use super::super::{ray_resolve, wave_sum, Emitter, Enc, Insn, Opnd, Res, LANE_TEMP_BASE};
use super::{first_lane, scalar, Flow};
use crate::rdna2::clobbers_scalar;
use std::collections::BTreeSet;

pub(super) struct Effect {
    pub(super) scalar_writes: u128,
    pub(super) writes_scc: bool,
}

impl Effect {
    pub(super) fn new(op: &Insn) -> Self {
        let m = op.mnemonic.as_str();
        let mut scalar_writes = 0;
        if let Some((reg, _)) = first_lane(op) {
            scalar_writes = 1 << reg;
        } else if m.starts_with("v_cmp_") {
            if let Some(reg) = op.dst.first().copied().and_then(scalar) {
                scalar_writes = 1 << reg;
            }
        } else if !m.starts_with("v_cmpx_")
            && !m.starts_with("s_cmpk_")
            && !matches!(m, "s_setreg_b32" | "s_setreg_imm32_b32")
            && (!op.dst.is_empty() || op.enc == Enc::Vop3)
        {
            scalar_writes = (0..106)
                .filter(|&reg| clobbers_scalar(op, reg))
                .fold(0, |set, reg| set | (1 << reg));
        }
        let writes_scc = op.dst.contains(&Opnd::Scc)
            || matches!(op.enc, Enc::Sop1 | Enc::Sop2 | Enc::Sopk | Enc::Sopc)
                && !matches!(
                    m,
                    "s_mov_b32"
                        | "s_mov_b64"
                        | "s_movk_i32"
                        | "s_ff1_i32_b32"
                        | "s_mul_i32"
                        | "s_mulk_i32"
                        | "s_cselect_b32"
                        | "s_cselect_b64"
                        | "s_getpc_b64"
                        | "s_setreg_b32"
                        | "s_setreg_imm32_b32"
                        | "s_waitcnt_vscnt"
                        | "s_waitcnt_vmcnt"
                        | "s_waitcnt_expcnt"
                        | "s_waitcnt_lgkmcnt"
                );
        Self {
            scalar_writes,
            writes_scc,
        }
    }

    pub(super) fn writes(&self, reg: u8) -> bool {
        self.scalar_writes & (1 << reg) != 0
    }
}

fn scalar_value(op: Opnd) -> bool {
    matches!(
        op,
        Opnd::Sgpr(0..=105)
            | Opnd::ExecLo
            | Opnd::VccLo
            | Opnd::Scc
            | Opnd::M0
            | Opnd::Null
            | Opnd::ConstInt(_)
            | Opnd::ConstF32(_)
            | Opnd::Literal(_)
    ) || matches!(op, Opnd::Temp(n) if n >= LANE_TEMP_BASE)
}

fn root_pointer(
    insns: &[Insn],
    flow: &Flow,
    effects: &[Effect],
    at: usize,
    base: u8,
    table: u8,
) -> bool {
    let mut pending = vec![(at, base)];
    let mut seen = BTreeSet::new();
    while let Some((at, reg)) = pending.pop() {
        if !seen.insert((at, reg)) {
            continue;
        }
        let (defs, fully_defined) =
            flow.reaching_writes(at, None, None, |i| effects[i].writes(reg));
        if !fully_defined && reg != table {
            return false;
        }
        for i in defs {
            let op = &insns[i];
            let [Opnd::Sgpr(src @ 0..=105)] = op.src[..] else {
                return false;
            };
            if op.mnemonic != "s_mov_b32" || op.dst != [Opnd::Sgpr(reg)] {
                return false;
            }
            pending.push((i, src));
        }
    }
    true
}

/// Recover all four descriptor dwords before treating the read as uniform.
/// The resolver may keep a descriptor at a merge with an unknown arm.
fn fixed_buffer(em: &Emitter<'_>, flow: &Flow, effects: &[Effect], at: usize, kind: u32) -> bool {
    let code = &em.insns;
    let [Opnd::Sgpr(base @ 0..=102)] = code[at].src[..] else {
        return false;
    };
    if em.bsel.contains_key(&at) {
        return false;
    }
    let mask = 0b1111 << base;
    let Some(defs) = flow.definitions(at, 0, None, |i| effects[i].scalar_writes & mask != 0) else {
        return false;
    };
    let mut identity = None;
    for i in defs {
        let op = &code[i];
        let Some(&(dest, id)) = em.dsel_loads.get(&i) else {
            return false;
        };
        let [Opnd::Sgpr(pointer @ 0..=104)] = op.src[..] else {
            return false;
        };
        let table = (id >> 24) as u8;
        if op.enc != Enc::Smem
            || op.op != 2
            || op.raw[1] >> 25 != 0x7d
            || dest != u16::from(base)
            || identity.is_some_and(|other| id != other)
            || !matches!(em.table_binding_space(table, id & 0xff_ffff, 32), Ok((Some(k), _, _)) if k == kind)
        {
            return false;
        }
        // The full-mask compute ABI reflects table pointers in one user SGPR.
        let reflected =
            tuxbox_dxbc::compute_table::user_data_records(&em.ex.fssm).is_some_and(|mut rows| {
                rows.any(|[class, _, width, _, location]| {
                    class == 3 && width == 1 && location & 0xff == u32::from(table)
                })
            });
        if !reflected || !root_pointer(code, flow, effects, i, pointer, table) {
            return false;
        }
        identity = Some(id);
    }
    true
}

pub(super) fn scalar_step(em: &Emitter<'_>, flow: &Flow, effects: &[Effect], i: usize) -> bool {
    let op = &em.insns[i];
    let m = op.mnemonic.as_str();
    match op.enc {
        Enc::Sop1 | Enc::Sop2 | Enc::Sopc | Enc::Sopk => {
            if m == ray_resolve::READ_LANE {
                return matches!(op.src[..], [Opnd::Vgpr(_), lane] if scalar_value(lane));
            }
            // The scalar emitter uses ordinary integer operations, the PC
            // constant, and mode state written by scalar instructions.
            !m.contains("movrel")
                && !m.contains("memtime")
                && !m.contains("realtime")
                && op.src.iter().all(|&src| scalar_value(src))
        }
        Enc::Smem => {
            let offset = op.raw[1] >> 25;
            if !matches!(offset, 0..=105 | 0x7d) {
                return false;
            }
            match em.res.get(&i).map(Vec::as_slice) {
                Some([Res::Cb(_)]) => fixed_buffer(em, flow, effects, i, 2),
                Some([Res::Raw(_, false) | Res::Buf(_, false)]) => {
                    fixed_buffer(em, flow, effects, i, 0)
                }
                Some([Res::HeapLoad(0..=105) | Res::HeapBuffer(0..=105)]) => em.compute_heap,
                Some([Res::CodeTbl | Res::HeapSamplerLoad(_)]) => true,
                None => op.op <= 4, // Descriptor-only load resolved before emission.
                _ => false,
            }
        }
        Enc::Vop1 | Enc::Vop2 | Enc::Vop3 | Enc::Vopc => {
            // These scalar destinations are broadcasts, including comparison
            // and carry masks that the full-mask emitter produces with ballots.
            if m == "v_readfirstlane_b32" {
                return first_lane(op).is_some();
            }
            if op.op == wave_sum::READ_LANE
                || m.starts_with("v_cmp_")
                || m.starts_with("v_cmpx_")
                || matches!(
                    m,
                    "v_add_co_u32"
                        | "v_sub_co_u32"
                        | "v_subrev_co_u32"
                        | "v_add_co_ci_u32"
                        | "v_sub_co_ci_u32"
                        | "v_subrev_co_ci_u32"
                        | "v_mad_u64_u32"
                        | "v_mad_i64_i32"
                )
            {
                return true;
            }
            effects[i].scalar_writes == 0
                && !op
                    .dst
                    .iter()
                    .any(|o| matches!(o, Opnd::VccLo | Opnd::ExecLo | Opnd::M0 | Opnd::Scc))
        }
        _ => effects[i].scalar_writes == 0,
    }
}
