//! Recognize the Scarlett wave64 append sequence independently of the culling
//! code that selects its active lanes. Its queue and statistics descriptors
//! and the encodings of the wave operations are part of the matched ABI.
use super::writes_scalar;
use crate::rdna2::{
    Enc, Insn,
    Opnd::{self, Sgpr, Vgpr},
};

pub(super) struct Tail {
    pub start: usize,
    pub node: Opnd,
    pub limit: Opnd,
    pub address: Opnd,
    pub flags: Opnd,
}

// A code generator emits the same register allocation for its internal wave
// temporaries across these variants. Fingerprint complete instruction words:
// DPP controls, EXEC masks, atomic return flags and branch destinations all
// affect the result, even when the decoded mnemonics are identical.
fn sequence(code: &[Insn], expected: u64) -> bool {
    let fingerprint = code
        .iter()
        .flat_map(|op| &op.raw)
        .flat_map(|w| w.to_le_bytes())
        .fold(0xcbf29ce484222325u64, |hash, b| {
            (hash ^ u64::from(b)).wrapping_mul(0x100000001b3)
        });
    fingerprint == expected
}

// These single-dword instructions take their first scalar source in bits 0..7.
// The other fields, including the operation and destination, must match.
fn scalar_input(op: &Insn, encoding: u32) -> Option<Opnd> {
    let [word] = op.raw[..] else { return None };
    let [reg @ Sgpr(10..=105), ..] = op.src[..] else {
        return None;
    };
    (word & !0xff == encoding).then_some(reg)
}

fn table_reaches_tail(prefix: &[Insn], start: usize, end: usize) -> bool {
    let mut copy = None;
    let mut zero = None;
    for op in prefix {
        if writes_scalar(op, 52) {
            if op.raw != [0xbeb40307] || copy.is_some() {
                return false;
            }
            copy = Some(op.offset);
        }
        if writes_scalar(op, 53) {
            if op.raw != [0xbeb50380] || zero.is_some() {
                return false;
            }
            zero = Some(op.offset);
        }
        if copy.is_none() && writes_scalar(op, 7) {
            return false;
        }
    }
    let (Some(copy), Some(zero)) = (copy, zero) else {
        return false;
    };
    // An early exit goes to ENDPGM. No other incoming branch may bypass
    // either table-address definition or enter the middle of the append.
    prefix.iter().all(|op| {
        if matches!(
            op.mnemonic.as_str(),
            "s_setpc_b64" | "s_swappc_b64" | "s_call_b64"
        ) {
            return false;
        }
        if op.enc != Enc::Sopp
            || !(op.mnemonic == "s_branch" || op.mnemonic.starts_with("s_cbranch_"))
        {
            return true;
        }
        let target = op.offset as i64 + 4 + op.aux.imm * 4;
        target == end as i64
            || (target >= 0
                && target <= start as i64
                && [copy, zero]
                    .iter()
                    .all(|&at| op.offset >= at || target <= at as i64))
    })
}

pub(super) fn recognize(code: &[Insn]) -> Option<Tail> {
    let (prefix, tail) = code.split_at(code.len().checked_sub(145)?);
    let start = tail[0].offset;
    let end = tail[144].offset;
    // Three loads through the incoming UAV table: queue u2, stats u3, nodes u1.
    if tail[0].raw != [0xf408001a, 0xfa000040]
        || tail[128].raw != [0xf408001a, 0xfa000060]
        || tail[136].raw != [0xf408001a, 0xfa000020]
    {
        return None;
    }

    let flags = scalar_input(&tail[98], 0x87008100)?; // s_and_b32 flags, 1
    let limit = scalar_input(&tail[133], 0x7d880800)?; // v_cmp_gt_u32 capacity, index
    let address = scalar_input(&tail[137], 0x90048400)?; // s_lshr_b32 encoded base, 4
    if !sequence(&tail[1..37], 0xa04ee4a75076c3fa)     // elect, sum, atomic-add at +16, broadcast old counter
        || !sequence(&tail[37..72], 0x630c9c68dba4d4f2) // exclusive prefix, count both halves of the mask
        || !sequence(&tail[72..98], 0x44f296f186d439d5) // count candidates, atomic-add at +20
        || !sequence(&tail[99..128], 0xb5d8a38bc0c0855b) // conditional stats election/reduction
        || !sequence(&tail[129..133], 0xca13f6ad2d54358a) // stats atomic-add at +16
        || !sequence(&tail[134..136], 0xe682cb50bccb78c5) // bound each write by the capacity
        || !sequence(&tail[138..145], 0xc15637baa286a961) // node = (v0 << 3) | 1, second word zero
        || !table_reaches_tail(prefix, start, end)
    {
        return None;
    }
    Some(Tail {
        start,
        node: Vgpr(0),
        limit,
        address,
        flags,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rdna2::disassemble;

    fn reaches(words: &[u32]) -> bool {
        let bytes: Vec<_> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
        table_reaches_tail(&disassemble(&bytes), bytes.len(), bytes.len() + 64)
    }

    #[test]
    fn append_table_must_reach_every_entry_to_the_tail() {
        const COPY: u32 = 0xbeb40307; // s52 = incoming UAV table s7
        const ZERO: u32 = 0xbeb50380; // s53 = 0
        assert!(reaches(&[COPY, ZERO]));
        assert!(!reaches(&[COPY, 0xbf820001, ZERO])); // branch over high word
        assert!(!reaches(&[COPY, ZERO, 0xbf820001])); // branch into append
        assert!(!reaches(&[COPY, ZERO, 0xbeb40380])); // overwrite table
        assert!(!reaches(&[0xbe870380, COPY, ZERO])); // overwrite source
        assert!(!reaches(&[COPY, ZERO, 0xf4080c9a, 0xfa000040])); // load s50..53
    }
}
