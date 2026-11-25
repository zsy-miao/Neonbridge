//! Packed-root compute scans use a five-step butterfly within each group of
//! 32 lanes. Wave64 scans additionally broadcast the subtotal from lane 31.
use super::*;

pub(super) struct Scan {
    pub width: u32,
    pub subtotal: std::ops::Range<usize>,
}

// The multi-wave scan reads preceding wave totals from LDS. Both loop branches
// are wave-uniform: every lane compares the same counter against local X / 64.
// Keep the original loop and all of its register writes in the common emitter.
fn subtotal_loop(insns: &[Insn]) -> Option<std::ops::Range<usize>> {
    let back = insns
        .iter()
        .position(|i| i.mnemonic == "s_cbranch_scc1" && i.aux.imm < 0)?;
    let start = insns[..back]
        .iter()
        .rposition(|i| i.mnemonic == "v_cmpx_ne_u32")?
        .checked_sub(1)?;
    let end = back.checked_add(2)?;
    let block: Vec<_> = insns
        .get(start..end)?
        .iter()
        .filter(|i| i.mnemonic != "s_nop")
        .collect();
    let [save, test, skip, copy, live, init, zero, jump, mask, read, offset, inc, address, load, wait, add, snapshot, done, remaining, repeat, restore] =
        block[..]
    else {
        return None;
    };
    let scalar = |i: &Insn| match i.dst[..] {
        [o @ Opnd::Sgpr(_)] => Some(o),
        _ => None,
    };
    let saved = scalar(save)?;
    let active = scalar(live)?;
    let counter = scalar(init)?;
    let carry = scalar(zero)?;
    let scratch = scalar(offset)?;
    let acc = *load.dst.first()?;
    let wave = *test.src.get(1)?;
    let zero_i = Opnd::ConstInt(0);
    let exec = Opnd::ExecLo;
    let is = |i: &Insn, m: &str, d: Opnd, src: &[Opnd]| i.mnemonic == m && i.dst == [d] && i.src == src;
    let jumps =
        |i: &Insn, m: &str, dest: &Insn| i.mnemonic == m && i.offset as i64 + 4 + 4 * i.aux.imm == dest.offset as i64;
    let definition = insns[..start].iter().rposition(|i| i.dst.contains(&wave))?;
    // A lane-local shift of the unmodified local X must reach every invocation.
    // The intervening vector ALU instructions write a single explicit destination.
    if !matches!(wave, Opnd::Vgpr(_))
        || !matches!(acc, Opnd::Vgpr(_))
        || wave == acc
        || !is(
            &insns[definition],
            "v_lshrrev_b32",
            wave,
            &[Opnd::ConstInt(6), Opnd::Vgpr(0)],
        )
        || insns[..definition].iter().any(|i| {
            (i.dst.contains(&Opnd::Vgpr(0)) && i.mnemonic != "v_nop")
                || i.dst.contains(&Opnd::ExecLo)
                || i.dst.contains(&Opnd::ExecHi)
                || i.mnemonic.contains("saveexec")
                || i.mnemonic.starts_with("v_cmpx_")
                || (i.enc == Enc::Sopp && Emitter::branch_cond(i.op).is_some())
        })
        || insns[definition + 1..start].iter().any(|i| {
            !matches!(
                i.mnemonic.as_str(),
                "v_cndmask_b32"
                    | "s_mov_b64"
                    | "v_add_nc_u32"
                    | "v_cmpx_eq_u32"
                    | "v_lshlrev_b32"
                    | "ds_write_b32"
                    | "s_waitcnt"
                    | "s_barrier"
                    | "s_load_dword"
                    | "v_mov_b32"
            )
        })
    {
        return None;
    }
    // The entry skips wave zero and initializes both the counter and the carry.
    if !is(save, "s_mov_b64", saved, &[exec])
        || !is(test, "v_cmpx_ne_u32", Opnd::VccLo, &[zero_i, wave])
        || !jumps(skip, "s_cbranch_execz", restore)
        || !is(copy, "s_mov_b64", scalar(copy)?, &[exec])
        || !is(live, "s_mov_b64", active, &[exec])
        || !is(init, "s_mov_b32", counter, &[zero_i])
        || !is(zero, "s_mov_b32", carry, &[zero_i])
        || !jumps(jump, "s_branch", offset)
    {
        return None;
    }
    // Subsequent iterations accumulate one dword at a wave-uniform LDS address.
    if !is(mask, "s_and_b64", exec, &[scratch, active])
        || !is(read, "v_readfirstlane_b32", carry, &[acc])
        || !is(offset, "s_lshl_b32", scratch, &[counter, Opnd::ConstInt(2)])
        || !is(inc, "s_add_i32", counter, &[counter, Opnd::ConstInt(1)])
        || !is(address, "v_mov_b32", acc, &[scratch])
        || load.mnemonic != "ds_read_b32"
        || load.src.first() != Some(&acc)
        || load.raw[0] & 0x2ffff != 0
        || wait.mnemonic != "s_waitcnt"
        || !is(add, "v_add_nc_u32", acc, &[carry, acc])
    {
        return None;
    }
    // The back edge continues until the counter reaches the current wave.
    if !is(snapshot, "s_mov_b64", scratch, &[exec])
        || !is(done, "v_cmpx_eq_u32", Opnd::VccLo, &[counter, wave])
        || !is(remaining, "s_andn2_b64", active, &[active, exec])
        || !jumps(repeat, "s_cbranch_scc1", mask)
        || !is(restore, "s_mov_b64", exec, &[saved])
    {
        return None;
    }

    let mut regs = BTreeSet::new();
    for lo in [saved, scalar(copy)?, active, scratch] {
        let Opnd::Sgpr(n) = lo else { return None };
        if n % 2 != 0 || n >= 105 || !regs.insert(n) || !regs.insert(n + 1) {
            return None;
        }
    }
    if !regs.insert(sreg_num(&counter)?) || carry != hi(&scratch) {
        return None;
    }

    // An external edge into this region would bypass the initialization above.
    for i in insns[..start].iter().chain(&insns[end..]) {
        let target = i.offset as i64 + 4 + 4 * i.aux.imm;
        if i.enc == Enc::Sopp
            && Emitter::branch_cond(i.op).is_some()
            && (save.offset as i64..=restore.offset as i64).contains(&target)
        {
            return None;
        }
    }
    Some(start..end)
}

