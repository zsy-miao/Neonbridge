//! Halfword lookups in constant tables appended to the shader executable.
//! Prove that the vector address comes from a scalar PC-relative base plus
//! a scaled element index before addressing the HLSL constant array.
use super::{Emitter, Enc, Insn, Opnd};
use crate::rdna2::clobbers_scalar;
use std::collections::BTreeSet;

pub(super) fn halfword(insns: &[Insn], load: usize, code: usize, end: usize) -> Option<()> {
    let ins = &insns[load];
    let [Opnd::Vgpr(v), _, Opnd::Null] = *ins.src.as_slice() else {
        return None;
    };
    if !matches!(ins.aux.mask, 0 | 2) || ins.aux.imm != 0 {
        return None;
    }
    let mut incoming = vec![Vec::new(); insns.len()];
    for (i, op) in insns.iter().enumerate() {
        if matches!(
            op.mnemonic.as_str(),
            "s_setpc_b64" | "s_swappc_b64" | "s_call_b64"
        ) {
            return None;
        }
        let branch = op.enc == Enc::Sopp && Emitter::branch_cond(op.op).is_some();
        if branch {
            let target = op.offset as i64 + 4 + op.aux.imm.checked_mul(4)?;
            let target = insns
                .binary_search_by_key(&usize::try_from(target).ok()?, |i| i.offset)
                .ok()?;
            incoming[target].push(i);
        }
        if i + 1 < insns.len() && !(op.enc == Enc::Sopp && matches!(op.op, 1 | 2)) {
            incoming[i + 1].push(i);
        }
    }

    // Vector address halves are adjacent moves. Scalar arithmetic between
    // their materialization and the load cannot change either VGPR.
    let mut move_hi = load.checked_sub(1)?;
    while matches!(
        insns[move_hi].enc,
        Enc::Sop1 | Enc::Sop2 | Enc::Sopk | Enc::Sopc
    ) || (insns[move_hi].mnemonic == "v_cvt_f32_i32"
        && matches!(insns[move_hi].dst.as_slice(), [Opnd::Vgpr(r)] if *r != v && Some(*r) != v.checked_add(1)))
    {
        move_hi = move_hi.checked_sub(1)?;
    }
    let move_lo = move_hi.checked_sub(1)?;
    let mov = |i: usize, v| {
        let op = &insns[i];
        match (op.mnemonic.as_str(), op.dst.as_slice(), op.src.as_slice()) {
            ("v_mov_b32", [Opnd::Vgpr(dst)], [Opnd::Sgpr(src)]) if *dst == v => Some(*src),
            _ => None,
        }
    };
    let lo = mov(move_lo, v)?;
    if mov(move_hi, v.checked_add(1)?)? != lo.checked_add(1)?
        || (move_lo + 1..=load).any(|i| incoming[i] != [i - 1])
    {
        return None;
    }

    // All paths, including loop backedges, must reach the same definition.
    let definition = |use_at: usize, reg: u8| -> Option<usize> {
        let mut seen = BTreeSet::new();
        let mut pending = incoming[use_at].clone();
        let mut found = None;
        while let Some(i) = pending.pop() {
            if !seen.insert(i) {
                continue;
            }
            if clobbers_scalar(&insns[i], reg) {
                if found.replace(i).is_some_and(|prev| prev != i) {
                    return None;
                }
            } else if incoming[i].is_empty() {
                return None;
            } else {
                pending.extend_from_slice(&incoming[i]);
            }
        }
        found
    };
    let add = definition(move_lo, lo)?;
    let carry = definition(move_hi, lo + 1)?;
    let a = &insns[add];
    let h = &insns[carry];
    let [Opnd::Sgpr(index), Opnd::Sgpr(base)] = *a.src.as_slice() else {
        return None;
    };
    if a.mnemonic != "s_add_u32"
        || h.mnemonic != "s_addc_u32"
        || carry != add + 1
        || incoming[carry] != [add]
        || h.src
            != [
                Opnd::Sgpr(index.checked_add(1)?),
                Opnd::Sgpr(base.checked_add(1)?),
            ]
    {
        return None;
    }
    let scaled = &insns[definition(add, index)?];
    if scaled.mnemonic != "s_lshl_b64"
        || scaled.dst != [Opnd::Sgpr(index)]
        || scaled.src.get(1) != Some(&Opnd::ConstInt(1))
    {
        return None;
    }
    let plus = definition(add, base)?;
    let plus_hi = definition(add, base + 1)?;
    let get = plus.checked_sub(1)?;
    let pc = &insns[get];
    let bias = &insns[plus];
    let carry = &insns[plus_hi];
    let b = Opnd::Sgpr(base);
    let bh = Opnd::Sgpr(base + 1);
    if plus_hi != plus + 1
        || incoming[plus] != [get]
        || incoming[plus_hi] != [plus]
        || pc.mnemonic != "s_getpc_b64"
        || pc.dst != [b]
        || bias.mnemonic != "s_add_u32"
        || bias.dst != [b]
        || bias.src.first() != Some(&b)
        || carry.mnemonic != "s_addc_u32"
        || carry.dst != [bh]
        || carry.src != [bh, Opnd::ConstInt(0)]
    {
        return None;
    }
    let disp = match bias.src.get(1)? {
        Opnd::Literal(bits) => *bits as i32 as i64,
        Opnd::ConstInt(i) => *i,
        _ => return None,
    };
    let start = (pc.offset as i64 + 4).checked_add(disp)?;
    let start = usize::try_from(start).ok()?;
    (start >= code && start.checked_add(2)? <= end && start % 2 == 0).then_some(())
}
