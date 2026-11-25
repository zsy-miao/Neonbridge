use crate::rdna2::{Enc, Insn, Opnd};
use std::collections::BTreeSet;

pub(super) fn branch_targets(insns: &[Insn]) -> BTreeSet<usize> {
    insns
        .iter()
        .filter(|ins| is_branch(ins))
        .filter_map(|ins| usize::try_from(ins.offset as i64 + 4 + ins.aux.imm * 4).ok())
        .collect()
}

fn is_branch(ins: &Insn) -> bool {
    ins.mnemonic == "s_branch" || ins.mnemonic.starts_with("s_cbranch_")
}

fn indirect_transfer(ins: &Insn) -> bool {
    matches!(
        ins.mnemonic.as_str(),
        "s_endpgm" | "s_setpc_b64" | "s_swappc_b64" | "s_rfe_b64" | "s_call_b64"
    )
}

fn bits(operand: &Opnd) -> Option<u32> {
    match *operand {
        Opnd::ConstInt(value) => u32::try_from(value).ok(),
        Opnd::Literal(value) => Some(value),
        _ => None,
    }
}

// Recognize a packed integer sampled without filtering. The shader moves one
// component into an SGPR, range-checks it and extracts an interior bit field.
// Read-first-lane and a comparison alone also occur on floating-point bits.
pub(super) fn packed_uint(load: &Insn, tail: &[Insn], targets: &BTreeSet<usize>) -> bool {
    let [Opnd::Vgpr(value)] = load.dst.as_slice() else {
        return false;
    };
    if (load.aux.mask & 15).count_ones() != 1 || load.raw.get(1).is_some_and(|word| word >> 31 != 0)
    {
        return false;
    }
    let mut next = tail
        .iter()
        .take_while(|ins| !targets.contains(&ins.offset) && !indirect_transfer(ins));
    let scalar = loop {
        let Some(ins) = next.next() else { return false };
        if ins.mnemonic == "v_readfirstlane_b32" && ins.src.as_slice() == [Opnd::Vgpr(*value)] {
            let [Opnd::Sgpr(reg)] = ins.dst.as_slice() else {
                return false;
            };
            break *reg;
        }
        // Only ordinary ALU and waits may intervene. Memory writes to VGPR
        // ranges and EXEC transitions can invalidate the selected lane's value.
        let simple = matches!(
            ins.enc,
            Enc::Vop1
                | Enc::Vop2
                | Enc::Vop3
                | Enc::Vopc
                | Enc::Sop1
                | Enc::Sop2
                | Enc::Sopc
                | Enc::Sopk
        ) || matches!(ins.mnemonic.as_str(), "s_waitcnt" | "s_nop");
        if !simple || is_branch(ins) || ins.mnemonic.contains("exec") || ins.mnemonic.starts_with("v_cmpx")
            || ins.dst.iter().any(|d| matches!(d, Opnd::ExecHi | Opnd::ExecLo))
            || ins.src.contains(&Opnd::Vgpr(*value))
            || ins.dst.iter().any(|dst| matches!(dst, Opnd::Vgpr(first) if *first == *value || (ins.mnemonic.contains("64") && first.checked_add(1) == Some(*value))))
        {
            return false;
        }
    };

    let Some(compare) = next.next() else {
        return false;
    };
    if !matches!(
        compare.mnemonic.as_str(),
        "s_cmp_gt_u32" | "s_cmp_ge_u32" | "s_cmp_lt_u32" | "s_cmp_le_u32"
    ) || !compare.src.contains(&Opnd::Sgpr(scalar))
        || !compare
            .src
            .iter()
            .filter_map(bits)
            .any(|value| (1..=u16::MAX.into()).contains(&value))
    {
        return false;
    }
    // A conditional branch's fall-through path can supply the corroborating
    // use. Stop at every branch target, unconditional transfer and SGPR write.
    for ins in next {
        if ins.mnemonic == "s_bfe_u32" && ins.src.first() == Some(&Opnd::Sgpr(scalar)) {
            if let Some(field) = ins.src.get(1).and_then(bits) {
                let offset = field & 31;
                let width = (field >> 16) & 127;
                return width > 0 && offset > 0 && offset + width <= 23;
            }
        }
        let mask = ins.mnemonic == "s_and_b32"
            && ins.src.first() == Some(&Opnd::Sgpr(scalar))
            && ins
                .src
                .get(1)
                .and_then(bits)
                .is_some_and(|mask| mask != 0 && mask <= 0xffff);
        if !(matches!(ins.enc, Enc::Sop1 | Enc::Sop2 | Enc::Sopc | Enc::Sopk)
            || ins.mnemonic.starts_with("s_cbranch_"))
            || (ins.src.contains(&Opnd::Sgpr(scalar)) && !mask)
            || super::writes_scalar(ins, scalar)
        {
            break;
        }
    }
    false
}

#[cfg(test)]
#[path = "../tests/hlsl/uniform_texel.rs"]
mod tests;
