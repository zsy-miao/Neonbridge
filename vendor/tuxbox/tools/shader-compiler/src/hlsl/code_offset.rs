//! A narrow proof of genuinely variable offsets into an embedded code table.
//! Never use modulo to make an unproven address fit a table.
use super::{Emitter, branch_targets};
use crate::rdna2::{clobbers_scalar, Enc, Insn, Opnd};

fn literal(op: &Opnd) -> Option<u32> {
    match *op {
        Opnd::ConstInt(n) => Some(n as u32),
        Opnd::Literal(n) => Some(n),
        _ => None,
    }
}

// A last local definition cannot be bypassed by an entry or a branch.
fn definition(ops: &[Insn], targets: &std::collections::BTreeSet<i64>, at: usize, reg: u8) -> Option<usize> {
    for j in (at.saturating_sub(32)..at).rev() {
        if targets.contains(&(ops[j + 1].offset as i64))
            || ops[j].enc == Enc::Sopp && Emitter::branch_cond(ops[j].op).is_some()
            || ops[j].mnemonic == "s_endpgm" {
            return None;
        }
        if clobbers_scalar(&ops[j], reg) { return Some(j); }
    }
    None
}

// Upper bit mask: every possible result is a subset of this mask. An
// ordinary scalar source may be entirely unknown; AND still bounds it.
pub(super) fn possible_bits(ops: &[Insn], targets: &std::collections::BTreeSet<i64>, at: usize, reg: u8, depth: u8) -> Option<u32> {
    if depth == 0 { return None; }
    let Some(j) = definition(ops, targets, at, reg) else { return Some(u32::MAX); };
    let ins = &ops[j];
    if ins.dst.first() != Some(&Opnd::Sgpr(reg)) { return Some(u32::MAX); }
    let bits = |op: &Opnd| -> Option<u32> {
        if let Some(v) = literal(op) { return Some(v); }
        match *op {
            Opnd::Sgpr(r @ 0..=105) => possible_bits(ops, targets, j, r, depth - 1),
            _ => None,
        }
    };
    match (ins.mnemonic.as_str(), ins.src.as_slice()) {
        ("s_mov_b32", [a]) => bits(a),
        ("s_and_b32", [a,b]) => Some(bits(a)? & bits(b)?),
        // Any bit possible in either operand may survive XOR or the SCC selection.
        ("s_or_b32" | "s_xor_b32" | "s_cselect_b32", [a,b]) => {
            Some(bits(a).unwrap_or(u32::MAX) | bits(b).unwrap_or(u32::MAX))
        }
        ("s_bfe_u32", [data, control]) => {
            let Some(field) = literal(control) else { return Some(u32::MAX); };
            let offset = field & 31;
            let width = (field >> 16) & 127;
            let mask = if width >= 32 { u32::MAX } else { (1u32 << width) - 1 };
            Some((bits(data).unwrap_or(u32::MAX) >> offset) & mask)
        }
        ("s_lshl_b32", [a,b]) => Some(bits(a)? << (literal(b)? & 31)),
        ("s_lshr_b32", [a,b]) => Some(bits(a)? >> (literal(b)? & 31)),
        _ => Some(u32::MAX),
    }
}

/// Return the byte offset register only if every possible byte address and
/// every dword of the load are aligned and inside the embedded table.
/// Require a directly reaching GETPC pair; no inference from a lone low word.
pub(super) fn prove(ops: &[Insn], at: usize, code_bytes: usize, data_bytes: usize) -> Option<u8> {
    let ins = ops.get(at)?;
    if code_bytes % 4 != 0 || data_bytes < code_bytes || data_bytes % 4 != 0
        || data_bytes > u32::MAX as usize
        || ins.enc != Enc::Smem || ins.op > 4 || !ins.is_complete()
        || ops.len() > 32768
        || ops.iter().any(|i| !i.is_complete() || i.enc == Enc::Unknown
            || i.mnemonic.contains('<') || i.mnemonic.starts_with("tb_")
            || i.mnemonic.starts_with("s_movreld_") || i.mnemonic.starts_with("s_subvector_loop_")
            || i.mnemonic.starts_with("s_cbranch_") && Emitter::branch_cond(i.op).is_none()
            || matches!(i.mnemonic.as_str(), "s_setpc_b64" | "s_swappc_b64" |
                "s_rfe_b64" | "s_call_b64" | "s_movrelsd_2_b32" | "s_trap"))
    { return None; }
    let Opnd::Sgpr(base @ 0..=104) = *ins.src.first()? else { return None };
    if base % 2 != 0 { return None; }
    let offset = (ins.raw.get(1)? >> 25) as u8;
    if offset > 105 { return None; }
    let targets = branch_targets(ops);
    let pc = definition(ops, &targets, at, base)?;
    if definition(ops, &targets, at, base+1)? != pc
        || ops[pc].mnemonic != "s_getpc_b64"
        || ops[pc].dst.first() != Some(&Opnd::Sgpr(base))
    { return None; }
    // Require the offset's bound to originate within the same basic block.
    let mask = possible_bits(ops, &targets, at, offset, 8)?;
    if mask & 3 != 0 { return None; }
    let start = ops[pc].offset.checked_add(4)?.checked_add(usize::try_from(ins.aux.imm).ok()?)?;
    let end = start.checked_add(mask as usize)?.checked_add(4usize << ins.op)?;
    if start % 4 != 0 || start < code_bytes || end > data_bytes { return None; }
    Some(offset)
}
