//! Fixed-width DPP sums, extrema and prefix scans. Keep intermediate
//! register writes and the restored EXEC observable after each reduction.
use super::{Emitter, Enc, Insn, Opnd};
use std::collections::BTreeSet;

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


// Reductions may have a completed vector load and a second predicate.
// The scalar AND intersects that predicate with the saved EXEC before the
// CNDMASK sets excluded lanes to the reduction's identity.
enum NeutralIdentity {
    // Immediate neutral: v_or_b32 / v_max_u32 (0), v_and_b32 / v_min_u32 (-1),
    // v_add_nc_u32 (0) and v_add_f32 (0.0 == 0x00000000). Accept ConstInt or
    // Literal encodings of the same bits.
    Immediate(i64),
    // Exact literal bits, used directly or materialized by a preceding v_mov_b32.
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
        (NeutralIdentity::LiteralBits(bits), Opnd::Literal(k)) if k == bits => {
            (ident, None, None)
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
        "v_or_b32" | "v_xor_b32" | "v_max_u32" => Some(NeutralIdentity::Immediate(0)),
        "v_and_b32" | "v_min_u32" => Some(NeutralIdentity::Immediate(-1)),
        "v_add_nc_u32" => Some(NeutralIdentity::Immediate(0)),
        "v_add_f32" => Some(NeutralIdentity::Immediate(0)),
        "v_min_i32" => Some(NeutralIdentity::LiteralBits(i32::MAX as u32)),
        "v_max_i32" => Some(NeutralIdentity::LiteralBits(i32::MIN as u32)),
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
    for (step, ins) in block[2..6].iter().enumerate() {
        let [Opnd::Raw(selector), rhs] = ins.src[..] else {
            return Err("unrecognized DPP reduction shuffle");
        };
        let Some(&word) = ins.raw.get(1) else {
            return Err("missing DPP reduction control");
        };
        // SAVEEXEC enabled every lane, so FI has no effect.
        let valid = if step == 3 {
            // ROW_MIRROR also stays in bounds. Preserve the row/bank masks and modifiers.
            selector == 250 && (word & !((1 << 18) | (1 << 19))) >> 8 == 0xff0140
        } else {
            // Each lane holds a partial for its half. The selected lane must hold
            // the complementary half of the same pair, quad or octet.
            let selectors = word >> 8;
            matches!(selector, 233 | 234) && (0..8).all(|lane| {
                let partner = (selectors >> (3 * lane)) & 7;
                (partner >> step) == ((lane >> step) ^ 1)
            })
        };
        if !valid || word as u8 != acc || rhs != Opnd::Vgpr(acc)
            || &ins.mnemonic != operation || ins.dst != [Opnd::Vgpr(acc)]
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
        // Every invocation reaching an unconditional branch takes it, so the
        // skipped interval cannot cause divergence: any branch inside it is
        // checked by its own guard. Only unbounded (indirect) edges and early
        // exits are unsafe here.
        return insns.iter().enumerate().all(|(i, op)| {
            if i == branch_index { return true; }
            if matches!(op.mnemonic.as_str(), "s_setpc_b64" | "s_swappc_b64" | "s_call_b64" | "s_rfe_b64" | "s_trap") {
                return false;
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

/// The branch emitter predicates regions per host invocation. HLSL lane reads
/// require the source invocation to participate, including lane zero when guest
/// EXEC has masked it. A branch around or into the region can violate that.
/// Whole-wave forward skip. Either a proven scalar/unconditional skip, or
/// an EXEC-empty skip of a masked full-mask region. The EXEC-empty case is
/// only sound when the region's prologue actually verified itself as a
/// masked (self-contained) full-mask reduction; otherwise the region would
/// run with a partial mask and stale values.
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
            if std::env::var("TUXBOX_WAVE_DEBUG").is_ok() {
                eprintln!("WAVEDEBUG execz_guard: not masked (region_start={region_start:#x}, reduction_end={reduction_end:#x}, op={operation})");
            }
            return false;
        }
        let pc = branch.offset as i64;
        let target = pc + 4 + 4 * branch.aux.imm;
        if target <= reduction_end as i64 {
            if std::env::var("TUXBOX_WAVE_DEBUG").is_ok() {
                eprintln!("WAVEDEBUG execz_guard: target {target:#x} <= reduction_end {reduction_end:#x}");
            }
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
                // An earlier EXECZ or SCC branch past the whole region is a
                // decision of the whole wave: `check_control_flow` checks it
                // as a skip of this region in its own right, and votes it if
                // it is an EXECZ. So the wave still arrives here together.
                if from < pc && target_edge > reduction_end as i64 && matches!(op.op, 4 | 5 | 8) {
                    continue;
                }
                // Every invocation must reach the guard's wave vote. A forward
                // rejoin at the guard is safe; bypasses and enclosing loops are not.
                if (from < pc && target_edge > pc)
                    || (target_edge <= pc && pc <= from)
                {
                    return false;
                }
                if from < pc || from >= region_start as i64 {
                    continue;
                }
                if target_edge > reduction_end as i64 {
                    // A uniform branch (EXEC, unconditional, or SCC) that skips past
                    // the region is a uniform skip: it only fires when every lane
                    // leaves together, so it cannot make the reduction run
                    // partially. Only a divergent (VCC) skip is non-uniform.
                    if matches!(op.op, 6 | 7) {
                        if std::env::var("TUXBOX_WAVE_DEBUG").is_ok() {
                            eprintln!("WAVEDEBUG execz_guard: divergent skip idx={} pc={from:#x} target={target_edge:#x} op={} region_start={region_start:#x} region_end={reduction_end:#x}", index, op.op);
                        }
                        return false;
                    }
                    continue;
                }
                if target_edge >= region_start as i64 {
                    // A uniform branch landing exactly on the region start is a
                    // merge (all active lanes arrive together); it is safe.
                    // Anything that lands inside the region, or any divergent
                    // branch, is not.
                    if target_edge == region_start as i64 && matches!(op.op, 2 | 4 | 5) {
                        continue;
                    }
                    if std::env::var("TUXBOX_WAVE_DEBUG").is_ok() {
                        eprintln!("WAVEDEBUG execz_guard: offending branch idx={} pc={from:#x} target={target_edge:#x} op={} region_start={region_start:#x} region_end={reduction_end:#x}", index, op.op);
                    }
                    return false;
                }
            }
        }
        return true;
    }
    uniform_forward_guard(insns, branch_index, reduction_end, verified_reads)
}

/// Instruction ranges from the save-EXEC prologue through the EXEC restore.
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

/// A forward SCC branch that skips a reduction is safe without proving its
/// comparison sources are uniform: the SCC is a scalar register, identical in
/// every lane, so the branch is uniform. When it fires all active lanes skip the
/// region; when it does not fire all active lanes fall through to it. The region
/// is therefore never executed on a partial lane set. Only branches on the
/// skip path that could make the region run partially are rejected.
fn scc_forward_guard(
    insns: &[Insn],
    branch_index: usize,
    region_start: usize,
    reduction_end: usize,
) -> bool {
    let branch = &insns[branch_index];
    let pc = branch.offset as i64;
    let target = pc + 4 + 4 * branch.aux.imm;
    if target <= reduction_end as i64 {
        return false;
    }
    // Branches between the guard and the region whose edges skip or enter the
    // region are non-uniform and would make the whole-wave reduction execute
    // partially. Anything else in the window is a local conditional that cannot
    // skip the region; branches after the region are independent of its execution.
    // Indirect edges are never statically bounded.
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
            if from < pc || from >= region_start as i64 {
                continue;
            }
            let target_edge = from + 4 + 4 * op.aux.imm;
            if target_edge > reduction_end as i64 {
                // A uniform branch that skips past the region is a uniform skip:
                // it only fires when every lane leaves together, so it cannot make
                // the reduction run partially. Only a divergent (VCC) skip is not.
                if matches!(op.op, 6 | 7) {
                    return false;
                }
                continue;
            }
            if target_edge >= region_start as i64 {
                // A uniform branch landing exactly on the region start is a merge
                // (all active lanes arrive together); anything that lands strictly
                // inside the region is not.
                if target_edge == region_start as i64 && matches!(op.op, 2 | 4 | 5) {
                    continue;
                }
                return false;
            }
        }
    }
    true
}

