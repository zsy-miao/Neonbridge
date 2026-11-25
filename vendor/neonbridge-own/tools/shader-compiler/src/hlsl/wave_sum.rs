//! Fixed-width DPP sums, extrema and prefix scans. Keep intermediate
//! register writes and the restored EXEC observable after each reduction.
use super::{Emitter, Enc, Insn, Opnd};

pub(super) const DPP_REDUCE: u32 = 0x7ff1;
pub(super) const READ_LANE: u32 = 0x7ff2;
// A straight-line DPP8 exchange is not a reduction. Preserve the actual
// partner, and do not let the approximate single-lane retry erase it.
pub(super) const DPP8_EXCHANGE: u32 = 0x7fe7;

/// Prove a full-EXEC, single 32-thread group (also valid as a partial
/// wave64), or a 64-thread group with only within-octet exchanges.
/// No branch, EXEC modification, early return, LDS or unknown operands.
/// Static descriptor loads and a single terminal store can be admitted;
/// the main emitter still resolves their resource and address metadata.
/// DPP8 FI=0 and FI=1 are equivalent here: every selected source lane
/// is active, and no instruction may change EXEC. Each source stays within
/// its octet. Constant readlane is
/// only allowed at 32 threads where wave32/wave64 agree for lanes 0..31.
pub(super) fn straight_exchange(insns: &mut [Insn], group: [u32; 3]) -> bool {
    if !matches!(group, [32, 1, 1] | [64, 1, 1]) || insns.len() > 4096
        || insns.last().is_none_or(|i| i.mnemonic != "s_endpgm")
    { return false; }
    let mut found = false;
    let mut stored = false;
    for (idx, i) in insns.iter().enumerate() {
        if !i.is_complete() { return false; }
        let ordinary = |o: &Opnd| matches!(o, Opnd::Vgpr(_) | Opnd::Sgpr(0..=105)
            | Opnd::ConstInt(_) | Opnd::ConstF32(_) | Opnd::Literal(_));
        if stored && !matches!(i.mnemonic.as_str(), "s_waitcnt" | "s_nop" | "s_endpgm") {
            return false;
        }
        if i.enc == Enc::Vop2 && matches!(i.src.first(), Some(Opnd::Raw(233 | 234))) {
            if !matches!(i.mnemonic.as_str(), "v_add_nc_u32" | "v_and_b32" |
                "v_or_b32" | "v_xor_b32" | "v_min_u32" | "v_max_u32" |
                "v_min_i32" | "v_max_i32")
                || !matches!(i.dst[..], [Opnd::Vgpr(_)])
                || !matches!(i.src[..], [Opnd::Raw(233 | 234), Opnd::Vgpr(_)])
                || i.raw.len() != 2 { return false; }
            found = true;
        } else if i.mnemonic == "v_readlane_b32" {
            let [Opnd::Vgpr(dst @ 0..=105)] = i.dst[..] else { return false };
            let (Some(Opnd::Vgpr(src)), Some(Opnd::ConstInt(lane @ 0..=31))) =
                (i.src.first(), i.src.get(1)) else { return false };
            if group[0] != 32 || i.raw.as_slice() !=
                [0xd7600000 | u32::from(dst), ((128 + *lane as u32) << 9) | 256 | u32::from(*src)]
            { return false; }
        } else if i.enc == Enc::Smem && i.mnemonic == "s_load_dwordx4" {
            // Descriptor loads only; actual resource provenance is checked
            // by the normal resource resolver. No runtime scalar offset.
            if i.raw.get(1).is_none_or(|w| w >> 25 != 0x7d)
                || !i.src.iter().chain(&i.dst).all(ordinary) { return false; }
        } else if i.enc == Enc::Mubuf && i.mnemonic == "buffer_store_dword" {
            // One terminal write, with no subsequent shared exchange or
            // memory read. The normal emitter validates address and binding.
            if !found || !i.src.iter().chain(&i.dst).all(ordinary) { return false; }
            stored = true;
        } else if i.enc == Enc::Sopp {
            if !(matches!(i.mnemonic.as_str(), "s_nop" | "s_waitcnt")
                || i.mnemonic == "s_endpgm" && idx+1 == insns.len()) { return false; }
        } else if !(matches!(i.mnemonic.as_str(), "v_mov_b32" | "v_add_nc_u32"
            | "v_sub_nc_u32" | "v_subrev_nc_u32" | "v_and_b32" | "v_or_b32"
            | "v_xor_b32" | "v_min_u32" | "v_max_u32" | "v_lshlrev_b32"
            | "v_lshrrev_b32" | "s_mov_b32" | "s_movk_i32")
            && i.src.iter().chain(&i.dst).all(ordinary)) { return false; }
    }
    if !found { return false; }
    for i in insns {
        if i.enc == Enc::Vop2 && matches!(i.src.first(), Some(Opnd::Raw(233 | 234))) {
            let control = i.raw[1];
            i.op = DPP8_EXCHANGE;
            i.src[0] = Opnd::Vgpr(control as u8);
            i.aux.imm = i64::from(control >> 8);
        } else if i.mnemonic == "v_readlane_b32" {
            let Opnd::Vgpr(dst) = i.dst[0] else { unreachable!() };
            i.op = READ_LANE;
            i.dst[0] = Opnd::Sgpr(dst);
            i.src.truncate(2);
        }
    }
    true
}

/// Divergent reductions: per-half wave intrinsics over the participating lanes.
pub(super) const HALF_REDUCE: u32 = 0x7ff3;
pub(super) const HALF_READ: u32 = 0x7ff4;

// Reductions may have a completed vector load and a second predicate.
// The scalar AND intersects that predicate with the saved EXEC before the
// CNDMASK sets excluded lanes to the reduction's identity.
enum NeutralIdentity {
    // Immediate neutral: v_or_b32 / v_max_u32 (0), v_and_b32 / v_min_u32 (-1),
    // v_add_nc_u32 (0) and v_add_f32 (0.0 == 0x00000000). Accept ConstInt or
    // Literal encodings of the same bits.
    Immediate(i64),
    // Neutral held in a VGPR: floating extrema materialize +/-inf with a
    // literal move (v_mov_b32) before the AND and CNDMASK.
    LiteralBits(u32),
}

