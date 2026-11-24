//! Fixed-width DPP sums, extrema and prefix scans. Keep intermediate
//! register writes and the restored EXEC observable after each reduction.
use super::{Emitter, Enc, Insn, Opnd};

pub(super) const DPP_REDUCE: u32 = 0x7ff1;
pub(super) const READ_LANE: u32 = 0x7ff2;

// Reductions may have a completed vector load and a second predicate.
// The scalar AND intersects that predicate with the saved EXEC before the
// CNDMASK sets excluded lanes to the reduction's identity.
fn masked_prologue(insns: &[Insn], first: usize, identity: i64) -> Option<(usize, Opnd)> {
    let select = insns.get(first.checked_sub(1)?)?;
    let [Opnd::ConstInt(value), Opnd::Vgpr(_), mask @ Opnd::Sgpr(_)] = select.src[..] else {
        return None;
    };
    if value != identity {
        return None;
    }
    let mut mask_sources = None;
    for (index, op) in insns[..first - 1].iter().enumerate().rev() {
        match op.mnemonic.as_str() {
            "s_waitcnt" | "v_nop" => (),
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
                return matches.then_some((index, mask));
            }
            _ => return None,
        }
    }
    None
}

fn masked_identity(operation: &str) -> Option<i64> {
    match operation {
        "v_or_b32" | "v_max_u32" => Some(0),
        "v_and_b32" | "v_min_u32" => Some(-1),
        _ => None,
    }
}

pub(super) fn lower(insns: &mut [Insn], first: usize) -> Result<(u32, usize), &'static str> {
    let start = first.checked_sub(2).ok_or("missing reduction prologue")?;
    let tail = &insns[start..];
    let operation = &insns[first].mnemonic;
    let floating = matches!(operation.as_str(), "v_min_f32" | "v_max_f32");
    // The floating form inserts the neutral value between SAVEEXEC and CNDMASK.
    let identity = masked_identity(operation);
    let masked = identity
        .map(|neutral| {
            masked_prologue(insns, first, neutral).ok_or("unrecognized masked DPP prologue")
        })
        .transpose()?;
    let save = if let Some((index, _)) = masked {
        &insns[index]
    } else if floating {
        let prologue = start.checked_sub(1).ok_or("missing extrema prologue")?;
        &insns[prologue]
    } else {
        &tail[0]
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
    let neutral = if floating {
        let identity = if operation == "v_min_f32" {
            f32::INFINITY
        } else {
            f32::NEG_INFINITY
        };
        if tail[0].mnemonic != "v_mov_b32" || tail[0].src != [Opnd::Literal(identity.to_bits())] {
            return Err("unexpected extrema identity");
        }
        let [reg @ Opnd::Vgpr(_)] = tail[0].dst[..] else {
            return Err("invalid identity register");
        };
        reg
    } else {
        Opnd::ConstInt(identity.unwrap_or(0))
    };
    if save.src != [Opnd::ConstInt(-1)]
        || block[1].mnemonic != "v_cndmask_b32"
        || !matches!(block[1].src[..], [identity, Opnd::Vgpr(_), mask] if mask == masked.map_or(Opnd::Sgpr(saved), |(_, mask)| mask) && identity == neutral)
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

/// The branch emitter predicates regions per host invocation. HLSL lane reads
/// require the source invocation to participate, including lane zero when guest
/// EXEC has masked it. A branch around or into the region can violate that.
pub(super) fn check_control_flow(insns: &[Insn], width: u32) -> Result<(), String> {
    for (first, ins) in insns.iter().enumerate() {
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
                .ok_or("unrecognized masked DPP prologue")?
                .0
        } else {
            first - prologue_len
        };
        let start = insns[start].offset as i64;
        let end = insns[first + if width == 32 { 7 } else { 9 }].offset as i64;
        for branch in insns.iter().filter(|i| i.enc == Enc::Sopp) {
            let pc = branch.offset as i64;
            if branch.op == 1 && pc < start {
                return Err(format!("wave reduction at {start:#x} after an early exit"));
            }
            if Emitter::branch_cond(branch.op).is_some() {
                let target = pc + 4 + 4 * branch.aux.imm;
                if (pc < start && target > start) || (pc > end && target <= end) {
                    return Err(format!(
                        "wave reduction at {start:#x} may execute in divergent control flow"
                    ));
                }
            }
        }
    }
    Ok(())
}
