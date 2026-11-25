//! Proofs that a program elects one lane, and the register queries they use.
use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::hlsl) enum ElectionLowering {
    Native32,
    Shared64,
}

/// Select the lane geometry only for ordinary straight-line compute kernels.
/// A group with at most 32 threads fits in one native wave even for a b64 search.
/// Larger wave64 elections reuse the workgroup exchange across both halves.
pub(in crate::hlsl) fn straight_election(ex: &ShaderExtract, insns: &[Insn]) -> Option<ElectionLowering> {
    if ex.stage != Stage::Compute || ex.fspx_config.is_some() { return None; }
    let threads = compute_group(ex).into_iter().fold(1u32, u32::saturating_mul);
    if threads == 0 || threads > 1024 { return None; }
    let first = insns.iter().find(|i| matches!(i.mnemonic.as_str(), "s_ff1_i32_b32" | "s_ff1_i32_b64"));
    // A small group fits one wave of either guest width, even without FF1.
    if first.is_none() && (threads > 32 || !insns.iter().any(exec_snapshot::captures_exec)) { return None; }
    let (end, body) = insns.split_last()?;
    if end.mnemonic != "s_endpgm" || !body.iter().all(|i| match i.mnemonic.as_str() {
        "s_ff1_i32_b32" | "s_ff1_i32_b64" => first.is_some_and(|f| f.mnemonic == i.mnemonic) && i.src == [Opnd::ExecLo],
        "s_load_dwordx4" | "s_waitcnt" | "s_nop" | "s_mov_b32" | "s_mov_b64"
        | "s_and_saveexec_b32" | "s_and_saveexec_b64" | "v_mbcnt_lo_u32_b32" | "v_mbcnt_hi_u32_b32"
        | "v_mov_b32" | "v_and_b32" | "v_lshlrev_b32" | "v_lshrrev_b32"
        | "v_add_nc_u32" | "v_cndmask_b32" | "buffer_store_dword" => true,
        m => m.starts_with("v_cmp_") || m.starts_with("v_cmpx_"),
    }) { return None; }
    Some(if threads <= 32 || first.is_some_and(|f| f.mnemonic == "s_ff1_i32_b32") {
        ElectionLowering::Native32
    } else {
        ElectionLowering::Shared64
    })
}

/// In a straight-line compute shader, a compare/save/FF1/restore sequence
/// produces a uniform scalar value. Recognize every mask access before allowing
/// uses other than the usual comparison against the lane index.
pub(in crate::hlsl) fn predicate_snapshots(mut code: &[Insn]) -> bool {
    let mask = |o: &Opnd| matches!(o, Opnd::ExecLo | Opnd::ExecHi | Opnd::VccLo | Opnd::VccHi);
    // Numeric vector operands can read the ballot captured at SAVEEXEC. Other
    // consumers still require proof about the representation they expect.
    let mut saved_words = BTreeMap::new();
    let count = code.len();
    while let [op, rest @ ..] = code {
        if exec_snapshot::copy_scalar_words(op, &mut saved_words) {
            code = rest;
            continue;
        }
        let numeric = exec_snapshot::numeric_sources(op) != 0;
        if let ("v_cndmask_b32", Some(Opnd::Sgpr(lo))) = (op.mnemonic.as_str(), op.src.get(2)) {
            let pair = [saved_words.get(lo), saved_words.get(&(lo + 1))];
            if pair.iter().any(|w| matches!(w, Some(exec_snapshot::Word::Saved(..))))
                && (op.enc != Enc::Vop3 || op.aux.sdwa.is_some() || pair.contains(&None))
            {
                return false;
            }
        }
        if !numeric && saved_words.iter().any(|(&reg, word)| matches!(word, exec_snapshot::Word::Saved(..))
            && (reads_sgpr(op, reg) || op.mnemonic.ends_with("64") && op.src.iter().any(|o| may_touch_sgpr(o, reg))))
        {
            return false;
        }
        let i = count - code.len();
        if op.mnemonic.starts_with("v_cmp_") && op.dst == [Opnd::VccLo] {
            let [save, search, restore, tail @ ..] = rest else { return false };
            let [Opnd::Sgpr(saved)] = save.dst.as_slice() else { return false };
            let [Opnd::Sgpr(result)] = search.dst.as_slice() else { return false };
            if op.src.iter().any(mask) || save.mnemonic != "s_and_saveexec_b64" || save.src != [Opnd::VccLo]
                || search.mnemonic != "s_ff1_i32_b64" || search.src != [Opnd::ExecLo]
                || result == saved || u16::from(*result) == u16::from(*saved) + 1
                || restore.mnemonic != "s_mov_b64" || restore.dst != [Opnd::ExecLo]
                || restore.src != [Opnd::Sgpr(*saved)]
            {
                return false;
            }
            exec_snapshot::record_scalar_write(save, i + 1, &mut saved_words);
            exec_snapshot::record_scalar_write(search, i + 2, &mut saved_words);
            code = tail;
        } else {
            if op.src.iter().chain(&op.dst).any(mask) || op.mnemonic.starts_with("v_cmpx_") {
                return false;
            }
            exec_snapshot::record_scalar_write(op, i, &mut saved_words);
            code = rest;
        }
    }
    true
}

