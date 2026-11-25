//! Some compilers put zero in an SGPR for the MUBUF byte offset. A zero
//! reaching every predecessor selects the same addressing as an inline zero.
use super::{writes_scalar, Emitter};
use crate::rdna2::{Enc, Insn, Opnd};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

// Track only ordinary SGPR offsets; special scalar encodings are not SGPRs.
fn offset_register(ins: &Insn) -> Option<u8> {
    match ins.enc {
        Enc::Mubuf => match ins.src.get(2) {
            Some(Opnd::Sgpr(reg @ 0..=105)) => Some(*reg),
            _ => None,
        },
        Enc::Smem if ins.op <= 4 => {
            let reg = ins.raw.get(1)? >> 25;
            (reg <= 105).then_some(reg as u8)
        }
        _ => None,
    }
}

pub(super) fn proven_zero(insns: &[Insn]) -> BTreeSet<(usize, u8)> {
    let mut used = 0u128;
    for ins in insns {
        if let Some(reg) = offset_register(ins) {
            used |= 1u128 << reg;
        }
    }
    if used == 0
        || insns.iter().any(|i| {
            !i.is_complete()
                || i.enc == Enc::Unknown
                || i.mnemonic.contains('<')
                || i.mnemonic.starts_with("s_movreld_")
                || i.mnemonic.starts_with("s_subvector_loop_")
                || (i.mnemonic.starts_with("s_cbranch_") && Emitter::branch_cond(i.op).is_none())
                || matches!(
                    i.mnemonic.as_str(),
                    "s_setpc_b64"
                        | "s_swappc_b64"
                        | "s_rfe_b64"
                        | "s_call_b64"
                        | "s_movrelsd_2_b32"
                        | "s_trap"
                )
        })
    {
        return BTreeSet::new();
    }
    // Include ordinary SGPRs that feed a tracked offset through scalar copies.
    // The closure is bounded by the 106 ordinary scalar registers.
    loop {
        let before = used;
        for ins in insns {
            if let ("s_mov_b32", [Opnd::Sgpr(dst @ 0..=105)], [Opnd::Sgpr(src @ 0..=105)]) =
                (ins.mnemonic.as_str(), ins.dst.as_slice(), ins.src.as_slice())
            {
                if used & (1u128 << dst) != 0 {
                    used |= 1u128 << src;
                }
            }
        }
        if used == before { break; }
    }
    let registers: Vec<_> = (0..106).filter(|&r| used & (1u128 << r) != 0).collect();
    let mut input = vec![None; insns.len()];
    let mut work = VecDeque::from([0]);
    input[0] = Some(0u128);
    while let Some(index) = work.pop_front() {
        let ins = &insns[index];
        let incoming_zero = input[index].expect("queued reachable instruction");
        let mut zero = incoming_zero;
        for &reg in &registers {
            if writes_scalar(ins, reg) {
                zero &= !(1u128 << reg);
            }
        }
        if ins.enc == Enc::Vop3 {
            // VOP3B carries have a second destination in SDST, separate
            // from the VGPR in the decoded destination list.
            if ins.mnemonic.contains("_co_")
                || matches!(
                    ins.mnemonic.as_str(),
                    "v_mad_u64_u32" | "v_mad_i64_i32" | "v_div_scale_f32" | "v_div_scale_f64"
                )
            {
                let reg = (ins.raw[0] >> 8) & 127;
                zero &= !(3u128 << reg);
            }
            if ins.mnemonic == "v_readfirstlane_b32" {
                zero &= !(1u128 << (ins.raw[0] & 127));
            }
        }
        if let ("s_mov_b32", [Opnd::Sgpr(reg)], [Opnd::ConstInt(0) | Opnd::Literal(0)]) = (
            ins.mnemonic.as_str(),
            ins.dst.as_slice(),
            ins.src.as_slice(),
        ) {
            zero |= used & (1u128 << reg);
        }

        // Read the incoming source before killing destination bits (self-copies
        // and overlapping definitions must not use the outgoing lattice state).
        if let ("s_mov_b32", [Opnd::Sgpr(dst @ 0..=105)], [Opnd::Sgpr(src @ 0..=105)]) =
            (ins.mnemonic.as_str(), ins.dst.as_slice(), ins.src.as_slice())
        {
            if incoming_zero & (1u128 << src) != 0 {
                zero |= used & (1u128 << dst);
            }
        }

        let mut merge = |next: usize| {
            let Some(slot) = input.get_mut(next) else {
                return;
            };
            let incoming = slot.map_or(zero, |prev| prev & zero);
            if *slot != Some(incoming) {
                *slot = Some(incoming);
                work.push_back(next);
            }
        };
        if ins.enc == Enc::Sopp && Emitter::branch_cond(ins.op).is_some() {
            let pc = ins.offset as i64 + 4 + 4 * ins.aux.imm;
            let Ok(next) = insns.binary_search_by_key(&pc, |i| i.offset as i64) else {
                return BTreeSet::new();
            };
            merge(next);
            if ins.op == 2 {
                continue;
            }
        }
        if ins.mnemonic != "s_endpgm" {
            merge(index + 1);
        }
    }
    let mut offsets = BTreeSet::new();
    for (ins, zero) in insns.iter().zip(input) {
        if let Some(reg) = offset_register(ins) {
            if zero.unwrap_or(0) & (1u128 << reg) != 0 {
                offsets.insert((ins.offset, reg));
            }
        }
    }
    offsets
}

pub(super) fn proven_smem_constants(insns: &[Insn]) -> BTreeMap<(usize, u8), u32> {
    super::scalar_values::at_reads(insns, super::scalar_values::Proof::Exact, |ins| {
        if ins.enc == Enc::Smem { offset_register(ins) } else { None }
    })
}


#[cfg(test)]
#[path = "../tests/hlsl/mubuf_offsets.rs"]
mod tests;
