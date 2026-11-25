//! A 1×1×1 workgroup starts with only lane zero active. Lower its election
//! and prefix counts when all paths preserve that property.
use super::{compute_group, writes_scalar, Emitter, Enc, Insn, Opnd, ShaderExtract, Stage};
use std::collections::{BTreeSet, VecDeque};

pub(super) fn prepare(ex: &ShaderExtract, code: &mut [Insn]) {
    if ex.stage != Stage::Compute
        || compute_group(ex) != [1, 1, 1]
        || !code
            .iter()
            .any(|i| i.mnemonic == "s_ff1_i32_b64" && i.src == [Opnd::ExecLo])
    {
        return;
    }
    if !only_lane_zero(code) && !reduce_singleton_sum(code) {
        return;
    }
    for i in code {
        match i.mnemonic.as_str() {
            "s_ff1_i32_b64" if i.src == [Opnd::ExecLo] => i.mnemonic = "s_ff1_i32_b32".into(),
            "v_mbcnt_lo_u32_b32" | "v_mbcnt_hi_u32_b32" => {
                // No bits precede lane zero in either half of a wave.
                i.enc = Enc::Vop1;
                i.op = 1;
                i.mnemonic = "v_mov_b32".into();
                i.src = vec![i.src[1]];
            }
            _ => {}
        }
    }
}

/// A compiler can temporarily unmask the other 63 lanes for a reduction.
/// For an unsigned sum, CNDMASK supplies zero in each one. Keep lane zero's
/// accumulator, the scalar broadcast, and the intermediate cross-row register.
fn reduce_singleton_sum(code: &mut [Insn]) -> bool {
    let Some(first) = code
        .iter()
        .position(|op| op.mnemonic == "v_add_nc_u32" && op.src.first() == Some(&Opnd::Raw(233)))
    else {
        return false;
    };
    let Some(start) = first.checked_sub(2) else {
        return false;
    };
    if code[..start].iter().any(|op| {
        op.mnemonic.contains("exec")
            || op.mnemonic.starts_with("v_cmpx")
            || op
                .dst
                .iter()
                .any(|r| matches!(r, Opnd::ExecLo | Opnd::ExecHi))
    }) {
        return false;
    }
    let mut trial = code.to_vec();
    // The general reduction decoder verifies every permute and lane read.
    let Ok((64, end)) = super::wave_sum::lower(&mut trial, first) else {
        return false;
    };
    if super::wave_sum::check_control_flow(&trial, 64).is_err() {
        return false;
    }
    trial[start].mnemonic = "s_mov_b64".into();
    trial[start].src = vec![Opnd::ExecLo];
    if !only_lane_zero(&trial) {
        return false;
    }
    // Retain the native SAVEEXEC and restore. Its temporary all-ones EXEC
    // affects only this verified sequence, including its original SCC write.
    for (i, op) in code[first..end - 1].iter_mut().enumerate() {
        match i {
            0..=3 => {
                op.enc = Enc::Sopp;
                op.mnemonic = "s_nop".into();
                op.op = 0;
                op.dst.clear();
                op.src.clear();
            }
            4 => {
                op.enc = Enc::Vop1;
                op.op = 1;
                op.mnemonic = "v_mov_b32".into();
                op.src = vec![Opnd::ConstInt(0)];
            }
            6 => {
                op.enc = Enc::Sop1;
                op.op = 3;
                op.mnemonic = "s_mov_b32".into();
                op.dst = trial[first + i].dst.clone();
                op.src = vec![Opnd::ConstInt(0)];
            }
            8 => {
                op.enc = Enc::Vop1;
                op.op = 2;
                op.mnemonic = "v_readfirstlane_b32".into();
                op.dst = trial[first + i].dst.clone();
                op.src.truncate(1);
            }
            _ => (),
        }
    }
    true
}