fn collective(i: &Insn) -> bool {
    matches!(
        i.mnemonic.as_str(),
        "ds_swizzle_b32"
            | "v_readlane_b32"
            | "s_barrier"
            | "v_mbcnt_lo_u32_b32"
            | "v_mbcnt_hi_u32_b32"
            | "v_readfirstlane_b32"
    ) || i.mnemonic.starts_with("v_cmp_")
}

pub(super) fn prepare(ex: &ShaderExtract, insns: &mut [Insn]) -> Result<Option<Scan>, String> {
    let Some([x, 1, 1]) = ex.psv_threads else {
        return Ok(None);
    };
    if ex.stage != Stage::Compute || !ex.root_sig.as_ref().is_some_and(|r| r.flags == 0x0300_0000) {
        return Ok(None);
    }
    let steps = [0x1e, 0x3c, 0x78, 0xf0, 0x1e0];
    let mut count = 0;
    for i in insns.iter().filter(|i| i.mnemonic == "ds_swizzle_b32") {
        if i.aux.imm != steps[count % steps.len()] || i.raw[0] & (1 << 17) != 0 {
            return Ok(None);
        }
        count += 1;
    }
    if count == 0 || count % steps.len() != 0 {
        return Ok(None);
    }

    let reads_half = insns
        .iter()
        .any(|i| i.mnemonic == "v_readlane_b32" && i.src.get(1) == Some(&Opnd::ConstInt(31)));
    let width = if reads_half { 64 } else { 32 };
    let final_lane = insns
        .iter()
        .any(|i| i.mnemonic == "v_cmp_eq_u32" && i.src == [Opnd::ConstInt(i64::from(width - 1)), Opnd::Vgpr(0)]);
    let upper_count = insns
        .iter()
        .any(|i| i.mnemonic == "v_mbcnt_hi_u32_b32" && i.src.first() == Some(&Opnd::ConstInt(-1)));
    if x == 0 || x % width != 0 || !(final_lane && x == width || width == 64 && upper_count) {
        return Ok(None);
    }

    let subtotal = if width == 64 {
        subtotal_loop(insns).unwrap_or(0..0)
    } else {
        0..0
    };
    let last = insns.iter().rposition(collective).unwrap();
    let mut saved = None;
    for idx in 0..insns.len() {
        if subtotal.contains(&idx) {
            if saved.is_some() {
                return Err("multi-wave subtotal requires full EXEC".into());
            }
            continue;
        }
        let i = &insns[idx];
        if i.enc == Enc::Sopp && Emitter::branch_cond(i.op).is_some() {
            if i.aux.imm < 0 {
                return Err("backward branch in packed wave scan".into());
            }
            if idx <= last {
                let target = i.offset as i64 + 4 + 4 * i.aux.imm;
                let end = insns.partition_point(|j| (j.offset as i64) < target);
                if i.mnemonic != "s_cbranch_execz"
                    || saved.is_none()
                    || !insns.get(end).is_some_and(|j| {
                        j.offset as i64 == target
                            && j.mnemonic == "s_mov_b64"
                            && j.dst == [Opnd::ExecLo]
                            && j.src.first() == saved.as_ref()
                    })
                    || insns[idx + 1..end].iter().any(collective)
                {
                    return Err("non-converged packed wave scan branch".into());
                }
            }
        }
        if idx > last {
            continue;
        }
        if let Some(Opnd::Sgpr(n)) = saved {
            if i.dst.iter().any(|o| *o == Opnd::Sgpr(n) || *o == Opnd::Sgpr(n + 1)) {
                return Err("wave scan clobbers saved EXEC".into());
            }
        }
        if i.mnemonic == "s_mov_b64" && i.dst == [Opnd::ExecLo] && saved.is_some() && i.src.first() == saved.as_ref() {
            saved = None;
        } else if i.mnemonic == "s_and_saveexec_b64" || (i.mnemonic == "s_mov_b64" && i.src == [Opnd::ExecLo]) {
            if saved.is_some() || !matches!(i.dst[..], [Opnd::Sgpr(n)] if n < 105 && n % 2 == 0) {
                return Err("unresolved wave scan EXEC scope".into());
            }
            saved = i.dst.first().copied();
        } else if i.dst.contains(&Opnd::ExecLo)
            || i.dst.contains(&Opnd::ExecHi)
            || i.mnemonic.contains("saveexec")
            || i.mnemonic == "s_endpgm"
            || (i.mnemonic.starts_with("v_cmpx_") && saved.is_none())
            || (saved.is_some() && collective(i))
        {
            return Err("non-converged packed wave scan".into());
        }
        if i.mnemonic == "v_readlane_b32" {
            let ([Opnd::Vgpr(d @ 0..=105)], [Opnd::Vgpr(v), lane, Opnd::Sgpr(0)]) = (&i.dst[..], &i.src[..]) else {
                return Err("invalid scan lane read".into());
            };
            let selector = match lane {
                Opnd::ConstInt(l) if (0..i64::from(width)).contains(l) => 128 + *l as u32,
                Opnd::Sgpr(n) => u32::from(*n),
                _ => return Err("unsupported scan lane selector".into()),
            };
            if i.raw != [0xd7600000 | u32::from(*d), selector << 9 | 256 | u32::from(*v)] {
                return Err("invalid scan lane read encoding".into());
            }
            let d = *d;
            let i = &mut insns[idx];
            i.op = wave_sum::READ_LANE;
            i.dst[0] = Opnd::Sgpr(d);
            i.src.truncate(2);
        }
    }
    Ok(Some(Scan { width, subtotal }))
}

#[cfg(test)]
#[path = "../tests/hlsl/wave_scan.rs"]
mod tests;
