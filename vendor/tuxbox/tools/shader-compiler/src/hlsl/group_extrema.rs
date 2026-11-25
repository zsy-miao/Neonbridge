//! A full wave64 extrema reduction with a saved launch mask. Row permutations
//! fit in physical subgroups of 32. The two broadcasts and the first set EXEC
//! bit use logical workgroup lanes, including on a wave32 host.
use super::{compute_group, wave_sum, Emitter, Enc, Insn, Opnd, ShaderExtract, Stage};
use crate::rdna2::clobbers_scalar;

pub(super) fn is_dpp8(ins: &Insn) -> bool {
    ins.enc == Enc::Vop2
        && ins
            .raw
            .first()
            .is_some_and(|w| matches!(w & 0x1ff, 233 | 234))
}

// DPP8 stores its lane selectors in the same bits that hold DPP16 write masks.
pub(super) fn destination_mask(ins: &Insn) -> Option<u8> {
    if ins.enc != Enc::Vop2 || ins.raw.first()? & 0x1ff != 250 {
        return None;
    }
    let mask = (ins.raw.get(1)? >> 24) as u8;
    (mask != 0xff).then_some(mask)
}

// Choose the same logical width as the program's lane broadcasts. The physical
// subgroup used to fetch the source operand may contain only half a guest wave.
pub(super) fn row_index_mask(em: &Emitter<'_>, rows: u8, banks: u8) -> Result<u32, String> {
    if (rows & 3) == (rows >> 2) || banks == 0 {
        return Ok(3); // The effective write mask repeats in both halves.
    }
    let register_width = em.packed.as_ref().and_then(|p| p.register_wave_size);
    let threads = compute_group(em.ex).into_iter().fold(1u32, u32::saturating_mul);
    match em.fspx_wave.or(register_width) {
        Some(32) => Ok(1),
        Some(64) => Ok(3),
        _ if threads <= 32 || em.resolve_wave32 || em.single_wave32 => Ok(1),
        _ if em.indexed_lane => Ok(3),
        _ => Err("asymmetric DPP16 row mask requires a known guest wave width".into()),
    }
}

fn wider_exchange(ins: &Insn) -> bool {
    if ins.enc != Enc::Vop2 || !matches!(ins.mnemonic.as_str(), "v_min_f32" | "v_max_f32") {
        return false;
    }
    if is_dpp8(ins) {
        return ins.raw.len() == 2;
    }
    // DPP16 rotation, mirror or quad with destination masks or source modifiers.
    ins.src.first() == Some(&Opnd::Raw(250))
        && ins.raw.get(1).is_some_and(|c| {
            let control = (c >> 8) & 0x1ff;
            c & 0x0002_0000 == 0
                && (matches!(control, 0x121..=0x12f | 0x140)
                    || (control <= 0xff && (destination_mask(ins).is_some() || c & 0x00f0_0000 != 0)))
        })
}

fn supported_step(ins: &Insn, op: &str) -> bool {
    if matches!(
        ins.mnemonic.as_str(),
        "v_permlanex16_b32" | "v_readlane_b32"
    ) {
        return true; // The existing lane exchange emitters validate the operands.
    }
    ins.enc == Enc::Vop2
        && ins.mnemonic == op
        && (wider_exchange(ins)
            || super::is_binary_quad_perm(ins)
            || matches!(
                &ins.src[..],
                [
                    Opnd::Sgpr(_)
                        | Opnd::Vgpr(_)
                        | Opnd::ConstInt(_)
                        | Opnd::ConstF32(_)
                        | Opnd::Literal(_),
                    Opnd::Vgpr(_)
                ]
            ))
}

// Keep the native reduction tree and intermediate registers intact. Its READLANE
// instructions must still address the logical wave, including the other 32 lanes.
pub(super) fn prepare_exchanges(ex: &ShaderExtract, code: &mut [Insn]) -> Result<bool, String> {
    if ex.stage != Stage::Compute {
        return Ok(false);
    }
    let mut changed = false;
    let mut targets = None;
    let mut pos = 0;
    while pos + 3 < code.len() {
        let save = &code[pos];
        pos += 1;
        if save.mnemonic != "s_nand_saveexec_b64" || save.src != [Opnd::ConstInt(0)] {
            continue;
        }
        let [Opnd::Sgpr(saved)] = save.dst[..] else {
            continue;
        };
        let [identity, select, first, ..] = &code[pos..] else {
            break;
        };
        if identity.mnemonic != "v_mov_b32"
            || select.mnemonic != "v_cndmask_b32"
            || select.src.get(2) != Some(&Opnd::Sgpr(saved))
            || !matches!(first.mnemonic.as_str(), "v_min_f32" | "v_max_f32")
        {
            continue;
        }
        let body = &code[pos + 2..];
        let count = body
            .iter()
            .take_while(|i| supported_step(i, &first.mnemonic))
            .count();
        let Some(restore) = body.get(count) else {
            continue;
        };
        if restore.mnemonic != "s_mov_b64"
            || restore.dst != [Opnd::ExecLo]
            || restore.src != [Opnd::Sgpr(saved)]
            || !body[..count].iter().any(wider_exchange)
        {
            continue;
        }
        // FI=0 on DPP8 relies on NAND_SAVEEXEC having enabled every source lane.
        let targets = targets.get_or_insert_with(|| super::branch_targets(code));
        if targets
            .range((save.offset as i64 + 1)..(restore.offset as i64))
            .next()
            .is_some()
        {
            return Err("branch enters a saved-EXEC extrema exchange".into());
        }
        pos += 2;
        for ins in &mut code[pos..pos + count] {
            if ins.enc == Enc::Vop2 {
                if matches!(ins.src.first(), Some(Opnd::Raw(233 | 234 | 250))) {
                    super::lower_dpp_binary(ins);
                } else {
                    ins.op = super::OP_LOCAL_EXTREMA;
                }
            }
        }
        pos += count;
        changed = true;
    }
    Ok(changed)
}