/// Decide whether a branch crossing a reduction region is safe.
///
/// Only VCC branches (op 6/7) are divergent: their condition differs per lane.
/// SCC, EXEC and unconditional branches are uniform — every active lane takes or
/// falls through together — so they cannot make the reduction run partially.
/// A uniform branch that skips past the region is therefore safe: when it fires
/// (EXEC empty, or the scalar condition holds) every lane leaves the region
/// together, and when it does not fire every active lane reaches it together.
/// A uniform branch that lands exactly on the region start is a merge: every
/// active lane reaches the region together.
///
/// A backward branch that re-enters the region is the back-edge of a loop whose
/// header sits before the region. It is safe when the loop body up to the region
/// contains no divergent branch and no branch that lands strictly inside the
/// region, so the reduction is reached by every active lane on each iteration.
fn guard_region(
    insns: &[Insn],
    branch_index: usize,
    region_start: usize,
    reduction_end: usize,
    first: usize,
    operation: &str,
    verified_reads: &[usize],
) -> bool {
    let branch = &insns[branch_index];
    let pc = branch.offset as i64;
    if pc < region_start as i64 {
        // A divergent (VCC) branch crossing the region is never safe: its
        // condition differs per lane, so it can make the reduction run on only
        // a subset of the active lanes.
        if matches!(branch.op, 6 | 7) {
            return false;
        }
        // A uniform branch that lands exactly on the region start is a merge:
        // every active lane either takes it (to the region) or falls through to
        // the region, so the reduction is reached by all active lanes together.
        // Any divergent branch on the fall-through path is rejected by its own
        // check, so nothing further needs to be proven here.
        if matches!(branch.op, 2 | 4 | 5)
            && pc + 4 + 4 * branch.aux.imm == region_start as i64
        {
            return true;
        }
        // A forward SCC branch that skips the region is uniform (the SCC is a
        // scalar register), so it is safe without proving the comparison sources.
        if matches!(branch.op, 4 | 5) {
            return scc_forward_guard(insns, branch_index, region_start, reduction_end);
        }
        // Forward branch: the existing forward guards handle it (they reject
        // divergent branches and accept uniform skips of a masked region).
        return execz_forward_guard(
            insns,
            branch_index,
            region_start,
            reduction_end,
            first,
            operation,
            verified_reads,
        );
    }
    if pc > reduction_end as i64 {
        // Backward branch entering the region: the back-edge of a loop whose
        // header sits before the region. It is safe when the loop body up to the
        // region cannot make the reduction run on only some lanes: no divergent
        // (VCC) branch may sit there, and no branch may land strictly inside the
        // region. Uniform branches (EXEC, unconditional, SCC) are safe: they
        // preserve the property that all active lanes reach the region together
        // on each iteration (a uniform skip or merge leaves every lane in the
        // same place).
        // The back-edge itself must be uniform: a divergent (VCC) loop condition
        // makes different lanes iterate different numbers of times, so the
        // reduction in the body would run on a partial lane set.
        if branch.op == 1 || !Emitter::branch_cond(branch.op).is_some()
            || matches!(branch.op, 6 | 7)
        {
            return false;
        }
        let target = pc + 4 + 4 * branch.aux.imm;
        if target >= region_start as i64 {
            return false;
        }
        for (index, op) in insns.iter().enumerate() {
            if index == branch_index {
                continue;
            }
            if op.enc == Enc::Sopp && (Emitter::branch_cond(op.op).is_some() || op.op == 1) {
                let from = op.offset as i64;
                if from < target || from >= region_start as i64 {
                    continue;
                }
                // A divergent branch in the loop body before the region can make
                // the reduction execute on only a subset of the active lanes.
                if matches!(op.op, 6 | 7) {
                    return false;
                }
                let target_edge = from + 4 + 4 * op.aux.imm;
                // A branch landing strictly inside the region makes the reduction
                // execute partially.
                if target_edge > region_start as i64 && target_edge < reduction_end as i64 {
                    return false;
                }
            }
        }
        return true;
    }
    // A branch inside the region cannot be uniform.
    false
}