fn masked_prologue(insns: &[Insn], first: usize, identity: NeutralIdentity) -> Option<(usize, Opnd, Opnd)> {
    let select = insns.get(first.checked_sub(1)?)?;
    let [ident, _, mask @ Opnd::Sgpr(_)] = select.src[..] else {
        return None;
    };
    let (expected, want_reg, want_literal) = match (identity, ident) {
        (NeutralIdentity::Immediate(v), Opnd::ConstInt(k)) if k == v => {
            (Opnd::ConstInt(v), None, None)
        }
        (NeutralIdentity::Immediate(v), Opnd::Literal(k)) if k == v as u32 => {
            (Opnd::Literal(k), None, None)
        }
        (NeutralIdentity::LiteralBits(bits), Opnd::Vgpr(reg)) => {
            (Opnd::Vgpr(reg), Some(reg), Some(bits))
        }
        _ => return None,
    };
    let mut mask_sources = None;
    let mut ident_reg_found = want_reg.is_none();
    for (index, op) in insns[..first - 1].iter().enumerate().rev() {
        match op.mnemonic.as_str() {
            "s_waitcnt" | "v_nop" => (),
            "v_mov_b32" if want_reg.is_some() => {
                let [Opnd::Vgpr(reg)] = op.dst[..] else { return None };
                let [Opnd::Literal(bits)] = op.src[..] else { return None };
                if reg != want_reg.unwrap() || bits != want_literal.unwrap() {
                    return None;
                }
                ident_reg_found = true;
            }
            "s_and_b32" | "s_and_b64" if mask_sources.is_none() && op.dst == [mask] => {
                mask_sources = Some((op.mnemonic.as_str(), &op.src));
            }
            save @ ("s_or_saveexec_b32" | "s_or_saveexec_b64") => {
                let [saved @ Opnd::Sgpr(_)] = op.dst[..] else {
                    return None;
                };
                let matches = match mask_sources {
                    Some((and, sources)) => {
                        (save.ends_with("32") == and.ends_with("32")) && sources.contains(&saved)
                    }
                    None => saved == mask,
                };
                if matches && ident_reg_found {
                    return Some((index, mask, expected));
                }
                return None;
            }
            _ => return None,
        }
    }
    None
}

fn masked_identity(operation: &str) -> Option<NeutralIdentity> {
    match operation {
        "v_or_b32" | "v_max_u32" => Some(NeutralIdentity::Immediate(0)),
        "v_and_b32" | "v_min_u32" => Some(NeutralIdentity::Immediate(-1)),
        "v_add_nc_u32" => Some(NeutralIdentity::Immediate(0)),
        "v_add_f32" => Some(NeutralIdentity::Immediate(0)),
        "v_min_f32" => Some(NeutralIdentity::LiteralBits(f32::INFINITY.to_bits())),
        "v_max_f32" => Some(NeutralIdentity::LiteralBits(f32::NEG_INFINITY.to_bits())),
        _ => None,
    }
}

// Whether the floating-extrema shape is the masked form (saveexec + identity
// move + scalar AND + CNDMASK) rather than the nand form. The caller routes
// floating extrema here; the nand form keeps its own lowering.
pub(super) fn masked_floating(insns: &[Insn], first: usize) -> bool {
    let Some(insn) = insns.get(first) else { return false };
    let Some(id) = masked_identity(&insn.mnemonic) else {
        return false;
    };
    masked_prologue(insns, first, id).is_some()
}

pub(super) fn lower(insns: &mut [Insn], first: usize) -> Result<(u32, usize), &'static str> {
    let start = first.checked_sub(2).ok_or("missing reduction prologue")?;
    let tail = &insns[start..];
    let operation = &insns[first].mnemonic;
    let floating = matches!(operation.as_str(), "v_min_f32" | "v_max_f32");
    // The floating form inserts the neutral value between SAVEEXEC and CNDMASK.
    let masked = masked_identity(operation)
        .map(|id| masked_prologue(insns, first, id).ok_or("unrecognized masked DPP prologue"))
        .transpose()?;
    let (save, neutral, expected_mask): (&Insn, Opnd, Opnd) = if let Some((index, mask, neutral)) = masked {
        (&insns[index], neutral, mask)
    } else if floating {
        let prologue = start.checked_sub(1).ok_or("missing extrema prologue")?;
        let identity_bits = if operation == "v_min_f32" {
            f32::INFINITY.to_bits()
        } else {
            f32::NEG_INFINITY.to_bits()
        };
        if tail[0].mnemonic != "v_mov_b32" || tail[0].src != [Opnd::Literal(identity_bits)] {
            return Err("unexpected extrema identity");
        }
        let [reg @ Opnd::Vgpr(_)] = tail[0].dst[..] else {
            return Err("invalid identity register");
        };
        (&insns[prologue], reg, Opnd::Sgpr(0)) // mask patched after saved is bound
    } else {
        let int = match masked_identity(operation) {
            Some(NeutralIdentity::Immediate(v)) => v,
            _ => return Err("unexpected identity encoding"),
        };
        (&tail[0], Opnd::ConstInt(int), Opnd::Sgpr(0)) // mask patched after saved is bound
    };
    let width = match save.mnemonic.as_str() {
        "s_or_saveexec_b32" => 32,
        "s_or_saveexec_b64" => 64,
        _ => return Err("unrecognized DPP reduction prologue"),
    };
    let last_read = if width == 32 { 8 } else { 10 };
    let block = tail
        .get(..last_read + 2)
        .ok_or("truncated wave reduction")?;
    let [Opnd::Sgpr(saved)] = save.dst[..] else {
        return Err("invalid saved EXEC register");
    };
    let [Opnd::Vgpr(acc)] = block[1].dst[..] else {
        return Err("invalid accumulator");
    };
    let expected_mask = if masked.is_some() {
        expected_mask
    } else {
        Opnd::Sgpr(saved)
    };
    if save.src != [Opnd::ConstInt(-1)]
        || block[1].mnemonic != "v_cndmask_b32"
        || block[1].src.len() != 3
        || block[1].src[0] != neutral
        || !matches!(block[1].src[1], Opnd::Vgpr(_))
        || block[1].src[2] != expected_mask
    {
        return Err("invalid masked reduction input");
    }

    // The first three DPP8 shuffles accumulate pairs, quads and octets.
    // DPP16 row_mirror combines the two octets in each 16-lane row.
    let controls = [0xde54c1, 0x936012, 0x000924, 0xff0940];
    for (i, (&ctrl, ins)) in controls.iter().zip(&block[2..6]).enumerate() {
        let selector = if i == 3 { 250 } else { 233 };
        if &ins.mnemonic != operation
            || ins.dst != [Opnd::Vgpr(acc)]
            || ins.src != [Opnd::Raw(selector), Opnd::Vgpr(acc)]
            || ins.raw.get(1) != Some(&(ctrl << 8 | u32::from(acc)))
        {
            return Err("unrecognized DPP reduction shuffle");
        }
    }
    let perm = &block[6];
    let [tmp @ Opnd::Vgpr(_)] = perm.dst[..] else {
        return Err("invalid cross-row destination");
    };
    let vr = Opnd::Vgpr(acc);
    if perm.mnemonic != "v_permlanex16_b32"
        || perm.src != [vr, Opnd::ConstInt(0), Opnd::ConstInt(0)]
        || &block[7].mnemonic != operation
        || block[7].dst != [vr]
        || block[7].src != [tmp, vr]
    {
        return Err("unrecognized cross-row add");
    }
    let lane_read = |ins: &Insn, lane| match ins.dst[..] {
        [Opnd::Vgpr(d @ 0..=105)]
            if ins.mnemonic == "v_readlane_b32"
                && ins.src.first() == Some(&vr)
                && ins.src.get(1) == Some(&Opnd::ConstInt(lane))
                && ins.raw.as_slice()
                    == [
                        0xd760_0000 | u32::from(d),
                        ((128 + lane as u32) << 9) | 256 | u32::from(acc),
                    ] =>
        {
            Ok(d)
        }
        _ => Err("invalid integer sum lane read"),
    };
    if width == 64 {
        let high = lane_read(&block[8], 32)?;
        if &block[9].mnemonic != operation
            || block[9].dst != [vr]
            || block[9].src != [Opnd::Sgpr(high), vr]
        {
            return Err("missing upper-half add");
        }
    }
    lane_read(&block[last_read], 0)?;
    let restore = &block[last_read + 1];
    if restore.mnemonic
        != (if width == 32 {
            "s_mov_b32"
        } else {
            "s_mov_b64"
        })
        || restore.dst != [Opnd::ExecLo]
        || restore.src != [Opnd::Sgpr(saved)]
    {
        return Err("integer sum does not restore EXEC");
    }

    for (step, ins) in insns[first..first + 4].iter_mut().enumerate() {
        ins.op = DPP_REDUCE;
        ins.src = vec![vr];
        ins.aux.imm = step as i64;
    }
    // READLANE's encoded destination names an SGPR. Mark just the actual
    // broadcasts here so the independent scalar lane-spill pass ignores them.
    for i in (8..=last_read).step_by(2) {
        let ins = &mut insns[start + i];
        let Opnd::Vgpr(dst) = ins.dst[0] else {
            return Err("invalid lane result");
        };
        ins.op = READ_LANE;
        ins.dst[0] = Opnd::Sgpr(dst);
        ins.src.truncate(2);
    }
    Ok((width, start + last_read + 2))
}

