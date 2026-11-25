//! The compiler spills scalar registers into constant lanes of an otherwise
//! unused VGPR. Each shader invocation can retain those uniform values locally.
use super::target;
use crate::rdna2::{Enc, Insn, Opnd};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

type Slot = (u8, u8);

fn slot(ins: &Insn) -> Result<Option<(Slot, u8, bool)>, String> {
    let write = match ins.mnemonic.as_str() {
        "v_writelane_b32" => true,
        "v_readlane_b32" => false,
        _ => return Ok(None),
    };
    let operands = |vector: u8, scalar: u8, lane: i64| {
        if vector < 128 && scalar < 128 && (0..32).contains(&lane) {
            Some(((vector, lane as u8), scalar, write))
        } else {
            None
        }
    };
    match (ins.dst.as_slice(), ins.src.as_slice(), write) {
        ([Opnd::Vgpr(v)], [Opnd::Sgpr(s), Opnd::ConstInt(i), Opnd::Sgpr(0)], true) => {
            operands(*v, *s, *i)
        }
        // Generic VOP3 decoding labels VDST as a VGPR; READLANE writes an SGPR.
        ([Opnd::Vgpr(s)], [Opnd::Vgpr(v), Opnd::ConstInt(i), Opnd::Sgpr(0)], false) => {
            operands(*v, *s, *i)
        }
        _ => None,
    }
    .map(Some)
    .ok_or_else(|| format!("unsupported scalar lane operands at {:#x}", ins.offset))
}

fn touches(ins: &Insn, register: u8) -> bool {
    if matches!(ins.mnemonic.as_str(), "v_movrels_b32" | "v_movreld_b32") {
        return true;
    }
    let width = if ins.mnemonic.contains("dwordx4") {
        4
    } else if ins.mnemonic.contains("dwordx3") {
        3
    } else if ins.mnemonic.contains("dwordx2") || ins.mnemonic.ends_with("64") {
        2
    } else {
        1
    };
    let overlaps = |op: &Opnd, width: u8| match *op {
        Opnd::Vgpr(base) => register
            .checked_sub(base)
            .is_some_and(|offset| offset < width),
        _ => false,
    };
    let (data, addr) = match ins.enc {
        Enc::Mimg if matches!(ins.op, 0 | 8) && ins.aux.imm == 1 && ins.aux.imm2 == 0 => {
            // Non-array 2D image_load/store consumes X and Y. A16 packs both
            // into one register; neither form touches the following VGPR.
            let a16 = ins.raw.get(1).is_some_and(|word| word & (1 << 30) != 0);
            (ins.aux.mask.count_ones() as u8, if a16 { 1 } else { 2 })
        }
        Enc::Mimg => (
            if ins.mnemonic.starts_with("image_gather4") {
                4
            } else {
                ins.aux.mask.count_ones() as u8
            },
            4,
        ),
        Enc::Mubuf => (width, 1),
        Enc::Flat => (width, width.max(2)),
        _ => (width, width),
    };
    // NSA encodes each address VGPR separately, including noncontiguous
    // coordinates and LOD. Only a packed address names a contiguous tuple.
    let addr = if ins.enc == Enc::Mimg && ins.aux.imm2 != 0 { 1 } else { addr };
    ins.dst.iter().any(|o| overlaps(o, data)) || ins.src.iter().any(|o| overlaps(o, addr))
}

pub(super) fn declarations(code: &[Insn], out: &mut String) -> Result<(), String> {
    let mut live = BTreeMap::new();
    let mut slots = BTreeSet::new();
    let branches: Vec<_> = code
        .iter()
        .filter_map(|ins| target(ins).map(|to| (ins.offset, to)))
        .collect();
    for ins in code {
        if let Some((key, s, write)) = slot(ins)? {
            if write {
                if live
                    .insert(key, (s, ins.offset, false))
                    .is_some_and(|(_, _, read)| !read)
                {
                    return Err(format!(
                        "overlapping scalar lane spills at {:#x}",
                        ins.offset
                    ));
                }
                slots.insert(key);
            } else {
                let Some((original, start, read)) = live.get_mut(&key) else {
                    return Err(format!("unpaired scalar lane restore at {:#x}", ins.offset));
                };
                if *original != s
                    || branches.iter().any(|&(from, to)| {
                        !(*start..ins.offset).contains(&from)
                            && (*start + 1..=ins.offset).contains(&to)
                    })
                {
                    return Err(format!(
                        "scalar lane restore without matching write at {:#x}",
                        ins.offset
                    ));
                }
                // Separate return blocks may both reload this lane. Reading
                // the guest VGPR leaves its saved value available.
                *read = true;
            }
        }
    }
    if live.values().any(|(_, _, read)| !read) {
        return Err("unterminated scalar lane spill".into());
    }
    // Even after a scalar restore, the original VGPR still contains the
    // saved words. Only substitute registers reserved for this purpose.
    let vectors: BTreeSet<_> = slots.iter().map(|&(v, _)| v).collect();
    if !vectors.is_empty() {
        for ins in code {
            if slot(ins)?.is_none() && vectors.iter().any(|&v| touches(ins, v)) {
                return Err(format!(
                    "vector operation overlaps scalar lane spill at {:#x}",
                    ins.offset
                ));
            }
        }
    }
    for (v, i) in slots {
        writeln!(out, "uint savedLane{v}_{i};").unwrap();
    }
    Ok(())
}

pub(super) fn emit(ins: &Insn) -> Result<Option<String>, String> {
    Ok(slot(ins)?.map(|((v, i), s, write)| {
        if write {
            format!("savedLane{v}_{i} = s[{s}];")
        } else {
            format!("s[{s}] = savedLane{v}_{i};")
        }
    }))
}

#[cfg(test)]
#[path = "../../tests/ray/continuation/lanes.rs"]
mod tests;