fn only_lane_zero(code: &[Insn]) -> bool {
    let mut pairs = BTreeSet::new();
    for i in code {
        let op = i.mnemonic.as_str();
        if op.contains("saveexec") || op.contains("wrexec") {
            if op != "s_and_saveexec_b64" {
                return false;
            }
        } else if matches!(i.dst.first(), Some(Opnd::ExecLo | Opnd::ExecHi)) {
            let [Opnd::Sgpr(r)] = i.src[..] else {
                return false;
            };
            if op != "s_mov_b64" || r >= 105 {
                return false;
            }
            pairs.insert(r);
        }
        // These instructions have implicit register writes or control transfers.
        if !i.is_complete()
            || i.enc == Enc::Unknown
            || op.contains('<')
            || op.contains("_co_")
            || op.contains("movrel")
            || op.contains("subvector")
            || op.starts_with("v_div_scale")
            || matches!(
                op,
                "v_mad_u64_u32"
                    | "v_mad_i64_i32"
                    | "s_setpc_b64"
                    | "s_swappc_b64"
                    | "s_rfe_b64"
                    | "s_call_b64"
                    | "s_trap"
            )
            || (op.starts_with("s_cbranch_") && Emitter::branch_cond(i.op).is_none())
        {
            return false;
        }
    }

    // Each set bit identifies an SGPR pair that must hold a saved EXEC (0 or 1).
    // Merging by intersection also checks restores reached through a backedge.
    let mut inputs = vec![None; code.len()];
    inputs[0] = Some(0u128);
    let mut pending = VecDeque::from([0]);
    while let Some(index) = pending.pop_front() {
        let i = &code[index];
        let mut saved = inputs[index].expect("reachable instruction");
        if i.dst.first() == Some(&Opnd::ExecLo) {
            let Opnd::Sgpr(r) = i.src[0] else {
                return false;
            };
            if saved & (1u128 << r) == 0 {
                return false;
            }
        }
        for &r in &pairs {
            if writes_scalar(i, r) || writes_scalar(i, r + 1) {
                saved &= !(1u128 << r);
            }
        }
        if i.mnemonic == "s_and_saveexec_b64"
            || (i.mnemonic == "s_mov_b64" && i.src == [Opnd::ExecLo])
        {
            if let [Opnd::Sgpr(r)] = i.dst[..] {
                saved |= 1u128 << r;
            }
        }
        let mut merge = |next: usize| {
            let Some(slot) = inputs.get_mut(next) else {
                return;
            };
            let joined = slot.map_or(saved, |other| other & saved);
            if *slot != Some(joined) {
                *slot = Some(joined);
                pending.push_back(next);
            }
        };
        if i.enc == Enc::Sopp && Emitter::branch_cond(i.op).is_some() {
            let pc = i.offset as i64 + 4 + 4 * i.aux.imm;
            let Ok(next) = code.binary_search_by_key(&pc, |ins| ins.offset as i64) else {
                return false;
            };
            merge(next);
            if i.op == 2 {
                continue;
            }
        }
        if i.mnemonic != "s_endpgm" {
            merge(index + 1);
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::only_lane_zero;
    use crate::rdna2::disassemble;

    #[test]
    fn restore_requires_a_snapshot_on_every_path() {
        let save = 0xbe8a246a; // s_and_saveexec_b64 s[10:11], vcc
        let restore = 0xbefe040a; // s_mov_b64 exec, s[10:11]
        let overwrite_high = 0xbe8b0381; // s_mov_b32 s11, 1
        let skip = 0xbf850001; // s_cbranch_scc1 +1
        let end = 0xbf810000;
        let admitted = |words: &[u32]| {
            let bytes = words
                .iter()
                .flat_map(|w| w.to_le_bytes())
                .collect::<Vec<_>>();
            only_lane_zero(&disassemble(&bytes))
        };
        assert!(admitted(&[save, restore, end]));
        assert!(!admitted(&[skip, save, restore, end]));
        assert!(!admitted(&[save, overwrite_high, restore, end]));
    }
}
