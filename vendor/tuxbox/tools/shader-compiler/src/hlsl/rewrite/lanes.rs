//! Lane reads, lane spills, local jumps and wave allocations.
use super::*;

/// Translator temporaries from this index up are lane-spill slots (declared up front).
pub(in crate::hlsl) const LANE_TEMP_BASE: u16 = 10_000;

/// SGPR spills through vector lanes.
///
/// When a shader runs out of scalar registers the console compiler parks scalars (observed:
/// texture/sampler descriptor dwords) in individual lanes of an otherwise unused VGPR with
/// `v_writelane_b32 vN, sX, <lane>` and brings them back with `v_readlane_b32 sY, vN, <lane>`
/// (VOP3 0x361 / 0x360; fixture `fortnite_lobby_desc.dxbc`). The translator models one lane
/// per VGPR, so each (VGPR, constant lane) slot is lowered to a function-scope translator
/// temporary and the two instructions become plain `s_mov_b32`. The descriptor dataflow
/// tracks those temporaries like registers (keys 1000+), so descriptors survive a spill.
///
/// Refused (explicit error): a non-constant lane index, a read of a slot that was never
/// written, or a spill VGPR that is also used as an ordinary vector.
/// A read of one lane of the caller's 64-lane block, exchanged like `ds_swizzle_b32`.
pub(in crate::hlsl) const GROUP_READ: &str = "tb_group_readlane";

pub(in crate::hlsl) fn exchanges(i: &Insn) -> bool { i.mnemonic == "ds_swizzle_b32" || i.mnemonic == GROUP_READ }

/// A pixel is a wave of its own, so the first lane of a mask that covers it is its own.
/// `s_ff1` names that lane for `v_readlane`; anything else that uses the lane is left alone.
pub(in crate::hlsl) fn lower_first_lane_reads(insns: &mut [Insn]) {
    for i in 0..insns.len() {
        if insns[i].mnemonic != "v_readlane_b32" { continue; }
        let Some(&Opnd::Sgpr(lane)) = insns[i].src.get(1) else { continue };
        let Some(first) = (i.saturating_sub(64)..i).rev()
            .find(|&j| insns[j].dst.contains(&Opnd::Sgpr(lane)) || insns[j].enc == Enc::Sopp && insns[j].mnemonic.contains("branch"))
        else { continue };
        if !insns[first].mnemonic.starts_with("s_ff1_i32_b") { continue; }
        // The destination field names an SGPR (decoded generically as a VGPR).
        let Some(&(Opnd::Vgpr(result) | Opnd::Sgpr(result))) = insns[i].dst.first() else { continue };
        let reads = |j: usize| j != i && insns[j].src.contains(&Opnd::Sgpr(lane));
        if (first + 1..i).any(reads) { continue; }
        if result != lane {
            let end = (i + 1..insns.len()).find(|&j| insns[j].dst.contains(&Opnd::Sgpr(lane)) || insns[j].mnemonic.contains("branch"));
            if end.is_none_or(|end| (i + 1..=end).any(reads) || insns[end].mnemonic.contains("branch")) { continue; }
        }
        insns[first].mnemonic = "s_mov_b32".into();
        insns[first].op = 3;
        insns[first].src = vec![Opnd::ConstInt(0)];
        let ins = &mut insns[i];
        ins.enc = Enc::Vop1;
        ins.op = 2;
        ins.mnemonic = "v_readfirstlane_b32".into();
        ins.dst = vec![Opnd::Sgpr(result)];
        ins.src.truncate(1);
    }
}