/// Validate fixed-width reductions and return EXECZ instruction indices whose
/// masked region and whole-wave arrival permit a wave-wide empty-EXEC vote.
pub(super) fn check_control_flow(insns: &[Insn], width: u32) -> Result<BTreeSet<usize>, String> {
    let mut execz_guards = BTreeSet::new();
    let mut verified_reads = Vec::new();
    for (first, region) in reduction_regions(insns, width) {
        let ins = &insns[first];
        let start = insns[*region.start()].offset as i64;
        let end = insns[*region.end()].offset as i64;
        let last_read = *region.end() - 1;
        for (branch_index, branch) in insns.iter().enumerate().filter(|(_, i)| i.enc == Enc::Sopp) {
            let pc = branch.offset as i64;
            if branch.op == 1 && pc < start {
                return Err(format!("wave reduction at {start:#x} after an early exit"));
            }
            if Emitter::branch_cond(branch.op).is_some() {
                let target = pc + 4 + 4 * branch.aux.imm;
                if (pc < start && target >= start) || (pc > end && target <= end)
                    || (pc >= start && pc <= end)
                {
                    if !guard_region(insns, branch_index, start as usize, end as usize, first, &ins.mnemonic, &verified_reads) {
                        if std::env::var("TUXBOX_WAVE_DEBUG").is_ok() {
                            let prev = insns.get(branch_index.checked_sub(1).unwrap_or(0));
                            eprintln!("WAVEDEBUG region=[{start:#x}..{end:#x}] op={} branch_idx={} op={} pc={:#x} target={:#x} prev={:?}",
                                ins.mnemonic, branch_index, branch.op, pc, target,
                                prev.map(|p| (p.mnemonic.as_str(), p.src.iter().map(|o| format!("{o:?}")).collect::<Vec<_>>())));
                        }
                        return Err(format!(
                            "wave reduction at {start:#x} may execute in divergent control flow"
                        ));
                    }
                    if branch.op == 8 {
                        execz_guards.insert(branch_index);
                    }
                }
            }
        }
        verified_reads.push(last_read);
    }
    Ok(execz_guards)
}

