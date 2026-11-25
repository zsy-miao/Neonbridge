//! A full wave64 extrema reduction with a saved launch mask. Row permutations
//! fit in physical subgroups of 32. The two broadcasts and the first set EXEC
//! bit use logical workgroup lanes, including on a wave32 host.
use super::{compute_group, wave_sum, Emitter, Enc, Insn, Opnd, ShaderExtract, Stage};
use crate::rdna2::clobbers_scalar;

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