pub(super) fn prepare(ex: &ShaderExtract, code: &mut [Insn]) -> bool {
    if ex.stage != Stage::Compute
        || !compute_group(ex)
            .into_iter()
            .try_fold(1u32, u32::checked_mul)
            .is_some_and(|n| n > 0 && n <= 1024 && n % 64 == 0)
        || tuxbox_dxbc::fssm::compute_lds_bytes(&ex.fssm).is_none()
    {
        return false;
    }
    let Some(first) = code.iter().position(|i| {
        matches!(i.mnemonic.as_str(), "v_min_f32" | "v_max_f32")
            && i.src.first() == Some(&Opnd::Raw(233))
    }) else {
        return false;
    };
    let Some(prologue) = first.checked_sub(5) else {
        return false;
    };
    let Some(tail) = code.get(first..first + 10) else {
        return false;
    };
    let [save, mov, and, wait, select] = &code[prologue..first] else {
        return false;
    };
    let [Opnd::Sgpr(saved)] = save.dst[..] else {
        return false;
    };
    let [Opnd::Sgpr(entry), Opnd::Sgpr(s)] = and.src[..] else {
        return false;
    };
    if save.mnemonic != "s_or_saveexec_b64"
        || s != saved
        || and.mnemonic != "s_and_b64"
        || wait.mnemonic != "s_waitcnt"
        || select.src.get(2..) != Some(and.dst.as_slice())
    {
        return false;
    }
    let Some(capture) = code[..prologue].iter().position(|i| {
        i.mnemonic == "s_mov_b64" && i.dst == [Opnd::Sgpr(entry)] && i.src == [Opnd::ExecLo]
    }) else {
        return false;
    };
    if code[..capture].iter().any(|i| {
        i.mnemonic.contains("branch")
            || i.mnemonic.contains("exec")
            || i.mnemonic.starts_with("v_cmpx")
            || i.dst.contains(&Opnd::ExecLo)
    }) || code[capture + 1..]
        .iter()
        .any(|i| clobbers_scalar(i, entry) || clobbers_scalar(i, entry + 1))
    {
        return false;
    }
    if code.iter().enumerate().any(|(j, i)| {
        let name = i.mnemonic.as_str();
        matches!(
            name,
            "s_setpc_b64" | "s_swappc_b64" | "s_call_b64" | "v_writelane_b32"
        ) || (name == "s_endpgm" && j + 1 != code.len())
            || (name.contains("branch")
                && (i.enc != Enc::Sopp
                    || Emitter::branch_cond(i.op).is_none()
                    || i.aux.imm < 0
                    || j < first + 10))
    }) {
        return false;
    }
    // The launch mask is all ones in a complete wave, so the explicit AND
    // selects the saved EXEC. Verify the remaining native sequence unchanged
    // with the ordinary reduction decoder, then retain the actual prologue.
    let mut seq = vec![save.clone(), mov.clone(), select.clone()];
    seq[2].src[2] = Opnd::Sgpr(saved);
    seq.extend_from_slice(tail);
    if !matches!(wave_sum::lower(&mut seq, 3), Ok((64, 13))) {
        return false;
    }
    let mut elections = Vec::new();
    for (j, i) in code
        .iter()
        .enumerate()
        .filter(|(_, i)| i.mnemonic == "s_ff1_i32_b64")
    {
        let Some(prev) = j.checked_sub(1).and_then(|j| code.get(j)) else {
            return false;
        };
        if prev.mnemonic != "s_and_b64"
            || prev.src != [Opnd::Sgpr(entry), Opnd::ExecLo]
            || prev.dst != i.src
        {
            return false;
        }
        elections.push(j);
    }
    code[first..first + 10].clone_from_slice(&seq[3..]);
    for i in &mut code[first..first + 10] {
        if i.op == wave_sum::READ_LANE {
            i.mnemonic = "tb_indexed_readlane".into();
            i.enc = Enc::Sop1;
        }
    }
    for j in elections {
        code[j].mnemonic = "tb_first_exec_lane".into();
    }
    true
}