// Only a complete, single guest wave64 may use group rendezvous. Every
// branch must be proven group-uniform and every collective must be one of the
// exact recognized reductions. All other programs keep the native path.
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
        // A materialized identity belongs to the protected region too.
        let operation = i.mnemonic.as_str();
        if !matches!(operation, "v_add_nc_u32" | "v_add_f32" | "v_min_u32" | "v_max_u32" | "v_min_i32" | "v_max_i32" |
            "v_and_b32" | "v_or_b32" | "v_xor_b32" | "v_min_f32" | "v_max_f32") { return false; }
        let Some((covered_start, _, _)) = masked_identity(operation)
            .and_then(|identity| masked_prologue(insns, first, identity)) else { return false };
        if covered_start != start && !(covered_start + 1 == start && block[0].mnemonic == "v_mov_b32") {
            return false;
        }
        let save = &insns[covered_start];
        let [Opnd::Sgpr(saved)] = save.dst[..] else { return false };
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
            | "v_lshrrev_b32" | "v_mul_lo_u32" | "v_add_f32" | "v_mul_f32" | "v_ldexp_f32"
            | "s_mov_b32" | "s_movk_i32" | "s_add_u32" | "s_sub_u32"
            | "s_getpc_b64" | "s_lshl_b32" | "s_bitcmp0_b32" | "s_bitcmp1_b32" | "s_cselect_b32"
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

    // Synthetic self-contained masked reduction: save EXEC, force the full
    // mask, CNDMASK-neutralize inactive lanes, DPP shape, readlane, restore.
    // Mirrors the shape lower() accepts but is only control-flow shaped
    // enough for check_control_flow.
    fn masked_reduction(width: u32, result: u8) -> Vec<Insn> {
        let save_mnemonic = if width == 32 {
            "s_or_saveexec_b32"
        } else {
            "s_or_saveexec_b64"
        };
        let and_mnemonic = if width == 32 { "s_and_b32" } else { "s_and_b64" };
        let mut ops = vec![
            insn(save_mnemonic, Enc::Sop1, 0, vec![Opnd::Sgpr(8)], vec![Opnd::ConstInt(-1)]),
            insn(and_mnemonic, Enc::Sop1, 0, vec![Opnd::Sgpr(9)], vec![Opnd::Sgpr(7), Opnd::Sgpr(8)]),
            insn("v_cndmask_b32", Enc::Vop2, 0, vec![Opnd::Vgpr(1)], vec![Opnd::ConstInt(0), Opnd::Vgpr(1), Opnd::Sgpr(9)]),
            insn("v_or_b32", Enc::Vop2, DPP_REDUCE, vec![Opnd::Vgpr(1)], vec![Opnd::Vgpr(1)]),
            insn("v_or_b32", Enc::Vop2, DPP_REDUCE, vec![Opnd::Vgpr(1)], vec![Opnd::Vgpr(1)]),
            insn("v_or_b32", Enc::Vop2, DPP_REDUCE, vec![Opnd::Vgpr(1)], vec![Opnd::Vgpr(1)]),
            insn("v_or_b32", Enc::Vop2, DPP_REDUCE, vec![Opnd::Vgpr(1)], vec![Opnd::Vgpr(1)]),
            insn("v_permlanex16_b32", Enc::Vop1, 0, vec![Opnd::Vgpr(2)], vec![Opnd::Vgpr(1), Opnd::ConstInt(0), Opnd::ConstInt(0)]),
            insn("v_or_b32", Enc::Vop2, 0, vec![Opnd::Vgpr(1)], vec![Opnd::Vgpr(2), Opnd::Vgpr(1)]),
        ];
        for (index, step) in [(4, 1), (5, 2), (6, 3)] {
            ops[index].aux.imm = step;
        }
        if width == 32 {
            ops.push(insn("v_readlane_b32", Enc::Vop3, READ_LANE,
                          vec![Opnd::Sgpr(result)], vec![Opnd::Vgpr(1), Opnd::ConstInt(0)]));
            ops.push(insn("s_mov_b32", Enc::Sop1, 0, vec![Opnd::ExecLo], vec![Opnd::Sgpr(8)]));
        } else {
            ops.push(insn("v_readlane_b32", Enc::Vop3, READ_LANE,
                          vec![Opnd::Sgpr(result)], vec![Opnd::Vgpr(1), Opnd::ConstInt(32)]));
            ops.push(insn("v_or_b32", Enc::Vop2, 0, vec![Opnd::Vgpr(1)], vec![Opnd::Sgpr(result), Opnd::Vgpr(1)]));
            ops.push(insn("v_readlane_b32", Enc::Vop3, READ_LANE,
                          vec![Opnd::Sgpr(result)], vec![Opnd::Vgpr(1), Opnd::ConstInt(0)]));
            ops.push(insn("s_mov_b64", Enc::Sop1, 0, vec![Opnd::ExecLo], vec![Opnd::Sgpr(8)]));
        }
        ops
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
    fn row_mirror_reduction_accepts_either_bound_ctrl_with_full_masks() {
        for width in [32, 64] {
            let (mut shader, mut words, _) = fixture::shader(width);
            let reference = super::super::translate(&shader).unwrap();
            for word in &mut words {
                if *word == 0xff09_4002 { *word = 0xff01_4002; }
            }
            shader.isa = words.iter().flat_map(|w| w.to_le_bytes()).collect();
            shader.isa_data = shader.isa.clone();
            let translated = super::super::translate(&shader).unwrap();
            assert_eq!(translated.hlsl, reference.hlsl);
            assert_eq!(translated.required_wave_size, reference.required_wave_size);

            for bit in [1 << 24, 1 << 28] {
                let mut ops = crate::rdna2::disassemble(&shader.isa);
                let mirror = ops.iter().position(|i| i.raw.get(1) == Some(&0xff01_4002)).unwrap();
                ops[mirror].raw[1] ^= bit;
                assert!(lower(&mut ops, mirror - 3).is_err());
            }
        }
    }

    #[test]
    fn full_exec_reduction_ignores_fetch_inactive_at_each_shuffle() {
        for width in [32, 64] {
            let (mut shader, words, _) = fixture::shader(width);
            let expected = super::super::translate(&shader).unwrap();
            // Three DPP8 selectors followed by the DPP16 ROW_MIRROR control word.
            for word_index in [3, 5, 7, 10] {
                let mut changed = words.clone();
                if word_index == 10 {
                    changed[word_index] |= 1 << 18;
                } else {
                    changed[word_index] += 1; // DPP8 FI=1 uses selector 234 instead of 233.
                }
                shader.isa = changed.iter().flat_map(|w| w.to_le_bytes()).collect();
                shader.isa_data = shader.isa.clone();
                let translated = super::super::translate(&shader).unwrap();
                assert_eq!(translated.hlsl, expected.hlsl);
                assert_eq!(translated.required_wave_size, expected.required_wave_size);

                let mut ops = crate::rdna2::disassemble(&shader.isa);
                ops[0].src = vec![Opnd::ConstInt(0)];
                assert!(lower(&mut ops, 2).is_err(), "reduction requires full EXEC");
            }
        }
    }

    #[test]
    fn dpp8_reductions_read_from_the_complementary_subgroup() {
        for width in [32, 64] {
            let (mut shader, mut words, _) = fixture::shader(width);
            // Each lane picks its XOR-2 partner, then its XOR-4 partner.
            words[6] = 0xb3e2_1a02;
            words[8] = 0x688f_ac02;
            shader.isa = words.iter().flat_map(|word| word.to_le_bytes()).collect();
            shader.isa_data = shader.isa.clone();
            let translated = super::super::translate(&shader).unwrap();
            let lane = if width == 32 { "WaveGetLaneIndex()" } else { "gtid.x" };
            for distance in [2, 4] {
                assert!(translated.hlsl.contains(&format!("{lane} ^ {distance}u")));
            }

            // An asymmetric table must retain its individual source lanes.
            words[8] = 0x4c1d_e502; // [5, 4, 7, 6, 1, 0, 3, 2]
            shader.isa = words.iter().flat_map(|word| word.to_le_bytes()).collect();
            shader.isa_data = shader.isa.clone();
            let mixed = super::super::translate(&shader).unwrap();
            assert!(mixed.hlsl.contains(&format!("0x4c1de5u >> (3u * ({lane} & 7u))")));

            // One destination reading its own half would double-count a partial sum.
            for (word_index, distance) in [(4, 1), (6, 2), (8, 4)] {
                let mut invalid = words.clone();
                invalid[word_index] ^= distance << 8;
                let isa = invalid.iter().flat_map(|word| word.to_le_bytes()).collect::<Vec<_>>();
                assert!(lower(&mut crate::rdna2::disassemble(&isa), 2).is_err());
            }
        }
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
            // Replace the proven scalar with an unproven entry register; the SCC
            // branch is uniform regardless of the source, so it is still accepted.
            words[cmp] = 0xbf06_8008;
            ex.isa = words.iter().flat_map(|word| word.to_le_bytes()).collect();
            ex.isa_data = ex.isa.clone();
            assert!(super::super::translate(&ex).is_ok());
        }
    }

    #[test]
    fn native_scc_guard_translates_with_any_source() {
        for width in [32, 64] {
            let (mut ex, mut words, cmp) = fixture::shader(width);
            // s_mov_b32 s7, 1; the following comparison and branch are unchanged.
            words.insert(cmp, 0xbe87_0381);
            ex.isa = words.iter().flat_map(|word| word.to_le_bytes()).collect();
            ex.isa_data = ex.isa.clone();
            let translated = super::super::translate(&ex).unwrap();
            assert!(translated.hlsl.contains("s7 = 1u;") || translated.hlsl.contains("s7 = 0x1u;") || translated.hlsl.contains("s7 = 0x00000001u;"), "{}", translated.hlsl);
            // s_mov_b32 s7, s8 (unknown input) is still accepted: the SCC branch
            // is uniform regardless of the comparison source.
            words[cmp] = 0xbe87_0308;
            ex.isa = words.iter().flat_map(|word| word.to_le_bytes()).collect();
            ex.isa_data = ex.isa.clone();
            assert!(super::super::translate(&ex).is_ok());
        }
    }

    #[test]
    fn bitwise_and_float_sum_reductions_use_shared_memory_only_for_a_single_wave64() {
        for (opcode, neutral, combine) in [
            (0x36, 0xc1, ") & v2;"), (0x38, 0x80, ") | v2;"),
            (0x3a, 0x80, ") ^ v2;"), (0x06, 0x80, ")) + asfloat(v2))"),
        ] {
            let (mut shader, mut words, end) = fixture::shader(64);
            words.truncate(end);
            words[2] = (words[2] & !0xff) | neutral;
            for word in &mut words {
                if *word >> 24 == 0x4a { *word = (opcode << 24) | (*word & 0xffffff); }
            }
            words.push(0xbf81_0000);
            shader.isa = words.iter().flat_map(|word| word.to_le_bytes()).collect();
            shader.isa_data = shader.isa.clone();
            let single = super::super::translate(&shader).unwrap();
            assert_eq!(single.required_wave_size, None);
            assert!(single.hlsl.lines().any(|line| line.contains("tb_reduction_read(gtid.x")
                && line.contains(combine)));

            shader.isa_preamble = [0, 128, 1, 1];
            let multi = super::super::translate(&shader).unwrap();
            assert_eq!(multi.required_wave_size, Some(64));
        }
    }

    #[test]
    fn portable_wave_can_scale_its_result_from_an_embedded_table() {
        let (mut shader, mut words, end) = fixture::shader(64);
        words.truncate(end);
        words.extend([
            0xbe9a1f00, 0xbf0d8005, 0x851c8084, // GETPC; bit-test group ID; choose offset 4 or 0
            0xf400074d, 0x38000028, 0xbf8cc07f, // load exponent into s29 and wait
            0x7e0c0207, 0xd7620006, 0x00003b06, // move reduction result s7; LDEXP v6,v6,s29
            0x8f1e8805, 0xbf810000,             // scale group ID for output addressing; endpgm
        ]);
        shader.isa = words.iter().flat_map(|word| word.to_le_bytes()).collect();
        shader.isa_data = shader.isa.clone();
        shader.isa_data.extend([u32::MAX, 2].into_iter().flat_map(u32::to_le_bytes));
        assert_eq!(super::super::translate(&shader).unwrap().required_wave_size, None);
        shader.isa_preamble = [0, 128, 1, 1];
        assert_eq!(super::super::translate(&shader).unwrap().required_wave_size, Some(64));

        let mut ops = crate::rdna2::disassemble(&shader.isa);
        super::super::lower_wave_reductions(&mut ops).unwrap();
        let load = ops.iter_mut().find(|i| i.mnemonic == "s_load_dword").unwrap();
        load.raw[1] &= 0x01ffffff; // SOFFSET s0 would expose the saved EXEC mask.
        assert!(!portable_single_wave(&ops));
    }

    #[test]
    fn portable_extrema_requires_direct_exec_and_no_mask_escape() {
        for operation in ["umin", "umax", "smin", "smax", "fmin", "fmax"] {
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
            if matches!(operation, "smin" | "smax" | "fmin" | "fmax") {
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
    fn extrema_accept_the_same_identity_as_a_direct_literal() {
        for op in ["smin", "smax", "fmin", "fmax"] {
            let mut shader = fixture::extrema(64, op);
            let mut words: Vec<u32> = shader.isa.chunks_exact(4)
                .map(|b| u32::from_le_bytes(b.try_into().unwrap())).collect();
            // Replace the literal move and register CNDMASK with a literal CNDMASK.
            let select = [words[3], (words[4] & !0x1ff) | 0xff, words[2]];
            words.splice(1..5, select);
            shader.isa = words.iter().flat_map(|w| w.to_le_bytes()).collect();
            shader.isa_data = shader.isa.clone();
            let translated = super::super::translate(&shader).unwrap();
            assert_eq!(translated.required_wave_size, None);
            assert!(translated.hlsl.contains("tb_reduction_read(gtid.x"));

            words[3] ^= 1;
            let isa = words.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<_>>();
            assert!(lower(&mut crate::rdna2::disassemble(&isa), 2).is_err());
        }
    }

    #[test]
    fn signed_extrema_with_an_unproven_shuffle_cannot_become_lane_local() {
        for operation in ["smin", "smax"] {
            let mut shader = fixture::extrema(64, operation);
            let mut words: Vec<u32> = shader.isa.chunks_exact(4)
                .map(|b| u32::from_le_bytes(b.try_into().unwrap())).collect();
            let shuffle = words.iter_mut().find(|w| **w == 0xde54_c102).unwrap();
            *shuffle ^= 1 << 8; // Lane zero now reads itself instead of its partner.
            shader.isa = words.iter().flat_map(|w| w.to_le_bytes()).collect();
            shader.isa_data = shader.isa.clone();
            assert!(super::super::translate(&shader).is_err());
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

    #[test]
    fn scc_guard_must_skip_the_entire_reduction() {
        // An SCC branch is uniform (the SCC is a scalar register), so it may skip
        // the whole reduction regardless of the comparison's sources. It must
        // however skip the entire region, not land inside it.
        for width in [32, 64] {
            let mut ops = vec![compare(vec![Opnd::ConstInt(1), Opnd::Literal(0)]), branch(4)];
            ops.extend(reduction(width, 7));
            let after = ops.len();
            ops.push(nop());
            edge(&mut ops, 1, after, 4);
            assert!(check_control_flow(&ops, width).is_ok());
            // The compare may instead use a scalar register; the SCC is uniform,
            // so the branch is still safe.
            ops[0].src[0] = Opnd::Sgpr(9);
            assert!(check_control_flow(&ops, width).is_ok());
            ops[0].src[0] = Opnd::ConstInt(1);
            edge(&mut ops, 1, after - 1, 4);
            assert!(check_control_flow(&ops, width).is_err());
            edge(&mut ops, 1, after, 8); // EXEC is a per-invocation mask
            assert!(check_control_flow(&ops, width).is_err());
        }
    }

    #[test]
    fn scc_guard_accepts_unproven_source_without_bypass() {
        // An SCC branch skipping the region is uniform regardless of the
        // comparison source (the SCC is a scalar register). A branch that bypasses
        // the region by landing inside it is still rejected.
        for width in [32, 64] {
            let mut ops = vec![nop(), insn("s_mov_b32", Enc::Sop1, 0,
                vec![Opnd::Sgpr(9)], vec![Opnd::ConstInt(1)]),
                compare(vec![Opnd::Sgpr(9), Opnd::ConstInt(0)]), branch(4)];
            ops.extend(reduction(width, 7));
            let after = ops.len();
            ops.push(nop());
            edge(&mut ops, 3, after, 4);
            assert!(check_control_flow(&ops, width).is_ok());
            // A branch landing inside the region bypasses its prologue.
            edge(&mut ops, 3, 5, 4);
            assert!(check_control_flow(&ops, width).is_err());
        }
    }

    #[test]
    fn scc_guard_accepts_scalar_chain_source() {
        // An SCC branch skipping the region is uniform even when the comparison
        // source is computed by a scalar chain; no uniformity proof is required.
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
            assert!(check_control_flow(&ops, width).is_ok());
            // A divergent (VCC) branch skipping the region is still rejected.
            edge(&mut ops, 5, after, 6);
            assert!(check_control_flow(&ops, width).is_err());
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
            assert!(check_control_flow(&ops, width).is_ok());
            edge(&mut ops, 0, 3, 8);
            assert!(check_control_flow(&ops, width).is_err());
            ops[0] = nop();
            offsets(&mut ops);
            edge(&mut ops, 1, after - 1, 2);
            assert!(check_control_flow(&ops, width).is_err());
        }
    }

    #[test]
    fn scc_guard_accepts_prior_reduction_source() {
        // An SCC branch skipping the region is uniform even when the comparison
        // source is a prior reduction's result; no uniformity proof is required.
        for width in [32, 64] {
            let mut ops = reduction(width, 7);
            ops.push(nop());
            ops.push(compare(vec![Opnd::Sgpr(7), Opnd::ConstInt(0)]));
            let guard = ops.len();
            ops.push(branch(5));
            ops.extend(reduction(width, 8));
            let after = ops.len();
            ops.push(nop());
            edge(&mut ops, guard, after, 5);
            assert!(check_control_flow(&ops, width).is_ok());
            // The comparison source may be any scalar register.
            ops[guard - 1].src[0] = Opnd::Sgpr(8);
            offsets(&mut ops);
            assert!(check_control_flow(&ops, width).is_ok());
        }
    }

    #[test]
    fn execz_forward_skip_accepts_self_contained_region() {
        // EXEC-empty skip: every lane takes the skip and the region never
        // runs; otherwise the region forces the full mask via its saved-EXEC
        // prologue, so the reduction runs cooperatively for the whole wave.
        for width in [32, 64] {
            let mut ops = vec![nop(), branch(8)];
            ops.extend(masked_reduction(width, 7));
            let after = ops.len();
            ops.push(nop());
            edge(&mut ops, 1, after, 8);
            assert_eq!(check_control_flow(&ops, width).unwrap(), BTreeSet::from([1]));
        }
    }

    #[test]
    fn execz_forward_skip_accepts_earlier_skips_of_the_whole_region() {
        // The shape compilers emit for nested conditions: an outer skip, then
        // the guard, both leaving past the reduction.
        for width in [32, 64] {
            for (outer, extra) in [(8, 0), (8, 1), (5, 0)] {
                let mut ops = vec![nop(), branch(outer), nop(), branch(8)];
                ops.extend(masked_reduction(width, 7));
                let after = ops.len();
                ops.extend([nop(), nop()]);
                edge(&mut ops, 1, after + extra, outer);
                edge(&mut ops, 3, after, 8);
                // Each EXECZ is voted on by the whole wave.
                let voted = if outer == 8 { BTreeSet::from([1, 3]) } else { BTreeSet::from([3]) };
                assert_eq!(check_control_flow(&ops, width).unwrap(), voted, "outer {outer} width {width}");
            }
        }
    }

    #[test]
    fn execz_forward_skip_rejects_earlier_branches_it_cannot_vote_on() {
        for width in [32, 64] {
            // A divergent skip, and an outer skip that lands inside the region.
            for (outer, inside) in [(6, false), (7, false), (9, false), (8, true), (5, true)] {
                let mut ops = vec![nop(), branch(outer), nop(), branch(8)];
                ops.extend(masked_reduction(width, 7));
                let after = ops.len();
                ops.push(nop());
                edge(&mut ops, 1, if inside { 7 } else { after }, outer);
                edge(&mut ops, 3, after, 8);
                assert!(check_control_flow(&ops, width).is_err(), "outer {outer} width {width}");
            }
        }
    }

    #[test]
    fn execz_forward_skip_rejects_enclosing_loop_before_region() {
        for width in [32, 64] {
            let mut ops = vec![nop(), branch(8), nop(), branch(4)];
            ops.extend(masked_reduction(width, 7));
            let after = ops.len();
            ops.push(nop());
            edge(&mut ops, 1, after, 8);
            // This loop encloses the vote but ends before the reduction.
            edge(&mut ops, 3, 0, 4);
            assert!(check_control_flow(&ops, width).is_err());
        }
    }

    #[test]
    fn execz_forward_skip_rejects_bypass_and_reentry() {
        for width in [32, 64] {
            let mut ops = vec![nop(), branch(8)];
            ops.extend(masked_reduction(width, 7));
            let after = ops.len();
            ops.push(nop());
            edge(&mut ops, 1, after, 8);
            assert!(check_control_flow(&ops, width).is_ok());
            // Another branch enters the skipped interval after the guard.
            edge(&mut ops, 0, 2, 8);
            assert!(check_control_flow(&ops, width).is_err());
            // A backward edge from after the region re-enters it.
            let mut ops = vec![nop(), branch(8)];
            ops.extend(masked_reduction(width, 7));
            let after = ops.len();
            ops.push(nop());
            edge(&mut ops, 1, after, 8);
            edge(&mut ops, after, 2, 8);
            assert!(check_control_flow(&ops, width).is_err());
        }
    }

    #[test]
    fn execz_forward_skip_rejects_indirect_edges() {
        let mut ops = vec![nop(), branch(8)];
        ops.extend(masked_reduction(32, 7));
        let after = ops.len();
        ops.push(nop());
        edge(&mut ops, 1, after, 8);
        assert!(check_control_flow(&ops, 32).is_ok());
        // An indirect edge is never statically bounded.
        ops[2] = insn("s_setpc_b64", Enc::Sop1, 0, vec![], vec![]);
        assert!(check_control_flow(&ops, 32).is_err());
    }

    #[test]
    fn execz_forward_skip_rejects_unmasked_region() {
        // Only the masked full-mask form is convergent under partial EXEC;
        // a region without the saved-EXEC CNDMASK prologue must be refused
        // even with an execz skip guard.
        for width in [32, 64] {
            let mut ops = vec![nop(), branch(8)];
            ops.extend(reduction(width, 7));
            let after = ops.len();
            ops.push(nop());
            edge(&mut ops, 1, after, 8);
            assert!(check_control_flow(&ops, width).is_err());
        }
    }

    #[test]
    fn execz_forward_skip_accepts_tail_branch_after_region() {
        // A local forward branch inside the skip window that starts and
        // resolves after the region cannot bypass or enter the region; the
        // region itself is convergent because it forces the full mask.
        for width in [32, 64] {
            let mut ops = vec![nop(), branch(8)];
            ops.extend(masked_reduction(width, 7));
            let tail = ops.len();
            ops.push(insn("s_cbranch_scc0", Enc::Sopp, 4, vec![], vec![]));
            ops.push(nop());
            ops.push(nop());
            let end = ops.len();
            edge(&mut ops, 1, end, 8);
            edge(&mut ops, tail, tail + 2, 4);
            assert!(check_control_flow(&ops, width).is_ok());
        }
    }

    #[test]
    fn execz_forward_skip_rejects_branch_skipping_the_region() {
        // A divergent branch inside the skip interval that reaches the region
        // would bypass or partially execute the whole-wave reduction; it is
        // rejected. (A uniform branch landing exactly on the region start is a
        // safe merge, covered by execz_skip_allows_uniform_merge_branch_in_window.)
        for width in [32, 64] {
            let mut ops = vec![nop(), branch(8), nop()];
            ops.extend(masked_reduction(width, 7));
            let end = ops.len();
            edge(&mut ops, 1, end, 8);
            // A divergent branch landing on the region start.
            edge(&mut ops, 2, 3, 6);
            assert!(check_control_flow(&ops, width).is_err());
            // A uniform branch landing strictly inside the region.
            edge(&mut ops, 2, 6, 4);
            assert!(check_control_flow(&ops, width).is_err());
        }
    }

    #[test]
    fn scc_guard_rejects_divergent_entry_and_early_exit() {
        // An SCC branch skipping the region is uniform, but a divergent (VCC)
        // branch entering the region, or an early exit before the region, is
        // rejected.
        for width in [32, 64] {
            let mut ops = vec![nop(), compare(vec![Opnd::ConstInt(0), Opnd::ConstInt(0)]), branch(5)];
            ops.extend(reduction(width, 7));
            let after = ops.len();
            ops.push(nop());
            ops.push(nop());
            edge(&mut ops, 2, after, 5);
            assert!(check_control_flow(&ops, width).is_ok());
            // A divergent branch entering the region is rejected.
            edge(&mut ops, 0, 4, 6);
            assert!(check_control_flow(&ops, width).is_err());
            // An early exit before the region is rejected.
            ops[0] = insn("s_endpgm", Enc::Sopp, 1, vec![], vec![]);
            offsets(&mut ops);
            assert!(check_control_flow(&ops, width).is_err());
        }
    }

    #[test]
    fn uniform_branch_landing_on_region_start_is_a_safe_merge() {
        // A uniform branch whose target is exactly the region start is a merge:
        // every active lane either takes it (to the region) or falls through to
        // the region, so the reduction is reached by all active lanes together.
        for width in [32, 64] {
            let mut ops = vec![compare(vec![Opnd::ConstInt(1), Opnd::ConstInt(0)]), branch(5)];
            ops.extend(reduction(width, 7));
            // The reduction (and its saved-EXEC prologue) starts at index 2.
            edge(&mut ops, 1, 2, 5);
            assert!(check_control_flow(&ops, width).is_ok());
            // A divergent (VCC) branch landing on the region start is not safe.
            edge(&mut ops, 1, 2, 6);
            assert!(check_control_flow(&ops, width).is_err());
        }
    }

    #[test]
    fn branch_landing_inside_the_reduction_is_rejected() {
        // A branch that lands strictly inside the region would make the
        // reduction execute partially; it is rejected for both uniform and
        // divergent branches.
        for width in [32, 64] {
            let mut ops = vec![compare(vec![Opnd::ConstInt(1), Opnd::ConstInt(0)]), branch(5)];
            ops.extend(reduction(width, 7));
            // Index 5 is strictly inside the region [2, 2 + len).
            edge(&mut ops, 1, 5, 5);
            assert!(check_control_flow(&ops, width).is_err());
            edge(&mut ops, 1, 5, 6);
            assert!(check_control_flow(&ops, width).is_err());
        }
    }

    #[test]
    fn loop_back_edge_with_uniform_body_branches_is_safe() {
        // The back-edge of a loop whose body up to the region contains only
        // uniform branches is safe: the region is reached by every active lane
        // on each iteration.
        for width in [32, 64] {
            let mut ops = vec![
                nop(), // loop header
                compare(vec![Opnd::ConstInt(1), Opnd::ConstInt(0)]),
                branch(5), // uniform scc1 in the body
            ];
            ops.extend(reduction(width, 7)); // region starts at index 3
            let after = ops.len();
            ops.push(nop());
            ops.push(nop());
            edge(&mut ops, 2, 3, 5); // body branch lands on the region start
            edge(&mut ops, after, 0, 4); // back-edge (scc0) to the loop header
            assert!(check_control_flow(&ops, width).is_ok());
        }
    }

    #[test]
    fn loop_back_edge_with_divergent_body_branch_is_rejected() {
        // A divergent (VCC) branch in the loop body before the region can make
        // the reduction run on only some lanes, even though the back-edge is
        // uniform.
        for width in [32, 64] {
            let mut ops = vec![
                nop(), // loop header
                compare(vec![Opnd::ConstInt(1), Opnd::ConstInt(0)]),
                branch(6), // divergent vcc in the body
            ];
            ops.extend(reduction(width, 7)); // region starts at index 3
            let after = ops.len();
            ops.push(nop());
            ops.push(nop());
            edge(&mut ops, 2, 0, 6); // divergent body branch to the header
            edge(&mut ops, after, 0, 4); // back-edge (scc0) to the loop header
            assert!(check_control_flow(&ops, width).is_err());
        }
    }

    #[test]
    fn loop_back_edge_divergent_condition_is_rejected() {
        // A divergent (VCC) back-edge makes different lanes iterate different
        // numbers of times, so the reduction in the body would run on a partial
        // lane set; it is rejected even when the loop body is otherwise uniform.
        for width in [32, 64] {
            let mut ops = vec![
                nop(), // loop header
                compare(vec![Opnd::ConstInt(1), Opnd::ConstInt(0)]),
                branch(5), // uniform scc1 in the body
            ];
            ops.extend(reduction(width, 7)); // region starts at index 3
            let after = ops.len();
            ops.push(nop());
            ops.push(nop());
            edge(&mut ops, 2, 3, 5); // uniform body branch to the region start
            edge(&mut ops, after, 0, 7); // divergent (vccnz) back-edge to the header
            assert!(check_control_flow(&ops, width).is_err());
        }
    }

    #[test]
    fn execz_skip_allows_uniform_merge_branch_in_window() {
        // An EXEC-empty skip of a masked region is safe even when the skip
        // window contains a uniform branch that lands on the region start
        // (a merge): all active lanes reach the region together.
        for width in [32, 64] {
            let mut ops = vec![
                nop(),
                branch(8), // execz skip
                compare(vec![Opnd::ConstInt(1), Opnd::ConstInt(0)]),
                branch(5), // uniform scc1
            ];
            ops.extend(masked_reduction(width, 7)); // region starts at index 4
            let after = ops.len();
            ops.push(nop());
            ops.push(nop());
            edge(&mut ops, 1, after, 8); // execz skip lands past the region
            edge(&mut ops, 3, 4, 5); // uniform branch lands on the region start
            assert!(check_control_flow(&ops, width).is_ok());
        }
    }
}

