//! Scalar bitwise operations that mix a lane mask with a plain number.
//!
//! In the per-lane form a mask SGPR holds only this lane's bit, while a number
//! holds every lane's bit. `s_and_b64 s[0:1], exec, s[4:5]` must therefore take
//! bit `lane` of the number, not its low bit, and SCC is "any bit of the whole
//! 64-bit result", not this lane's bit. Ported from xcl 438d420 (Kalmarv,
//! PR #1866, copy with credit allowed): the numeric operand of a mixed mask
//! operation is given a lane atom, so its branch and select guards see the
//! same mask-valued SCC. Which SGPRs are masks comes from the CFG mask
//! analysis (`lane_mask_reads`); a register that is a mask on any path is a
//! mask, as in ScarlettPort's victory-stack structure.rs classification.
//! TB_MIXED_MASK=0 turns it off.

use std::collections::BTreeSet;

use super::{is_lane_mask, Insn, Opnd};

/// Comment line the emitter writes at each site, counted by `--refusals`.
pub(crate) const MARK: &str = "// tb_mixed_mask";

pub(super) fn on() -> bool {
    std::env::var("TB_MIXED_MASK").map_or(true, |v| v.trim() != "0")
}

/// The bitwise operation of a scalar mask op (`and`, `or`, ...), if `ins` is one.
pub(super) fn bitwise(ins: &Insn) -> Option<&str> {
    let m = ins.mnemonic.as_str();
    let op = m.strip_prefix("s_")?;
    let op = op.strip_suffix("_b64").or_else(|| op.strip_suffix("_b32"))?;
    matches!(op, "and" | "or" | "xor" | "andn2" | "orn2" | "nand" | "nor" | "xnor").then_some(op)
}

/// With the masks known before `ins`: the source position of the lane mask
/// when the other source is a plain number (an SGPR that is no mask, or a
/// constant other than 0 and all ones, whose bits differ by lane).
pub(super) fn mask_position(masks: &BTreeSet<u8>, ins: &Insn) -> Option<usize> {
    if bitwise(ins).is_none() || ins.src.len() != 2 {
        return None;
    }
    let number = |o: &Opnd| match *o {
        Opnd::Sgpr(n) => !masks.contains(&n) && !(n > 0 && masks.contains(&(n - 1))),
        Opnd::Literal(v) => v != 0 && v != u32::MAX,
        Opnd::ConstInt(v) => v != 0 && v != -1,
        _ => false,
    };
    (0..2).find(|&k| is_lane_mask(masks, &ins.src[k]) && number(&ins.src[1 - k]))
}
