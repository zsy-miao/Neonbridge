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

// A bounded forward constant lattice for static scalar table addressing.
// Only exact values agreed on by all reachable predecessors are retained.
// Unknown writes kill facts, including the secondary destinations handled by
// the existing zero-offset analysis. Larger programs retain the old zero proof.
pub(super) fn proven_smem_constants(insns: &[Insn]) -> BTreeMap<(usize, u8), u32> {
    if insns.is_empty() || insns.len() > 32768
        || !insns.iter().any(|i| i.enc == Enc::Smem && offset_register(i).is_some())
        || insns.iter().any(|i| !i.is_complete() || i.enc == Enc::Unknown
            || i.mnemonic.contains('<') || i.mnemonic.starts_with("s_movreld_")
            || i.mnemonic.starts_with("s_subvector_loop_")
            || (i.mnemonic.starts_with("s_cbranch_") && Emitter::branch_cond(i.op).is_none())
            || matches!(i.mnemonic.as_str(), "s_setpc_b64" | "s_swappc_b64"
                | "s_rfe_b64" | "s_call_b64" | "s_movrelsd_2_b32" | "s_trap"))
    { return BTreeMap::new(); }

    type State = BTreeMap<u8, u32>;
    let mut input: Vec<Option<State>> = vec![None; insns.len()];
    input[0] = Some(State::new());
    let mut work = VecDeque::from([0]);
    let mut iterations = 0usize;
    while let Some(index) = work.pop_front() {
        iterations += 1;
        if iterations > insns.len() * 128 { return BTreeMap::new(); }
        let ins = &insns[index];
        let incoming = input[index].as_ref().expect("reachable");
        let mut out = incoming.clone();
        out.retain(|&reg, _| !writes_scalar(ins, reg));
        if ins.enc == Enc::Vop3 {
            if ins.mnemonic.contains("_co_")
                || matches!(ins.mnemonic.as_str(), "v_mad_u64_u32" | "v_mad_i64_i32" | "v_div_scale_f32" | "v_div_scale_f64")
            {
                let reg = ((ins.raw[0] >> 8) & 127) as u8;
                out.remove(&reg);
                out.remove(&(reg + 1));
            }
            if ins.mnemonic == "v_readfirstlane_b32" {
                out.remove(&((ins.raw[0] & 127) as u8));
            }
        }
        // Ordinary scalar operations whose emitted uint semantics are exact.
        let value = |op: &Opnd| match op {
            Opnd::Sgpr(r @ 0..=105) => incoming.get(r).copied(),
            Opnd::ConstInt(v) => Some(*v as u32),
            Opnd::Literal(v) => Some(*v),
            _ => None,
        };
        if let [Opnd::Sgpr(dst @ 0..=105)] = ins.dst.as_slice() {
            let known = match (ins.mnemonic.as_str(), ins.src.as_slice()) {
                ("s_mov_b32", [a]) => value(a),
                ("s_add_u32" | "s_add_i32", [a, b]) => value(a).zip(value(b)).map(|(a,b)| a.wrapping_add(b)),
                ("s_sub_u32" | "s_sub_i32", [a, b]) => value(a).zip(value(b)).map(|(a,b)| a.wrapping_sub(b)),
                ("s_and_b32", [a,b]) => value(a).zip(value(b)).map(|(a,b)| a & b),
                ("s_or_b32", [a,b]) => value(a).zip(value(b)).map(|(a,b)| a | b),
                ("s_xor_b32", [a,b]) => value(a).zip(value(b)).map(|(a,b)| a ^ b),
                ("s_lshl_b32", [a,b]) => value(a).zip(value(b)).map(|(a,b)| a << (b & 31)),
                ("s_lshr_b32", [a,b]) => value(a).zip(value(b)).map(|(a,b)| a >> (b & 31)),
                _ => None,
            };
            if let Some(v) = known { out.insert(*dst, v); }
        }
        let mut merge = |next: usize| {
            let Some(slot) = input.get_mut(next) else { return };
            let mut joined = slot.clone().unwrap_or_else(|| out.clone());
            joined.retain(|r, v| out.get(r) == Some(v));
            if slot.as_ref() != Some(&joined) {
                *slot = Some(joined);
                work.push_back(next);
            }
        };
        if ins.enc == Enc::Sopp && Emitter::branch_cond(ins.op).is_some() {
            let target = ins.offset as i64 + 4 + 4 * ins.aux.imm;
            let Ok(next) = insns.binary_search_by_key(&target, |i| i.offset as i64)
                else { return BTreeMap::new(); };
            merge(next);
            if ins.op == 2 { continue; }
        }
        if ins.mnemonic != "s_endpgm" { merge(index + 1); }
    }
    let mut result = BTreeMap::new();
    for (ins, state) in insns.iter().zip(input) {
        if ins.enc == Enc::Smem {
            if let Some(reg) = offset_register(ins) {
                if let Some(v) = state.and_then(|s| s.get(&reg).copied()) {
                    result.insert((ins.offset, reg), v);
                }
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rdna2::disassemble;

    const ZERO: u32 = 0xbec20380; // s_mov_b32 s66, 0
    const ONE: u32 = 0xbec20381; // s_mov_b32 s66, 1
    const USE: [u32; 2] = [0xe0342000, 0x42070202]; // buffer_load_dwordx2 with soffset s66
    const END: u32 = 0xbf810000;

    fn tracked(words: &[u32]) -> BTreeSet<(usize, u8)> {
        let bytes = words
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<_>>();
        proven_zero(&disassemble(&bytes))
    }

    #[test]
    fn every_incoming_path_must_preserve_zero() {
        let branch = 0xbf840001; // s_cbranch_scc0 +1
        let nop = 0xbf800000;
        assert_eq!(
            tracked(&[ZERO, branch, nop, USE[0], USE[1], END]),
            BTreeSet::from([(12, 66)])
        );
        assert!(tracked(&[ZERO, branch, ONE, USE[0], USE[1], END]).is_empty());
        assert!(tracked(&[branch, ZERO, USE[0], USE[1], END]).is_empty());
        // s_cbranch_scc0 -4 returns to the buffer instruction with s66 now equal to 1.
        assert!(tracked(&[ZERO, USE[0], USE[1], ONE, 0xbf84fffc, END]).is_empty());
    }

    #[test]
    fn nonzero_constant_proof_handles_alu_merges_and_clobbers() {
        let load = [0xf4081000, 66u32 << 25];
        let thirty_two = 0xbec203a0;
        let add = 0x80428142; // s_add_u32 s66, s66, 1
        let check = |words: &[u32]| {
            let bytes: Vec<_> = words.iter().flat_map(|v| v.to_le_bytes()).collect();
            proven_smem_constants(&disassemble(&bytes))
        };
        assert_eq!(check(&[thirty_two, add, load[0], load[1], END]).get(&(8,66)), Some(&33));
        let branch = 0xbf840001;
        assert_eq!(check(&[thirty_two, branch, thirty_two, load[0], load[1], END]).get(&(12,66)), Some(&32));
        assert!(check(&[thirty_two, branch, ONE, load[0], load[1], END]).is_empty());
        assert!(!check(&[thirty_two, load[0], load[1], load[0], load[1], END]).contains_key(&(12,66)));
        assert!(check(&[thirty_two, 0xd70f4200, 0x02020300, load[0], load[1], END]).is_empty());
    }

    #[test]
    fn copied_offsets_require_proven_sources_on_every_path() {
        let zero65 = 0xbec10380; // s_mov_b32 s65, 0
        let copy = 0xbec20341;   // s_mov_b32 s66, s65
        let self_copy = 0xbec20342;
        let load = [0xf4081000, 66u32 << 25];
        assert_eq!(tracked(&[zero65, copy, self_copy, load[0], load[1], END]),
                   BTreeSet::from([(12, 66)]));
        assert!(tracked(&[copy, load[0], load[1], END]).is_empty());
        let branch = 0xbf840001;
        assert!(tracked(&[branch, zero65, copy, load[0], load[1], END]).is_empty());
    }

    #[test]
    fn scalar_load_offset_must_be_zero_on_all_paths() {
        let load = [0xf4081000, 66u32 << 25];
        let branch = 0xbf840001;
        assert_eq!(tracked(&[ZERO, load[0], load[1], END]), BTreeSet::from([(4, 66)]));
        assert!(tracked(&[ONE, load[0], load[1], END]).is_empty());
        assert!(tracked(&[ZERO, branch, ONE, load[0], load[1], END]).is_empty());
        assert!(tracked(&[branch, ZERO, load[0], load[1], END]).is_empty());
        assert!(tracked(&[ZERO, load[0], load[1], load[0], load[1], END]).contains(&(4, 66)));
        assert!(!tracked(&[ZERO, load[0], load[1], load[0], load[1], END]).contains(&(12, 66)));
    }

    #[test]
    fn overlapping_and_secondary_scalar_destinations_clobber_zero() {
        for writer in [
            [0xf4081000, 0xfa000000], // s_load_dwordx4 s[64:67], s[0:1], 0
            [0xd70f4200, 0x02020300], // v_add_co_u32 v0, s66, v0, v1
        ] {
            assert!(tracked(&[ZERO, writer[0], writer[1], USE[0], USE[1], END]).is_empty());
        }
    }
}