// Restrict the proof to a small straight-line scalar slice. No lane reads,
// EXEC/VCC values, memory operations or unrecognized definitions are admitted.
// The caller checks all control-flow edges crossing this entire slice.
fn uniform_scalar_chain(insns: &[Insn], end: usize, operand: Opnd) -> Option<usize> {
    use std::collections::BTreeSet;
    let Opnd::Sgpr(reg @ 0..=105) = operand else { return None };
    let mut needed = BTreeSet::from([reg]);
    for index in (end.saturating_sub(32)..end).rev() {
        let ins = &insns[index];
        if matches!(ins.mnemonic.as_str(), "s_nop" | "s_waitcnt") { continue; }
        let [Opnd::Sgpr(dst @ 0..=105)] = ins.dst.as_slice() else { return None };
        let ordinary = match (ins.enc, ins.mnemonic.as_str(), ins.src.len()) {
            (Enc::Sop1, "s_mov_b32", 1) => true,
            (Enc::Sop2, "s_add_u32" | "s_add_i32" | "s_sub_u32" | "s_sub_i32"
                | "s_and_b32" | "s_or_b32" | "s_xor_b32"
                | "s_lshl_b32" | "s_lshr_b32", 2) => true,
            _ => false,
        };
        if !ordinary { return None; }
        if needed.remove(dst) {
            for source in &ins.src {
                match source {
                    Opnd::Sgpr(r @ 0..=105) => { needed.insert(*r); }
                    Opnd::ConstInt(_) | Opnd::Literal(_) => (),
                    _ => return None,
                }
            }
        }
        if needed.is_empty() { return Some(index); }
    }
    None
}

/// A forward SCC branch can skip a whole reduction without divergence when
/// its comparison uses only immediates or the immediately preceding verified
/// reduction's scalar result. Require the entire proof window to be free of
/// other control-flow edges: a lane skipping the comparison, entering its
/// body, or revisiting it on a loop could otherwise break uniformity.
fn uniform_forward_guard(
    insns: &[Insn],
    branch_index: usize,
    reduction_end: usize,
    verified_reads: &[usize],
) -> bool {
    let branch = &insns[branch_index];
    let pc = branch.offset as i64;
    let target = pc + 4 + 4 * branch.aux.imm;
    if !matches!(branch.op, 2 | 4 | 5) || target <= reduction_end as i64 {
        return false;
    }
    if branch.op == 2 {
        // Every invocation reaching an unconditional branch takes it. Prove
        // that there is no bypass of the branch and no entry into the skipped
        // interval, including a backward edge or an indirect branch.
        return insns.iter().enumerate().all(|(i, op)| {
            if i == branch_index { return true; }
            if matches!(op.mnemonic.as_str(), "s_setpc_b64" | "s_swappc_b64" | "s_call_b64" | "s_rfe_b64" | "s_trap") {
                return false;
            }
            if op.enc == Enc::Sopp && Emitter::branch_cond(op.op).is_some() {
                let from = op.offset as i64;
                let to = from + 4 + 4 * op.aux.imm;
                return !(from.min(to) < target && from.max(to) > pc);
            }
            !(op.enc == Enc::Sopp && op.op == 1 && (op.offset as i64) < target)
        });
    }
    let Some(compare_index) = branch_index.checked_sub(1) else { return false };
    let compare = &insns[compare_index];
    if compare.enc != Enc::Sopc
        || !matches!(compare.mnemonic.as_str(),
            "s_cmp_eq_u32" | "s_cmp_lg_u32" | "s_cmp_gt_u32" | "s_cmp_ge_u32"
            | "s_cmp_lt_u32" | "s_cmp_le_u32" | "s_cmp_eq_i32" | "s_cmp_lg_i32"
            | "s_cmp_gt_i32" | "s_cmp_ge_i32" | "s_cmp_lt_i32" | "s_cmp_le_i32")
        || compare.src.len() != 2
    {
        return false;
    }

    // A final READ_LANE of an already checked reduction broadcasts the same
    // scalar to every host invocation. No intervening writes are permitted.
    let previous_read = verified_reads.last().copied().filter(|&read| {
        read + 1 < compare_index
            && insns[read].op == READ_LANE
            && insns[read].enc == Enc::Vop3
            && insns[read + 1].mnemonic.starts_with("s_mov_b")
            && insns[read + 1].dst == [Opnd::ExecLo]
            && insns[read + 2..compare_index].iter().all(|i| {
                matches!(i.mnemonic.as_str(), "s_nop" | "s_waitcnt")
            })
    });
    let mut proof_start = compare.offset as i64;
    for src in &compare.src {
        match src {
            Opnd::ConstInt(_) | Opnd::Literal(_) => (),
            Opnd::Sgpr(_) => {
                if let Some(read) = previous_read.filter(|&read| insns[read].dst.as_slice() == [*src]) {
                    proof_start = proof_start.min(insns[read].offset as i64);
                } else {
                    // A contiguous straight-line chain of ordinary scalar moves
                    // and integer operations is uniform if all of its dependencies
                    // are literals or previously established uniform definitions.
                    let Some(first) = uniform_scalar_chain(insns, compare_index, *src) else { return false };
                    proof_start = proof_start.min(insns[first].offset as i64);
                }
            }
            _ => return false,
        }
    }

    for (index, op) in insns.iter().enumerate() {
        if index == branch_index {
            continue;
        }
        // Indirect edges do not have a statically bounded target.
        if matches!(op.mnemonic.as_str(),
            "s_setpc_b64" | "s_swappc_b64" | "s_call_b64" | "s_rfe_b64" | "s_trap")
        {
            return false;
        }
        if op.enc == Enc::Sopp && Emitter::branch_cond(op.op).is_some() {
            let from = op.offset as i64;
            let to = from + 4 + 4 * op.aux.imm;
            if from.min(to) < target && from.max(to) > proof_start {
                return false;
            }
        } else if op.enc == Enc::Sopp && op.op == 1 && (op.offset as i64) < target {
            return false;
        }
    }
    true
}

