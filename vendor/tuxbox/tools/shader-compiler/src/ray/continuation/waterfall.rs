//! Native descriptor instructions accept an SGPR address. The compiler loops
//! over groups of lanes with equal addresses. A DXR local shader record already
//! supplies resources per invocation, so each invocation can process its own group.
use super::target;
use crate::rdna2::{Enc, Insn, Opnd};
use std::collections::BTreeMap;

fn writes_scalar(ins: &Insn, reg: u8) -> bool {
    let [Opnd::Sgpr(base)] = ins.dst.as_slice() else {
        return false;
    };
    let width = match ins.mnemonic.as_str() {
        "s_load_dwordx4" => 4,
        "s_load_dwordx8" => 8,
        _ => 1,
    };
    reg.checked_sub(*base).is_some_and(|i| i < width)
}

fn descriptor_loop(
    code: &[Insn],
    lo: usize,
    hi: usize,
    inner: &BTreeMap<usize, usize>,
) -> Option<()> {
    let [low, high, cmp, save, load, .., xor, _] = &code[lo..=hi] else {
        return None;
    };
    let ([Opnd::Sgpr(s)], [Opnd::Vgpr(v)]) = (low.dst.as_slice(), low.src.as_slice()) else {
        return None;
    };
    let [Opnd::Sgpr(saved)] = save.dst.as_slice() else {
        return None;
    };
    let mask = cmp.dst.as_slice();
    if low.mnemonic != "v_readfirstlane_b32"
        || high.mnemonic != low.mnemonic
        || high.dst != [Opnd::Sgpr(s.checked_add(1)?)]
        || high.src != [Opnd::Vgpr(v.checked_add(1)?)]
        || cmp.mnemonic != "v_cmp_eq_u64"
        || cmp.src != [low.dst[0], low.src[0], Opnd::Sgpr(0)]
        || !matches!(mask, [Opnd::Sgpr(_)])
        || save.mnemonic != "s_and_saveexec_b32"
        || save.src != mask
        || save.dst == mask
        || !matches!(load.mnemonic.as_str(), "s_load_dwordx4" | "s_load_dwordx8")
        || load.src != low.dst
        || xor.mnemonic != "s_xor_b32"
        || xor.dst != [Opnd::ExecLo]
        || xor.src != [Opnd::ExecLo, save.dst[0]]
        || code[lo + 4..hi - 1]
            .iter()
            .any(|i| writes_scalar(i, *saved))
    {
        return None;
    }

    let mut i = lo + 5;
    while i < hi - 1 {
        let ins = &code[i];
        if ins.mnemonic == "s_mov_b32" && ins.src == [Opnd::ExecLo] {
            let [Opnd::Sgpr(outer_mask)] = ins.dst.as_slice() else {
                return None;
            };
            let start = i + 1;
            let stop = *inner.get(&start)?;
            if stop >= hi - 1
                || code[start..=stop]
                    .iter()
                    .any(|i| writes_scalar(i, *outer_mask))
            {
                return None;
            }
            i = stop + 1;
            while i < hi - 1 && code[i].mnemonic == "s_nop" {
                i += 1
            }
            if i >= hi - 1
                || code[i].mnemonic != "s_mov_b32"
                || code[i].dst != [Opnd::ExecLo]
                || code[i].src != [Opnd::Sgpr(*outer_mask)]
            {
                return None;
            }
        } else if !(matches!(
            ins.mnemonic.as_str(),
            "s_nop" | "s_waitcnt" | "s_bfe_u32" | "v_lshl_or_b32" | "v_lshrrev_b32"
        ) || (ins.enc == Enc::Mimg
            && (ins.mnemonic.starts_with("image_sample")
                || ins.mnemonic.starts_with("image_load")))
            || (ins.enc == Enc::Mubuf
                && (ins.mnemonic.starts_with("buffer_load")
                    || (ins.mnemonic == "buffer_store_dword"
                        && super::scratch(ins)
                        && ins.aux.mask == 0))))
        {
            return None;
        }
        i += 1;
    }
    Some(())
}

pub(super) fn specialize(code: &mut [Insn]) {
    let edges: Vec<_> = code
        .iter()
        .filter_map(|i| Some((i.offset, target(i)?)))
        .collect();
    let mut matched = BTreeMap::new();
    for (hi, ins) in code
        .iter()
        .enumerate()
        .filter(|(_, i)| i.mnemonic == "s_cbranch_execnz")
    {
        let Some(entry) = target(ins).filter(|&pc| pc < ins.offset) else {
            continue;
        };
        let Ok(lo) = code.binary_search_by_key(&entry, |i| i.offset) else {
            continue;
        };
        if !edges.iter().any(|&(from, to)| {
            (entry < to && to <= ins.offset) && !(entry..=ins.offset).contains(&from)
        }) && descriptor_loop(code, lo, hi, &matched).is_some()
        {
            matched.insert(lo, hi);
        }
    }
    for (lo, hi) in matched {
        // The two words come from the same invocation. Hence cmp_eq_u64 sets
        // the original EXEC mask. The save/mask instructions remain, including
        // their changes to SCC and EXEC at loop exit.
        for ins in &mut code[lo..lo + 3] {
            if ins.mnemonic == "v_cmp_eq_u64" {
                ins.src = vec![Opnd::ExecLo];
            }
            ins.mnemonic = "s_mov_b32".into();
            ins.enc = Enc::Sop1;
        }
        code[hi].mnemonic = "s_nop".into();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rdna2::instructions;

    #[test]
    fn a_proven_pair_uses_each_invocations_address_and_keeps_the_resource_operation() {
        let words: [u32; 14] = [
            2120746286, 2120877359, // readfirstlane s52,s53 from v46,v47
            3571580982, 154676, 3200072758, // compare address and save EXEC in s61
            4094168346, 4194304000, 3213672575, 3212836864, 3212836864, 3761250312,
            2148346402, // typed descriptor load then one dword read
            2306751870, 3213492210, // remove processed lanes and repeat
        ];
        let bytes: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let original: Vec<_> = instructions(&bytes).collect();
        let mut specialized = original.clone();
        specialize(&mut specialized);
        assert_eq!(specialized[0].mnemonic, "s_mov_b32");
        assert_eq!(specialized[1].src, [Opnd::Vgpr(47)]);
        assert_eq!(specialized[2].src, [Opnd::ExecLo]);
        assert_eq!(specialized.last().unwrap().mnemonic, "s_nop");
        for (i, op) in original.iter().enumerate().take(original.len() - 1).skip(3) {
            assert_eq!(specialized[i].raw, op.raw);
            assert_eq!(specialized[i].mnemonic, op.mnemonic);
        }
        let mut mismatched = original.clone();
        mismatched[1].src = vec![Opnd::Vgpr(48)];
        specialize(&mut mismatched);
        assert_eq!(mismatched[0].mnemonic, "v_readfirstlane_b32");
        let mut overwritten = original.clone();
        overwritten[4].dst = vec![Opnd::Sgpr(60)]; // x4 descriptor would clobber saved s61
        specialize(&mut overwritten);
        assert_eq!(overwritten.last().unwrap().mnemonic, "s_cbranch_execnz");
    }
}