/// True when `o` may name SGPR `n` as part of a 32- or 64-bit operand.
pub(in crate::hlsl) fn may_touch_sgpr(o: &Opnd, n: u8) -> bool {
    matches!(*o, Opnd::Sgpr(k) if u16::from(k) + 1 >= u16::from(n) && u16::from(k) <= u16::from(n) + 1)
}

/// Whether `ins` may read SGPR `n`, by operand width: scalar 64-bit operations,
/// vector lane-mask sources, and descriptor or address bases of memory operations.
pub(in crate::hlsl) fn reads_sgpr(ins: &Insn, n: u8) -> bool {
    let m = ins.mnemonic.as_str();
    // GETPC has no source. Its unused SOP1 source field decodes as s0.
    if m == "s_getpc_b64" { return false; }
    // Scalar loads encode their offset register separately from the base operands.
    if ins.enc == Enc::Smem && n <= 105 && ins.raw.get(1).is_some_and(|w| w >> 25 == u32::from(n)) {
        return true;
    }
    let memory = if m.starts_with("image_") { 8 }
        else if m.starts_with("s_buffer") || m.starts_with("buffer_") || m.starts_with("tbuffer_") { 4 }
        else if m.starts_with("s_load") || m.starts_with("global_") { 2 } else { 0 };
    ins.src.iter().enumerate().any(|(i, o)| {
        // VOP3 decodes three source fields; LDEXP uses only the value and exponent.
        if m == "v_ldexp_f32" && i == 2 { return false; }
        let Opnd::Sgpr(k) = *o else { return false };
        let width: u16 = if memory != 0 { memory } else if m.starts_with("s_") && (m.ends_with("64") || m.contains("saveexec_b64")) { 2 }
            else if m.starts_with("v_") && i >= 2 { 2 } else { 1 };
        u16::from(k) <= u16::from(n) && u16::from(n) < u16::from(k) + width
    })
}

pub(in crate::hlsl) fn branch_pc(ins: &Insn) -> i64 {
    ins.offset as i64 + 4 + ins.aux.imm * 4
}

pub(in crate::hlsl) fn branch_targets(insns: &[Insn]) -> BTreeSet<i64> {
    insns.iter()
        .filter(|b| b.enc == Enc::Sopp && Emitter::branch_cond(b.op).is_some())
        .map(|b| b.offset as i64 + 4 + 4 * b.aux.imm)
        .collect()
}

/// The last instruction before `at` that writes VGPR `v`, if no branch enters between.
pub(in crate::hlsl) fn vgpr_def(insns: &[Insn], targets: &BTreeSet<i64>, at: usize, v: u8) -> Option<usize> {
    for j in (0..at).rev() {
        if targets.contains(&(insns[j + 1].offset as i64)) { return None; }
        if insns[j].dst.contains(&Opnd::Vgpr(v)) { return Some(j); }
    }
    None
}

