//! Proof for indexed reads of a locally constructed linear byte descriptor.
use crate::rdna2::{Insn, Opnd, clobbers_scalar};

/// The existing DirectRaw binding recognizer proves the root SRV. For IDXEN,
/// additionally require its complete stride-one construction in the same
/// basic block and no intervening overwrite of any of its four words.
pub(super) fn local_byte_descriptor(ops: &[Insn], base: u16) -> bool {
    let Ok(base) = u8::try_from(base) else { return false };
    if base > 100 || base % 4 != 0 { return false; }
    let Some(end) = ops.iter().rposition(|i| clobbers_scalar(i, base + 1)) else {
        return false;
    };
    let Some(start) = end.checked_sub(3) else { return false };
    let [format, size, low, high] = &ops[start..=end] else { return false };
    let [Opnd::Sgpr(root)] = low.src[..] else { return false };
    if root > 104 || (base..=base+3).contains(&root) || (base..=base+3).contains(&(root+1)) {
        return false;
    }
    format.mnemonic == "s_mov_b32"
        && format.dst == [Opnd::Sgpr(base + 3)]
        && format.src == [Opnd::Literal(0x11014fac)]
        && size.mnemonic == "s_mov_b32"
        && size.dst == [Opnd::Sgpr(base + 2)]
        && size.src == [Opnd::ConstInt(-1)]
        && low.mnemonic == "s_mov_b32"
        && low.dst == [Opnd::Sgpr(base)]
        && high.mnemonic == "s_or_b32"
        && high.dst == [Opnd::Sgpr(base + 1)]
        && high.src == [Opnd::Sgpr(root + 1), Opnd::Literal(65536)]
        && ops[end + 1..].iter().all(|i| {
            !(base..=base+3).any(|reg| clobbers_scalar(i, reg))
        })
}