/// Whole-wave forward skip: a proven uniform skip, or an EXEC-empty skip of a
/// self-contained masked full-mask region. Copied from TuxBox d59da4d (Pavel
/// Kirilin), with credit.
fn execz_forward_guard(
    insns: &[Insn],
    branch_index: usize,
    region_start: usize,
    reduction_end: usize,
    first: usize,
    operation: &str,
    verified_reads: &[usize],
) -> bool {
    let branch = &insns[branch_index];
    if branch.op == 8 {
        let masked = masked_identity(operation)
            .map(|id| masked_prologue(insns, first, id).is_some())
            .unwrap_or(false);
        if !masked {
            return false;
        }
        let pc = branch.offset as i64;
        let target = pc + 4 + 4 * branch.aux.imm;
        if target <= reduction_end as i64 {
            return false;
        }
        // Branches between the guard and the region whose edges skip or
        // enter the region (or lands exactly at the region start) are
        // non-uniform and would make the whole-wave reduction execute
        // partially. Anything else in the window is a local conditional that
        // cannot skip the region; branches after the region are independent
        // of its execution (the guard skips it wholesale). Indirect edges
        // are never statically bounded.
        for (index, op) in insns.iter().enumerate() {
            if index == branch_index {
                continue;
            }
            if matches!(op.mnemonic.as_str(),
                "s_setpc_b64" | "s_swappc_b64" | "s_call_b64" | "s_rfe_b64" | "s_trap")
            {
                return false;
            }
            if op.enc == Enc::Sopp && (Emitter::branch_cond(op.op).is_some() || op.op == 1) {
                let from = op.offset as i64;
                let target_edge = from + 4 + 4 * op.aux.imm;
                // TuxBox 340366d (credit): every invocation must reach the guard's
                // wave vote. A forward rejoin at the guard is safe; bypasses and
                // enclosing loops are not. TB_REDUCTION_EXECZ_VOTE=0 off.
                if reduction_execz_vote() && ((from < pc && target_edge > pc)
                    || (target_edge <= pc && pc <= from))
                {
                    return false;
                }
                if from < pc || from >= region_start as i64 {
                    continue;
                }
                if target_edge >= region_start as i64 {
                    return false;
                }
            }
        }
        return true;
    }
    uniform_forward_guard(insns, branch_index, reduction_end, verified_reads)
}

/// The branch emitter predicates regions per host invocation. HLSL lane reads
/// require the source invocation to participate, including lane zero when guest
/// EXEC has masked it. A branch around or into the region can violate that, so such
/// regions compute each half's reduction with wave intrinsics instead: lanes that
/// do not participate on the host are inactive in the guest and add the identity.
/// Instruction ranges from the save-EXEC prologue through the EXEC restore
/// (TuxBox be2a198, for wave_loops; credit to its author).
pub(super) fn reduction_regions(insns: &[Insn], width: u32)
    -> impl Iterator<Item = (usize, std::ops::RangeInclusive<usize>)> + '_
{
    insns.iter().enumerate().filter_map(move |(first, ins)| {
        if ins.op != DPP_REDUCE || ins.aux.imm != 0 {
            return None;
        }
        let prologue_len = if matches!(ins.mnemonic.as_str(), "v_min_f32" | "v_max_f32") { 3 } else { 2 };
        let start = masked_identity(&ins.mnemonic)
            .and_then(|neutral| masked_prologue(insns, first, neutral))
            .map_or_else(|| first - prologue_len, |(index, _, _)| index);
        let restore = first + if width == 32 { 7 } else { 9 };
        Some((first, start..=restore))
    })
}

/// TuxBox 340366d (credit): a proven EXECZ guard of a full-mask reduction votes
/// wave-wide (`!WaveActiveAnyTrue`) so every invocation arrives at the
/// reduction. TB_REDUCTION_EXECZ_VOTE=0 keeps the per-lane test.
pub(super) fn reduction_execz_vote() -> bool {
    std::env::var("TB_REDUCTION_EXECZ_VOTE").map_or(true, |v| v.trim() != "0")
}