/// Verify a first-active-lane election that the generic lowering already expresses:
/// `s_ff1_i32_b64 s, exec` (or `_b32`) compared with the full-mask `v_mbcnt` lane index. The
/// elected lane usually adds the number of active lanes to a counter; that count
/// reads an EXEC copy as a wave mask, so capture the copy as a ballot.
pub(in crate::hlsl) fn verify_lane_election(insns: &mut [Insn], ff1: usize, straight_compute: bool) -> Result<(), String> {
    let at = insns[ff1].offset;
    let bad = |what: &str| format!("unverified EXEC election at {at:#x}: {what}");
    let Some(&Opnd::Sgpr(lane)) = insns[ff1].dst.first() else { return Err(bad("no scalar destination")) };
    let targets = branch_targets(insns);
    // Two-source VOP3 forms decode an unused third source; ignore it.
    let sources = |ins: &Insn| {
        let two = ins.mnemonic.starts_with("v_cmp_") || ins.mnemonic.starts_with("v_mbcnt_");
        ins.src.iter().take(if two { 2 } else { usize::MAX }).cloned().collect::<Vec<_>>()
    };
    let cmp = (ff1 + 1..insns.len())
        .find(|&j| {
            let ins = &insns[j];
            if straight_compute && ins.mnemonic == "s_mov_b32" {
                return ins.src.iter().chain(&ins.dst).any(|o| *o == Opnd::Sgpr(lane));
            }
            sources(ins).iter().chain(&ins.dst).any(|o| may_touch_sgpr(o, lane))
        })
        .ok_or_else(|| bad("unused lane"))?;
    if (ff1 + 1..=cmp).any(|j| targets.contains(&(insns[j].offset as i64))) {
        return Err(bad("branch enters the election"));
    }
    let c = &insns[cmp];
    // The compare either fills a mask for a later saveexec or narrows EXEC itself.
    let narrows = matches!(c.mnemonic.as_str(), "v_cmpx_eq_i32" | "v_cmpx_eq_u32");
    if !(narrows || matches!(c.mnemonic.as_str(), "v_cmp_eq_i32" | "v_cmp_eq_u32")) {
        return Err(bad("lane is not compared for equality"));
    }
    // Equality is symmetric. The VOP3 encoding permits the elected scalar
    // on either side; preserve all existing full-lane-index proof checks.
    let index = match c.src.get(..2) {
        Some([Opnd::Sgpr(reg), Opnd::Vgpr(v)]) | Some([Opnd::Vgpr(v), Opnd::Sgpr(reg)]) if *reg == lane => *v,
        _ => return Err(bad("lane is not compared for equality with a VGPR")),
    };
    let bits = if insns[ff1].mnemonic.ends_with("_b32") { "b32" } else { "b64" };
    let full_low = |l: &Insn| l.mnemonic == "v_mbcnt_lo_u32_b32" && l.aux.mask == 0
        && l.src.get(..2) == Some(&[Opnd::ConstInt(-1), Opnd::ConstInt(0)][..]);
    let last = vgpr_def(insns, &targets, cmp, index).ok_or_else(|| bad("no lane index"))?;
    let h = &insns[last];
    // Wave32 counts the low lanes only; wave64 adds the high half.
    let lane_index = if bits == "b32" { full_low(h) } else {
        let Some(&Opnd::Vgpr(low)) = h.src.get(1) else { return Err(bad("lane index is not v_mbcnt")) };
        let lo = vgpr_def(insns, &targets, last, low).ok_or_else(|| bad("no low lane count"))?;
        h.mnemonic == "v_mbcnt_hi_u32_b32" && h.src[0] == Opnd::ConstInt(-1) && h.aux.mask == 0 && full_low(&insns[lo])
    };
    if !lane_index {
        return Err(bad("lane index is not a full-mask v_mbcnt"));
    }
    // Inside the elected branch, an active-lane count reads an EXEC copy made earlier.
    let (save, restore) = if narrows {
        // EXEC was copied before the compare. The branch lasts until any copy is
        // restored, or to the end of a program that does not need the mask again.
        let restore = (cmp + 1..insns.len())
            .find(|&j| insns[j].dst.first() == Some(&Opnd::ExecLo) && insns[j].mnemonic.starts_with("s_"))
            .unwrap_or(insns.len());
        (cmp, restore)
    } else {
        let Some(save) = (cmp + 1..insns.len()).find(|&j| insns[j].mnemonic == format!("s_and_saveexec_{bits}")) else { return Ok(()) };
        // A different comparison replaced VCC before SAVEEXEC. In straight-line
        // code that save starts an independent masked region.
        if straight_compute && c.dst == [Opnd::VccLo] && insns[save].src == [Opnd::VccLo]
            && insns[cmp + 1..save].iter().any(|i| i.mnemonic.starts_with("v_cmp_") && i.dst == [Opnd::VccLo])
        {
            return Ok(());
        }
        let Some(&Opnd::Sgpr(saved)) = insns[save].dst.first() else { return Ok(()) };
        let restore = (save + 1..insns.len())
            .find(|&j| insns[j].mnemonic == format!("s_mov_{bits}") && insns[j].dst.first() == Some(&Opnd::ExecLo)
                && insns[j].src.first() == Some(&Opnd::Sgpr(saved)))
            .ok_or_else(|| bad("elected branch does not restore EXEC"))?;
        (save, restore)
    };
    for count in save + 1..restore {
        if insns[count].mnemonic != format!("s_bcnt1_i32_{bits}") { continue; }
        if narrows && bits == "b64" && counted_lanes(insns, &targets, cmp, count).is_some() && insns[count].src.first().is_some() {
            insns[count].mnemonic = COUNT_LANES.into();
            continue;
        }
        let Some(&Opnd::Sgpr(mask)) = insns[count].src.first() else { return Err(bad("count of a non-SGPR mask")) };
        let copy = (0..count).rev()
            .find(|&j| insns[j].dst.iter().any(|o| may_touch_sgpr(o, mask)))
            .ok_or_else(|| bad("counted mask has no definition"))?;
        if insns[copy].mnemonic != format!("s_mov_{bits}") || insns[copy].dst != [Opnd::Sgpr(mask)] || insns[copy].src != [Opnd::ExecLo] {
            return Err(bad("counted mask is not an EXEC copy"));
        }
        let entered = (copy + 1..=count).any(|j| targets.contains(&(insns[j].offset as i64)));
        let read = (copy + 1..count).any(|j| sources(&insns[j]).iter().any(|o| may_touch_sgpr(o, mask)));
        // Each half not overwritten by the count must die unread.
        let later_read = |half: u8| (count + 1..insns.len())
            .map(|j| &insns[j])
            .take_while(|ins| !ins.dst.iter().any(|o| *o == Opnd::Sgpr(half)
                || *o == Opnd::Sgpr(half.wrapping_sub(1)) && (ins.mnemonic.ends_with("_b64") || ins.mnemonic.starts_with("v_cmp"))
                || *o == Opnd::Sgpr(half) && ins.mnemonic.starts_with("s_")))
            .any(|ins| reads_sgpr(ins, half));
        let counted_into = |half: u8| insns[count].dst == [Opnd::Sgpr(half)];
        let halves: &[u8] = if bits == "b32" { &[mask] } else { &[mask, mask + 1] };
        let live = halves.iter().any(|&half| !counted_into(half) && later_read(half));
        if entered || read || live {
            return Err(bad(&format!("EXEC copy has other uses (entered {entered}, read {read}, live {live})")));
        }
        insns[copy].mnemonic = if bits == "b32" { "tb_capture_allocation_mask32" } else { "tb_capture_allocation_mask" }.into();
    }
    Ok(())
}