pub(in crate::hlsl) fn lower_lane_spills(insns: &mut [Insn], wave32: bool, group_reads: bool) -> Result<(), String> {
    const READLANE: u32 = 0x360;
    const WRITELANE: u32 = 0x361;
    let is_lane = |i: &Insn| i.enc == Enc::Vop3 && (i.op == READLANE || i.op == WRITELANE);
    if !insns.iter().any(is_lane) {
        return Ok(());
    }
    let mut slots: BTreeMap<(u8, i64), Opnd> = BTreeMap::new();
    let mut spill_vregs: BTreeSet<u8> = BTreeSet::new();
    let written_vregs: BTreeSet<_> = insns.iter()
        .filter(|ins| wave32 && ins.enc == Enc::Vop3 && ins.op == WRITELANE)
        .filter_map(|ins| match ins.dst.first() {
            Some(Opnd::Vgpr(v)) => Some(*v),
            _ => None,
        })
        .collect();
    let any_written: BTreeSet<_> = insns.iter()
        .filter(|ins| ins.enc == Enc::Vop3 && ins.op == WRITELANE)
        .filter_map(|ins| match ins.dst.first() {
            Some(Opnd::Vgpr(v)) => Some(*v),
            _ => None,
        })
        .collect();
    for ins in insns.iter_mut() {
        if !is_lane(ins) {
            continue;
        }
        let at = ins.offset;
        if wave32 && ins.op == READLANE {
            if let ([Opnd::Vgpr(d) | Opnd::Sgpr(d)], [Opnd::Vgpr(v), lane, ..]) =
                (ins.dst.as_slice(), ins.src.as_slice())
            {
                if !written_vregs.contains(v) {
                    ins.dst = vec![Opnd::Sgpr(*d)];
                    ins.src = vec![Opnd::Vgpr(*v), *lane];
                    ins.enc = Enc::Sop1;
                    ins.mnemonic = ray_resolve::READ_LANE.into();
                    continue;
                }
            }
        }
        // A uniform Sgpr lane operand is only expressible through the whole-wave
        // group read; anything else stays refused. Spill areas (vregs written
        // by v_writelane) are still slotted per constant lane, so they remain
        // refused for dynamic lanes.
        if !wave32 && group_reads && ins.op == READLANE {
            let converted = match (ins.src.first(), ins.src.get(1), ins.dst.first()) {
                (Some(Opnd::Vgpr(v)), Some(Opnd::Sgpr(_)),
                 Some(Opnd::Vgpr(_)) | Some(Opnd::Sgpr(_)))
                    if !any_written.contains(v) =>
                {
                    let v = *v;
                    let d = match ins.dst.first() {
                        Some(Opnd::Vgpr(n)) | Some(Opnd::Sgpr(n)) => *n,
                        _ => unreachable!(),
                    };
                    ins.dst = vec![Opnd::Sgpr(d)];
                    ins.src = vec![Opnd::Vgpr(v), ins.src[1].clone()];
                    ins.enc = Enc::Sop1;
                    ins.mnemonic = GROUP_READ.into();
                    true
                }
                _ => false,
            };
            if converted {
                continue;
            }
        }
        let lane = match ins.src.get(1) {
            Some(Opnd::ConstInt(l)) if (0..64).contains(l) => *l,
            other => return Err(format!("lane op at {at:#x}: non-constant lane operand {other:?}")),
        };
        if ins.op == WRITELANE {
            let Some(Opnd::Vgpr(v)) = ins.dst.first().cloned() else {
                return Err(format!("v_writelane at {at:#x}: destination is not a VGPR"));
            };
            let next = slots.len();
            let slot = match slots.get(&(v, lane)) {
                Some(s) => *s,
                None => {
                    let s = Opnd::Temp(LANE_TEMP_BASE + next as u16);
                    slots.insert((v, lane), s);
                    s
                }
            };
            spill_vregs.insert(v);
            let value = ins.src.first().cloned().ok_or_else(|| format!("v_writelane at {at:#x}: no source"))?;
            ins.dst = vec![slot];
            ins.src = vec![value];
        } else {
            // The destination field of v_readlane names an SGPR (decoded generically as a VGPR).
            let d = match ins.dst.first() {
                Some(Opnd::Vgpr(n)) | Some(Opnd::Sgpr(n)) => *n,
                other => return Err(format!("v_readlane at {at:#x}: unexpected destination {other:?}")),
            };
            let Some(Opnd::Vgpr(v)) = ins.src.first().cloned() else {
                return Err(format!("v_readlane at {at:#x}: source is not a VGPR"));
            };
            if group_reads && !any_written.contains(&v) {
                ins.dst = vec![Opnd::Sgpr(d)];
                ins.src = vec![Opnd::Vgpr(v), Opnd::ConstInt(lane)];
                ins.enc = Enc::Sop1;
                ins.mnemonic = GROUP_READ.into();
                continue;
            }
            let Some(&slot) = slots.get(&(v, lane)) else {
                return Err(format!("v_readlane at {at:#x}: v{v} lane {lane} read before any v_writelane"));
            };
            ins.dst = vec![Opnd::Sgpr(d)];
            ins.src = vec![slot];
        }
        ins.enc = Enc::Sop1;
        ins.op = 0x03;
        ins.mnemonic = "s_mov_b32".into();
    }
    for ins in insns.iter() {
        if ins.mnemonic == "v_nop" {
            continue; // the two header words at the start of every program decode as v_nop
        }
        for o in ins.dst.iter().chain(ins.src.iter()) {
            if let Opnd::Vgpr(v) = o {
                if spill_vregs.contains(v) {
                    return Err(format!("v{v} is used both as a lane spill area and as a vector at {:#x}", ins.offset));
                }
            }
        }
    }
    Ok(())
}