/// Validate fixed-width reductions and return EXECZ instruction indices whose
/// masked region and whole-wave arrival permit a wave-wide empty-EXEC vote.
pub(super) fn check_control_flow(insns: &mut [Insn], width: u32) -> Result<std::collections::BTreeSet<usize>, String> {
    let mut execz_guards = std::collections::BTreeSet::new();
    let mut verified_reads = Vec::new();
    for first in 0..insns.len() {
        let ins = &insns[first];
        if ins.op != DPP_REDUCE || ins.aux.imm != 0 {
            continue;
        }
        let prologue_len = if matches!(ins.mnemonic.as_str(), "v_min_f32" | "v_max_f32") {
            3
        } else {
            2
        };
        let start = if let Some(neutral) = masked_identity(&ins.mnemonic) {
            masked_prologue(insns, first, neutral)
                .map(|(index, _, _)| index)
                .unwrap_or(first - prologue_len)
        } else {
            first - prologue_len
        };
        let start = insns[start].offset as i64;
        let last_read = first + if width == 32 { 6 } else { 8 };
        // The readlane is the last collective; the following restore is
        // part of the protected region too.
        let restore = first + if width == 32 { 7 } else { 9 };
        let end = insns[restore].offset as i64;
        let mut divergent = false;
        for (branch_index, branch) in insns.iter().enumerate().filter(|(_, i)| i.enc == Enc::Sopp) {
            let pc = branch.offset as i64;
            if branch.op == 1 && pc < start {
                divergent = true;
            }
            if Emitter::branch_cond(branch.op).is_some() {
                let target = pc + 4 + 4 * branch.aux.imm;
                if (pc < start && target >= start) || (pc > end && target <= end)
                    || (pc >= start && pc <= end)
                {
                    if !(pc < start && execz_forward_guard(insns, branch_index, start as usize, end as usize, first, &ins.mnemonic, &verified_reads)) {
                        divergent = true;
                    } else if branch.op == 8 && reduction_execz_vote() {
                        execz_guards.insert(branch_index);
                    }
                }
            }
        }
        if !divergent {
            verified_reads.push(last_read);
            continue;
        }
        // Steps 1-3, the cross-row permute and its combine fold into the first step.
        for i in first + 1..first + 6 {
            let x = &mut insns[i];
            x.enc = Enc::Vop1;
            x.op = 0;
            x.mnemonic = "v_nop".into();
            x.src.clear();
            x.aux = Default::default();
        }
        insns[first].op = HALF_REDUCE;
        insns[first].aux.imm2 = first as i64;
        let reads: &[(usize, i64)] = if width == 32 { &[(first + 6, 0)] } else { &[(first + 6, 32), (first + 8, 0)] };
        for &(i, lane) in reads {
            if insns[i].op != READ_LANE {
                return Err(format!("divergent wave reduction at {start:#x} lacks its lane reads"));
            }
            insns[i].op = HALF_READ;
            insns[i].aux.imm = lane;
            insns[i].aux.imm2 = first as i64;
        }
    }
    Ok(execz_guards)
}

// Only a complete, single guest wave64 may use group rendezvous. Every
// branch must be proven group-uniform and every collective must be one of the
// exact recognized integer reductions. All other programs keep the native path.
pub(super) fn portable_single_wave(insns: &[Insn]) -> bool {
    let mut covered = vec![false; insns.len()];
    let mut masks = Vec::new();
    let mut reads = Vec::new();
    for (first, i) in insns.iter().enumerate() {
        if i.op != DPP_REDUCE || i.aux.imm != 0 {
            continue;
        }
        let Some(start) = first.checked_sub(2) else { return false };
        let Some(block) = insns.get(start..start + 12) else { return false };
        // The same group-shared exchange works for the already verified
        // extrema pattern, but only with the direct saved-EXEC predicate.
        // A floating identity move belongs to the protected region too.
        let operation = i.mnemonic.as_str();
        if !matches!(operation, "v_add_nc_u32" | "v_min_u32" | "v_max_u32" |
            "v_min_f32" | "v_max_f32") { return false; }
        let floating = matches!(operation, "v_min_f32" | "v_max_f32");
        let Some(covered_start) = start.checked_sub(usize::from(floating)) else { return false };
        let save = &insns[covered_start];
        let [Opnd::Sgpr(saved)] = save.dst[..] else { return false };
        if floating {
            let identity = if operation == "v_min_f32" { 0x7f800000 } else { 0xff800000 };
            if block[0].mnemonic != "v_mov_b32" || block[0].src != [Opnd::Literal(identity)]
                || block[0].dst.len() != 1 || block[1].src.first() != block[0].dst.first()
            { return false; }
        }
        if saved >= 105 || saved % 2 != 0 || save.mnemonic != "s_or_saveexec_b64"
            || save.src != [Opnd::ConstInt(-1)]
            || block[1].mnemonic != "v_cndmask_b32"
            || block[1].src.get(2) != Some(&Opnd::Sgpr(saved))
            || !block[2..6].iter().enumerate().all(|(step, i)| {
                i.op == DPP_REDUCE && i.aux.imm == step as i64 && i.mnemonic == operation
            })
            || block[6].mnemonic != "v_permlanex16_b32"
            || block[6].raw.first().is_none_or(|w| w & 0x7800 != 0)
            || block[7].mnemonic != operation
            || block[9].mnemonic != operation
            || block[11].mnemonic != "s_mov_b64"
            || block[11].dst != [Opnd::ExecLo]
            || block[11].src != [Opnd::Sgpr(saved)]
            || covered[covered_start..start + 12].iter().any(|&v| v)
        {
            return false;
        }
        for (idx, lane) in [(8, 32), (10, 0)] {
            let read = &block[idx];
            if read.op != READ_LANE || read.enc != Enc::Vop3
                || read.src.get(1) != Some(&Opnd::ConstInt(lane))
                || !matches!(read.dst[..], [Opnd::Sgpr(n)] if n != saved && n != saved + 1)
            {
                return false;
            }
        }
        masks.extend([saved, saved + 1]);
        reads.push(start + 10);
        covered[covered_start..start + 12].fill(true);
    }
    if reads.is_empty() {
        return false;
    }
    for (idx, i) in insns.iter().enumerate() {
        if covered[idx] {
            continue;
        }
        // The generic emitter represents an EXEC bit per invocation, not a
        // full scalar mask. Do not let a saved mask escape the reduction.
        if masks.iter().any(|&n| super::reads_sgpr(i, n)) {
            return false;
        }
        if i.enc == Enc::Sopp {
            if Emitter::branch_cond(i.op).is_some() {
                let prior: Vec<_> = reads.iter().copied().filter(|&r| r < idx).collect();
                if !uniform_forward_guard(insns, idx, i.offset, &prior) {
                    return false;
                }
                continue;
            }
            if matches!(i.mnemonic.as_str(), "s_nop" | "s_waitcnt")
                || (i.mnemonic == "s_endpgm" && idx + 1 == insns.len())
            {
                continue;
            }
            return false;
        }
        // No other wave/quad instructions, EXEC transitions, or LDS accesses.
        // Keep a deliberately small whitelist of ordinary ALU and buffer ops.
        let lane_local = matches!(i.mnemonic.as_str(),
            "v_mov_b32" | "v_add_nc_u32" | "v_sub_nc_u32" | "v_subrev_nc_u32"
            | "v_and_b32" | "v_or_b32" | "v_xor_b32" | "v_lshlrev_b32"
            | "v_lshrrev_b32" | "v_mul_lo_u32" | "v_add_f32" | "v_mul_f32"
            | "s_mov_b32" | "s_movk_i32" | "s_add_u32" | "s_sub_u32"
            | "s_cmp_eq_u32" | "s_cmp_lg_u32" | "s_cmp_gt_u32" | "s_cmp_ge_u32"
            | "s_cmp_lt_u32" | "s_cmp_le_u32" | "s_cmp_eq_i32" | "s_cmp_lg_i32"
            | "s_cmp_gt_i32" | "s_cmp_ge_i32" | "s_cmp_lt_i32" | "s_cmp_le_i32")
            && i.src.iter().chain(&i.dst).all(|o| matches!(o, Opnd::Vgpr(_)
                | Opnd::Sgpr(_) | Opnd::ConstInt(_) | Opnd::ConstF32(_) | Opnd::Literal(_) | Opnd::Scc));
        let buffer = matches!(i.enc, Enc::Mubuf | Enc::Smem)
            && (i.mnemonic.starts_with("buffer_load_dword")
                || i.mnemonic.starts_with("buffer_store_dword")
                || i.mnemonic.starts_with("s_load_dword")
                || i.mnemonic.starts_with("s_buffer_load_dword"))
            && i.src.iter().chain(&i.dst).all(|o| matches!(o, Opnd::Vgpr(_)
                | Opnd::Sgpr(_) | Opnd::ConstInt(_) | Opnd::Literal(_)));
        if !lane_local && !buffer {
            return false;
        }
    }
    true
}

