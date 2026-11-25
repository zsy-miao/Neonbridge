//! A saved EXEC is represented by one bit per host invocation. WQM on
//! that value must exchange bits with the other three pixels of the quad.
use super::{Emitter, Enc, Insn, Opnd, writes_scalar};

pub(super) fn saved_exec(code: &[Insn], at: usize) -> bool {
    let op = &code[at];
    let [Opnd::Sgpr(low)] = op.src.as_slice() else {
        return false;
    };
    if op.mnemonic != "s_wqm_b64" || *low >= 127 {
        return false;
    }
    let pair = Opnd::Sgpr(*low);
    let mut saved = false;
    for ins in &code[..at] {
        if !writes_scalar(ins, *low) && !writes_scalar(ins, *low + 1) {
            continue;
        }
        if ins.dst != [pair] {
            return false;
        }
        match ins.mnemonic.as_str() {
            "s_mov_b64" if ins.src == [Opnd::ExecLo] => saved = true,
            "s_andn2_b64" if saved && ins.src == [pair, Opnd::ExecLo] => {}
            _ => return false,
        }
    }
    // A loop back to this instruction could carry a later register definition.
    saved
        && !code[at..].iter().any(|ins| {
            ins.enc == Enc::Sopp
                && Emitter::branch_cond(ins.op).is_some()
                && ins.offset as i64 + 4 + 4 * ins.aux.imm <= op.offset as i64
        })
}

pub(super) fn guards_expansion(code: &[Insn], at: usize) -> bool {
    let Some([branch, wqm]) = code.get(at + 1..at + 3) else {
        return false;
    };
    branch.mnemonic == "s_cbranch_scc0"
        && branch.aux.imm > 0
        && code[at].dst == wqm.src
        && saved_exec(code, at + 2)
}

pub(super) fn vote(id: usize, mask: &str) -> String {
    // Use bitwise OR so all four quad operations execute even where the
    // current lane's bit is already set.
    format!(
        "uint qb{id} = {mask} & 1u; uint q{id} = qb{id} | QuadReadAcrossX(qb{id}) | QuadReadAcrossY(qb{id}) | QuadReadAcrossDiagonal(qb{id});"
    )
}

#[cfg(test)]
#[path = "../tests/hlsl/pixel_quad.rs"]
mod tests;