/// Resolve local PC transfers when GETPC and a constant add form one basic block.
pub(in crate::hlsl) fn lower_local_pc_transfers(insns: &mut [Insn]) -> Result<(), String> {
    if !insns
        .iter()
        .any(|i| matches!(i.mnemonic.as_str(), "s_setpc_b64" | "s_swappc_b64"))
    {
        return Ok(());
    }
    let mut entries: BTreeSet<i64> = insns
        .iter()
        .filter(|i| i.enc == Enc::Sopp && Emitter::branch_cond(i.op).is_some())
        .map(|i| i.offset as i64 + 4 + 4 * i.aux.imm)
        .collect();
    let mut local = Vec::new();
    for (i, seq) in insns.windows(3).enumerate() {
        let [pc, add, jump] = seq else { unreachable!() };
        if !matches!(jump.mnemonic.as_str(), "s_setpc_b64" | "s_swappc_b64")
            || pc.mnemonic != "s_getpc_b64"
            || add.mnemonic != "s_add_u32"
            || add.dst.first() != pc.dst.first()
            || add.src.first() != pc.dst.first()
            || jump.src.first() != pc.dst.first()
        {
            continue;
        }
        let offset = match add.src.get(1) {
            Some(Opnd::ConstInt(k)) => *k,
            Some(Opnd::Literal(k)) => i64::from(*k as i32),
            _ => continue,
        };
        let target = pc.offset as i64 + 4 + offset;
        if !insns.iter().any(|x| x.offset as i64 == target) {
            return Err(format!(
                "PC transfer at {:#x} targets outside the decoded instructions",
                jump.offset
            ));
        }
        entries.insert(target);
        local.push((i, target));
    }
    for (i, target) in local {
        if entries.contains(&(insns[i + 1].offset as i64)) || entries.contains(&(insns[i + 2].offset as i64)) {
            return Err("branch enters a local PC-transfer sequence after GETPC".into());
        }
        let jump = &mut insns[i + 2];
        jump.enc = Enc::Sopp;
        jump.op = 2;
        jump.aux.imm = (target - jump.offset as i64 - 4) / 4;
        if jump.mnemonic == "s_setpc_b64" {
            jump.dst.clear();
        }
    }
    Ok(())
}

// These instruction sequences elect the lowest active lane to reserve a contiguous
// range, restore EXEC, broadcast the atomic result and add a per-lane prefix.
// The two register allocations differ. Match the complete regions before changing
// a saved EXEC mask from its usual per-invocation predicate into wave-uniform data.
pub(in crate::hlsl) fn mark_wave_allocations(insns: &mut [Insn], wave32_mask: bool, election: Option<ElectionLowering>) -> Result<bool, String> {
    const REGIONS: [&[u32]; 2] = [
        &[
        0xd7650007, 0x000100c1, 0xbe8e147e, 0xbe8c047e, 0xd7660007, 0x00020ec1,
        0xd482000e, 0x00020e0e, 0x7e0e0280, 0xbe8e240e, 0xbf880005, 0xbf800000,
        0xbe8c100c, 0x7e0e020c, 0xe0c84000, 0x80070700, 0xbefe040e, 0xd7650008,
        0x0001007e, 0xbf8c3f70, 0x7e180507, 0xd7660007, 0x0002107f, 0x4a0e0e0c
        ],
        &[
        0xd7650007, 0x000100c1, 0xbe83147e, 0xbe8e047e, 0xd7660007, 0x00020ec1,
        0xd4820012, 0x00020e03, 0x7e0e0280, 0xbe922412, 0xbf880005, 0xbf800000,
        0xbe83100e, 0x7e0e0203, 0xe0c84000, 0x80070700, 0xbefe0412, 0xd7650008,
        0x0001007e, 0xbf8c3f70, 0x7e060507, 0xd7660007, 0x0002107f, 0x4a0e0e03
        ],
    ];
    const COUNT: usize = 18;
    let snapshots = election.is_some() && predicate_snapshots(insns);
    let mut found = false;
    for i in 0..insns.len() {
        // Wave32 programs keep the previous lowering unless the election verifies.
        // Modes that model EXEC as a whole-wave mask already lower it directly.
        if !wave32_mask && insns[i].mnemonic == "s_ff1_i32_b32" && insns[i].src == [Opnd::ExecLo] {
            let mut trial = insns.to_vec();
            match verify_lane_election(&mut trial, i, election.is_some()) {
                Ok(()) => {
                    trial[i].mnemonic = "tb_first_active_lane".into();
                    insns.clone_from_slice(&trial);
                }
                Err(e) if std::env::var_os("TB_DFDEBUG").is_some() => eprintln!("{e}"),
                Err(_) => {}
            }
            continue;
        }
        if insns[i].mnemonic != "s_ff1_i32_b64" || insns[i].src != [Opnd::ExecLo] { continue; }
        let region = i.checked_sub(1).and_then(|first| insns.get(first..first + COUNT))
            .filter(|region| REGIONS.iter().any(|words| region.iter().flat_map(|op| op.raw.iter()).eq(words.iter())));
        let Some(region) = region else {
            if !snapshots {
                verify_lane_election(insns, i, election.is_some())?;
            }
            if election == Some(ElectionLowering::Shared64) { insns[i].mnemonic = "tb_first_exec_lane".into(); }
            continue;
        };
        let first = region[0].offset;
        let end = region[COUNT - 1].offset + 4;
        for branch in insns.iter().filter(|b| b.enc == Enc::Sopp && Emitter::branch_cond(b.op).is_some()) {
            let target = branch.offset as i64 + 4 + 4 * branch.aux.imm;
            if !(first..end).contains(&branch.offset) && target > first as i64 && target < end as i64 {
                return Err("external branch enters wave allocation".into());
            }
        }
        insns[i + 1].mnemonic = "tb_capture_allocation_mask".into();
        insns[i + 12].mnemonic = "tb_allocation_prefix_lo".into();
        insns[i + 15].mnemonic = "tb_allocation_prefix_hi".into();
        found = true;
    }
    Ok(found)
}