// A second barrier prevents a fast host subgroup from overwriting a slot
// before the other subgroup has consumed it. The verifier above ensures
// that every invocation reaches these barriers in the same order.
pub(super) const PORTABLE_DECLARATIONS: &str = r#"
groupshared uint tb_reduction_lanes[64];
uint tb_reduction_read(uint lane, uint value, uint source) {
    tb_reduction_lanes[lane] = value;
    GroupMemoryBarrierWithGroupSync();
    uint result = tb_reduction_lanes[source];
    GroupMemoryBarrierWithGroupSync();
    return result;
}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ShaderExtract, Stage};
    mod fixture {
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/support/wave_guard_fixture.rs"));
    }

    fn insn(mnemonic: &str, enc: Enc, op: u32, dst: Vec<Opnd>, src: Vec<Opnd>) -> Insn {
        Insn {
            offset: 0,
            dwords: 1,
            enc,
            op,
            raw: vec![0],
            mnemonic: mnemonic.into(),
            dst,
            src,
            aux: Default::default(),
        }
    }

    fn nop() -> Insn {
        insn("s_nop", Enc::Sopp, 0, vec![], vec![])
    }

    fn branch(op: u32) -> Insn {
        insn("branch", Enc::Sopp, op, vec![], vec![])
    }

    fn compare(src: Vec<Opnd>) -> Insn {
        insn("s_cmp_eq_u32", Enc::Sopc, 6, vec![], src)
    }

    // Synthetic input to the control-flow pass, after the exact DPP
    // sequence has been recognized by lower(). Include both wave widths.
    fn reduction(width: u32, result: u8) -> Vec<Insn> {
        let mut ops = vec![nop(); if width == 32 { 10 } else { 12 }];
        ops[0] = insn("s_or_saveexec_b64", Enc::Sop1, 0,
                     vec![Opnd::Sgpr(0)], vec![Opnd::ConstInt(-1)]);
        ops[2] = insn("v_add_nc_u32", Enc::Vop2, DPP_REDUCE,
                     vec![Opnd::Vgpr(1)], vec![Opnd::Vgpr(1)]);
        let read = ops.len() - 2;
        ops[read] = insn("v_readlane_b32", Enc::Vop3, READ_LANE,
                         vec![Opnd::Sgpr(result)], vec![Opnd::Vgpr(1), Opnd::ConstInt(0)]);
        ops[read + 1] = insn("s_mov_b64", Enc::Sop1, 0,
                             vec![Opnd::ExecLo], vec![Opnd::Sgpr(0)]);
        ops
    }

    fn offsets(ops: &mut [Insn]) {
        for (i, op) in ops.iter_mut().enumerate() {
            op.offset = i * 4;
        }
    }

    fn edge(ops: &mut [Insn], from: usize, to: usize, op: u32) {
        ops[from] = branch(op);
        ops[from].aux.imm = to as i64 - from as i64 - 1;
        offsets(ops);
    }

    #[test]
    fn straight_dpp8_exchange_is_not_a_lane_local_copy() {
        let (mut ex, _, _) = fixture::shader(32);
        let words = [0x2800_00e9u32, 0xde54_c100, 0xd760_0007, 0x0001_0100, 0xbf81_0000];
        ex.isa = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        ex.isa_data = ex.isa.clone();
        let translated = super::super::translate(&ex).unwrap();
        assert!(translated.hlsl.contains("tb_reduction_read(gtid.x, v0, (gtid.x & ~7u)"));
        assert!(translated.hlsl.contains("tb_reduction_read(gtid.x, v0, 0x0u)"));
        assert_eq!(translated.required_wave_size, None);
        let ops = crate::rdna2::disassemble(&ex.isa);
        for group in [[16,1,1],[64,1,1],[32,2,1]] {
            assert!(!straight_exchange(&mut ops.clone(), group));
        }
        let mut branch = ops.clone();
        branch.insert(0, self::branch(2));
        assert!(!straight_exchange(&mut branch,[32,1,1]));
        let mut mask = ops.clone();
        mask.insert(0, insn("s_mov_b32", Enc::Sop1, 3,
            vec![Opnd::ExecLo],vec![Opnd::ConstInt(0)]));
        assert!(!straight_exchange(&mut mask,[32,1,1]));
        let mut dynamic = ops.clone();
        dynamic[1].src[1] = Opnd::Sgpr(8);
        assert!(!straight_exchange(&mut dynamic,[32,1,1]));
        let mut after_store = ops.clone();
        after_store.insert(1, insn("buffer_store_dword", Enc::Mubuf, 0,
            vec![Opnd::Vgpr(4)],vec![Opnd::Vgpr(5),Opnd::Sgpr(16)]));
        assert!(!straight_exchange(&mut after_store,[32,1,1]));
        let mut exchange = ops.clone();
        exchange.remove(1);
        assert!(straight_exchange(&mut exchange,[64,1,1]));
        assert_eq!(exchange[0].op,DPP8_EXCHANGE);
    }

    #[test]
    fn translates_two_native_reductions_with_a_uniform_scc_guard() {
        for width in [32, 64] {
            // GFX1030 instructions assembled with llvm-mc, with the four DPP
            // control words from the exact reduction recognized by lower().
            let (mut ex, mut words, cmp) = fixture::shader(width);
            let translated = super::super::translate(&ex).unwrap();
            assert_eq!(translated.required_wave_size, (width == 32).then_some(32));
            if width == 64 {
                assert!(translated.hlsl.contains("tb_reduction_read("));
                assert!(!translated.hlsl.contains("WaveReadLaneAt("));
                assert!(!translated.hlsl.contains("[WaveSize("));
            } else {
                assert!(translated.hlsl.contains("WaveReadLaneAt("));
            }
            // Replace the proven scalar with an unproven entry register.
            words[cmp] = 0xbf06_8008;
            ex.isa = words.iter().flat_map(|word| word.to_le_bytes()).collect();
            ex.isa_data = ex.isa.clone();
            // e736a39: an unproven guard now translates via per-half wave intrinsics.
            let halved = super::super::translate(&ex).unwrap();
            assert!(halved.hlsl.contains("WaveGetLaneIndex() < 32u ?"), "{}", halved.hlsl);
        }
    }

    #[test]
    fn native_literal_scalar_guard_translates_and_unknown_source_refuses() {
        for width in [32, 64] {
            let (mut ex, mut words, cmp) = fixture::shader(width);
            // s_mov_b32 s7, 1; the following comparison and branch are unchanged.
            words.insert(cmp, 0xbe87_0381);
            ex.isa = words.iter().flat_map(|word| word.to_le_bytes()).collect();
            ex.isa_data = ex.isa.clone();
            let translated = super::super::translate(&ex).unwrap();
            assert!(translated.hlsl.contains("s7 = 1u;") || translated.hlsl.contains("s7 = 0x1u;") || translated.hlsl.contains("s7 = 0x00000001u;"), "{}", translated.hlsl);
            // s_mov_b32 s7, s8 (unknown input) must still be rejected.
            words[cmp] = 0xbe87_0308;
            ex.isa = words.iter().flat_map(|word| word.to_le_bytes()).collect();
            ex.isa_data = ex.isa.clone();
            // e736a39: an unknown source now translates via per-half wave intrinsics.
            let halved = super::super::translate(&ex).unwrap();
            assert!(halved.hlsl.contains("WaveGetLaneIndex() < 32u ?"), "{}", halved.hlsl);
        }
    }

    #[test]
    fn portable_extrema_requires_direct_exec_and_no_mask_escape() {
        for operation in ["umin", "umax", "fmin", "fmax"] {
            let ex = fixture::extrema(64, operation);
            let translated = super::super::translate(&ex).unwrap();
            assert_eq!(translated.required_wave_size, None);
            assert!(translated.hlsl.contains("tb_reduction_read("));
            assert!(!translated.hlsl.contains("WaveReadLaneAt("));
            let mut ops = crate::rdna2::disassemble(&ex.isa);
            assert_eq!(super::super::lower_wave_reductions(&mut ops).unwrap(), Some(64));
            assert!(portable_single_wave(&ops));
            let select = ops.iter().position(|i| i.mnemonic == "v_cndmask_b32").unwrap();
            let mut altered = ops.clone();
            altered[select].src[2] = Opnd::Sgpr(8);
            assert!(!portable_single_wave(&altered));
            if operation.starts_with('f') {
                let mut altered = ops.clone();
                altered[select-1].src[0] = Opnd::Literal(0);
                assert!(!portable_single_wave(&altered));
            }
            let mut altered = ops.clone();
            altered.insert(altered.len()-1, insn("s_mov_b32", Enc::Sop1, 0,
                vec![Opnd::Sgpr(8)], vec![Opnd::Sgpr(0)]));
            assert!(!portable_single_wave(&altered));
            let native = fixture::extrema(32, operation);
            assert_eq!(super::super::translate(&native).unwrap().required_wave_size, Some(32));
        }
    }

    #[test]
    fn portable_wave64_requires_a_single_group_and_no_other_wave_operations() {
        let (mut ex, _, _) = fixture::shader(64);
        let mut insns = crate::rdna2::disassemble(&ex.isa);
        assert_eq!(super::super::lower_wave_reductions(&mut insns).unwrap(), Some(64));
        assert!(portable_single_wave(&insns));
        let compare = insns.iter().position(|i| i.mnemonic == "s_cmp_eq_u32").unwrap();
        let outside = compare + 2;
        let mut changed = insns.clone();
        changed[outside].mnemonic = "v_readfirstlane_b32".into();
        assert!(!portable_single_wave(&changed));
        changed = insns.clone();
        changed[outside].src[0] = Opnd::Sgpr(0); // saved EXEC must not escape
        assert!(!portable_single_wave(&changed));
        changed = insns.clone();
        changed[compare].src[0] = Opnd::Sgpr(8);
        assert!(!portable_single_wave(&changed));
        changed = insns.clone();
        changed[8].dst[0] = Opnd::Sgpr(0); // overwrites saved EXEC
        assert!(!portable_single_wave(&changed));

        ex.isa_preamble = [0, 128, 1, 1];
        let native = super::super::translate(&ex).unwrap();
        assert_eq!(native.required_wave_size, Some(64));
        assert!(native.hlsl.contains("[WaveSize(64)]"));
        assert!(!native.hlsl.contains("tb_reduction_read("));
    }

    /// True when check_control_flow treats a reduction as divergent and
    /// rewrites it to per-half wave intrinsics (e736a39; formerly a refusal).
    fn halved(ops: &[Insn], width: u32) -> bool {
        let mut ops = ops.to_vec();
        // The synthetic reduction() has no READ_LANE ops, so the rewrite stops at
        // "lacks its lane reads"; that error is only reachable on the divergent path.
        match check_control_flow(&mut ops, width) {
            Ok(_) => ops.iter().any(|i| i.op == HALF_REDUCE),
            Err(e) => { assert!(e.contains("divergent wave reduction"), "{e}"); true }
        }
    }

    #[test]
    fn uniform_literal_guard_must_skip_the_entire_reduction() {
        for width in [32, 64] {
            let mut ops = vec![compare(vec![Opnd::ConstInt(1), Opnd::Literal(0)]), branch(4)];
            ops.extend(reduction(width, 7));
            let after = ops.len();
            ops.push(nop());
            edge(&mut ops, 1, after, 4);
            assert!(!halved(&ops, width));
            // The compare may instead use a lane-dependent scalar.
            ops[0].src[0] = Opnd::Sgpr(9);
            assert!(halved(&ops, width)); // e736a39: divergent regions use per-half wave intrinsics
            ops[0].src[0] = Opnd::ConstInt(1);
            edge(&mut ops, 1, after - 1, 4);
            assert!(halved(&ops, width)); // e736a39: divergent regions use per-half wave intrinsics
            edge(&mut ops, 1, after, 8); // EXEC is a per-invocation mask
            assert!(halved(&ops, width)); // e736a39: divergent regions use per-half wave intrinsics
        }
    }

    #[test]
    fn literal_scalar_definition_proves_forward_guard_only_without_bypass() {
        for width in [32, 64] {
            let mut ops = vec![nop(), insn("s_mov_b32", Enc::Sop1, 0,
                vec![Opnd::Sgpr(9)], vec![Opnd::ConstInt(1)]),
                compare(vec![Opnd::Sgpr(9), Opnd::ConstInt(0)]), branch(4)];
            ops.extend(reduction(width, 7));
            let after = ops.len();
            ops.push(nop());
            edge(&mut ops, 3, after, 4);
            assert!(!halved(&ops, width));
            edge(&mut ops, 0, 2, 8);
            assert!(halved(&ops, width)); // e736a39: divergent regions use per-half wave intrinsics
            ops[0] = nop();
            offsets(&mut ops);
            ops[1].src[0] = Opnd::Sgpr(10);
            assert!(halved(&ops, width)); // e736a39: divergent regions use per-half wave intrinsics
        }
    }

    #[test]
    fn scalar_chain_guards_require_all_dependencies_and_no_bypass() {
        for width in [32, 64] {
            let mut ops = vec![nop(),
                insn("s_mov_b32", Enc::Sop1, 0, vec![Opnd::Sgpr(9)], vec![Opnd::ConstInt(2)]),
                insn("s_add_u32", Enc::Sop2, 0, vec![Opnd::Sgpr(10)], vec![Opnd::Sgpr(9), Opnd::ConstInt(1)]),
                insn("s_mov_b32", Enc::Sop1, 0, vec![Opnd::Sgpr(11)], vec![Opnd::Sgpr(10)]),
                compare(vec![Opnd::Sgpr(11), Opnd::Sgpr(9)]), branch(4)];
            ops.extend(reduction(width, 7));
            let after = ops.len();
            ops.push(nop());
            edge(&mut ops, 5, after, 4);
            assert!(!halved(&ops, width));
            edge(&mut ops, 0, 2, 8);
            assert!(halved(&ops, width)); // e736a39: divergent regions use per-half wave intrinsics
            ops[0] = nop();
            offsets(&mut ops);
            ops[2].src[1] = Opnd::Sgpr(12);
            assert!(halved(&ops, width)); // e736a39: divergent regions use per-half wave intrinsics
        }
    }

    #[test]
    fn unconditional_bypass_must_skip_whole_collective_and_have_no_other_entry() {
        for width in [32, 64] {
            let mut ops = vec![nop(), branch(2)];
            ops.extend(reduction(width, 7));
            let after = ops.len();
            ops.push(nop());
            edge(&mut ops, 1, after, 2);
            assert!(!halved(&ops, width));
            edge(&mut ops, 0, 3, 8);
            assert!(halved(&ops, width)); // e736a39: divergent regions use per-half wave intrinsics
            ops[0] = nop();
            offsets(&mut ops);
            edge(&mut ops, 1, after - 1, 2);
            assert!(halved(&ops, width)); // e736a39: divergent regions use per-half wave intrinsics
        }
    }

    #[test]
    fn prior_reduction_result_is_uniform_only_without_intervening_writes() {
        for width in [32, 64] {
            let mut ops = reduction(width, 7);
            let spacer = ops.len();
            ops.push(nop());
            ops.push(compare(vec![Opnd::Sgpr(7), Opnd::ConstInt(0)]));
            let guard = ops.len();
            ops.push(branch(5));
            ops.extend(reduction(width, 8));
            let after = ops.len();
            ops.push(nop());
            edge(&mut ops, guard, after, 5);
            assert!(!halved(&ops, width));
            ops[spacer] = insn("s_mov_b32", Enc::Sop1, 0,
                               vec![Opnd::Sgpr(7)], vec![Opnd::Sgpr(9)]);
            offsets(&mut ops);
            assert!(halved(&ops, width)); // e736a39: divergent regions use per-half wave intrinsics
            ops[spacer] = nop();
            offsets(&mut ops);
            ops[guard - 1].src[0] = Opnd::Sgpr(8);
            assert!(halved(&ops, width)); // e736a39: divergent regions use per-half wave intrinsics
        }
    }

    #[test]
    fn alternate_entry_and_loop_edges_invalidate_uniform_guard() {
        let mut ops = vec![nop(), compare(vec![Opnd::ConstInt(0), Opnd::ConstInt(0)]), branch(5)];
        ops.extend(reduction(64, 7));
        let after = ops.len();
        ops.push(nop());
        ops.push(nop());
        edge(&mut ops, 2, after, 5);
        assert!(!halved(&ops, 64));
        // Another branch can enter after the comparison.
        edge(&mut ops, 0, 2, 8);
        assert!(halved(&ops, 64)); // e736a39: divergent regions use per-half wave intrinsics
        // A branch landing at the comparison, however, reconverges before SCC.
        edge(&mut ops, 0, 1, 8);
        assert!(!halved(&ops, 64));
        edge(&mut ops, after + 1, 1, 8);
        assert!(halved(&ops, 64)); // e736a39: divergent regions use per-half wave intrinsics
        ops[after + 1] = nop();
        ops[0] = insn("s_endpgm", Enc::Sopp, 1, vec![], vec![]);
        offsets(&mut ops);
        assert!(halved(&ops, 64)); // e736a39: divergent regions use per-half wave intrinsics
    }
}