/// The population count of a lane predicate, taken over the wave where it was computed.
pub(in crate::hlsl) const COUNT_LANES: &str = "tb_count_lanes";

/// Where the lane predicate counted at `count` inside an elected branch is computed:
/// before the election at `cmp`, with no branch between, and from active lanes only.
pub(in crate::hlsl) fn counted_lanes(insns: &[Insn], targets: &BTreeSet<i64>, cmp: usize, count: usize) -> Option<usize> {
    let mask = *insns[count].src.first()?;
    let names = |o: &Opnd| match (*o, mask) {
        (Opnd::Sgpr(a), Opnd::Sgpr(b)) => a == b || a + 1 == b || a == b + 1,
        (a, b) => a == b || (b == Opnd::VccLo && a == Opnd::VccHi),
    };
    let def = (0..count).rev().find(|&j| insns[j].dst.iter().any(names))?;
    let d = &insns[def];
    if def >= cmp || d.dst.first() != Some(&mask) { return None; }
    // `v_cmpx` names VCC as a destination it does not write.
    if d.mnemonic.starts_with("v_cmpx") { return None; }
    let active_only = d.mnemonic == "s_mov_b64" && d.src == [Opnd::ExecLo]
        || d.mnemonic == "s_and_b64" && d.src.contains(&Opnd::ExecLo)
        || d.mnemonic.starts_with("v_cmp_");
    let straight = !(def + 1..=cmp).any(|j| targets.contains(&(insns[j].offset as i64))
        || (insns[j].enc == Enc::Sopp && Emitter::branch_cond(insns[j].op).is_some())
        || (j < cmp && insns[j].dst.first() == Some(&Opnd::ExecLo)));
    (active_only && straight).then_some(def)
}
