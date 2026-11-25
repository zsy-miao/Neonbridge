//! A saved EXEC has two views: per-invocation bits for restoring the predicate,
//! and two uniform words when the shader reads it as integer data.
use super::{ff1_b64_expr, ElectionLowering, Emitter, Enc, Insn, Opnd};
use std::collections::BTreeMap;

#[derive(Clone, Copy)]
pub(super) enum Site {
    Capture(ElectionLowering),
    Predicate,
    LaneMask(ElectionLowering, u8, [Word; 2]),
    First(usize),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Word {
    Saved(usize, bool),
    Uniform,
}

#[derive(Default)]
pub(super) struct Snapshots {
    pub(super) sites: BTreeMap<usize, Site>,
    // Each register word has its own reaching definition, including in a pair.
    inputs: BTreeMap<(usize, u8), (usize, bool)>,
}

impl Snapshots {
    pub(super) fn word(&self, instruction: usize, operand: &Opnd) -> Option<String> {
        let Opnd::Sgpr(reg) = *operand else {
            return None;
        };
        let &(def, high) = self.inputs.get(&(instruction, reg))?;
        Some(format!("saved_exec{def}.{}", if high { "y" } else { "x" }))
    }
}

pub(super) fn captures_exec(ins: &Insn) -> bool {
    (ins.mnemonic == "s_mov_b64" && ins.src == [Opnd::ExecLo])
        || ins.mnemonic == "s_and_saveexec_b64"
}

/// The verified 64-bit EXEC search broadcasts one integer to its guest wave.
fn first_exec_lane(ins: &Insn) -> bool {
    matches!(
        ins.mnemonic.as_str(),
        "s_ff1_i32_b64" | "tb_first_exec_lane"
    ) && ins.src == [Opnd::ExecLo]
}

/// Only ordinary integer data operands. In particular CNDMASK's third
/// source is the lane predicate and must retain its per-invocation value.
pub(super) fn numeric_sources(ins: &Insn) -> usize {
    if ins.aux.sdwa.is_some() {
        return 0;
    }
    match (ins.enc, ins.mnemonic.as_str()) {
        (Enc::Vop1 | Enc::Vop3, "v_mov_b32") => 1,
        (
            Enc::Vop2 | Enc::Vop3,
            "v_add_nc_u32" | "v_and_b32" | "v_lshrrev_b32" | "v_lshlrev_b32" | "v_cndmask_b32",
        ) => 2,
        (Enc::Vopc | Enc::Vop3, m)
            if (m.starts_with("v_cmp_") || m.starts_with("v_cmpx_"))
                && matches!(
                    m.rsplit('_').next(),
                    Some("u16" | "i16" | "u32" | "i32" | "u64" | "i64")
                ) =>
        {
            2
        }
        _ => 0,
    }
}

/// Transfer the reaching definition of each word before updating the destination.
/// A move from an ordinary scalar also ends an old mask definition there.
pub(super) fn copy_scalar_words(ins: &Insn, words: &mut BTreeMap<u8, Word>) -> bool {
    let ([Opnd::Sgpr(src)], [Opnd::Sgpr(dst)]) = (ins.src.as_slice(), ins.dst.as_slice()) else {
        return false;
    };
    let width = match ins.mnemonic.as_str() {
        "s_mov_b32" => 1,
        "s_mov_b64" => 2,
        _ => return false,
    };
    let sources = [words.get(src).copied(), words.get(&(src + 1)).copied()];
    for (offset, source) in sources.into_iter().take(width).enumerate() {
        let dest = dst + offset as u8;
        if let Some(def) = source {
            words.insert(dest, def);
        } else {
            words.remove(&dest);
        }
    }
    true
}

/// Record a scalar destination after its inputs have been consumed. Scalar
/// register copies are transferred separately by `copy_scalar_words`.
pub(super) fn record_scalar_write(ins: &Insn, i: usize, words: &mut BTreeMap<u8, Word>) {
    let [Opnd::Sgpr(dst)] = ins.dst.as_slice() else {
        return;
    };
    let width = match ins.mnemonic.as_str() {
        "s_load_dwordx4" => 4,
        "s_mov_b64" | "s_and_saveexec_b64" => 2,
        m if m.starts_with("v_cmp") => 2,
        _ => 1,
    };
    for word in *dst..dst.saturating_add(width) {
        words.remove(&word);
    }
    if captures_exec(ins) {
        words.insert(*dst, Word::Saved(i, false));
        words.insert(*dst + 1, Word::Saved(i, true));
    } else if first_exec_lane(ins) {
        words.insert(*dst, Word::Uniform);
    } else if matches!(ins.mnemonic.as_str(), "s_mov_b32" | "s_mov_b64")
        && matches!(ins.src.as_slice(), [Opnd::ConstInt(_) | Opnd::Literal(_)])
    {
        for word in *dst..*dst + width {
            words.insert(word, Word::Uniform);
        }
    }
}

/// Native EXEC saves already carry the right per-lane bit when an intact
/// pair controls a lane. A reconstructed pair needs the bit from its chosen words.
fn plan_lane_mask(plan: &mut Snapshots, i: usize, mode: ElectionLowering, lo: u8, pair: [Word; 2]) {
    if matches!(pair, [Word::Saved(lo, false), Word::Saved(hi, true)] if lo == hi) {
        return;
    }
    let count = if mode == ElectionLowering::Native32 {
        1
    } else {
        2
    };
    for (j, word) in pair.iter().take(count).enumerate() {
        if let Word::Saved(def, high) = word {
            if *high == (j == 1) {
                plan.sites.entry(*def).or_insert(Site::Predicate);
            } else {
                plan.sites.insert(*def, Site::Capture(mode));
            }
        }
    }
    plan.sites.insert(i, Site::LaneMask(mode, lo, pair));
}

/// Only called for straight-line compute with established wave geometry.
pub(super) fn analyze(code: &[Insn], mode: ElectionLowering) -> Snapshots {
    let mut words = BTreeMap::new();
    let mut plan = Snapshots::default();
    for (i, ins) in code.iter().enumerate() {
        let predicate = match ins.mnemonic.as_str() {
            "s_mov_b64" if ins.dst == [Opnd::ExecLo] => ins.src.first(),
            "v_cndmask_b32" if ins.enc == Enc::Vop3 && ins.aux.sdwa.is_none() => ins.src.get(2),
            _ => None,
        };
        if let Some(Opnd::Sgpr(lo)) = predicate {
            if let (Some(&low), Some(&high)) = (words.get(lo), words.get(&(lo + 1))) {
                plan_lane_mask(&mut plan, i, mode, *lo, [low, high]);
            }
        }
        let pair = ins.mnemonic.ends_with("_u64") || ins.mnemonic.ends_with("_i64");
        for src in ins.src.iter().take(numeric_sources(ins)) {
            let Opnd::Sgpr(first) = *src else { continue };
            for reg in first..=first + u8::from(pair) {
                if let Some(&Word::Saved(def, high)) = words.get(&reg) {
                    plan.sites.insert(def, Site::Capture(mode));
                    plan.inputs.insert((i, reg), (def, high));
                }
            }
        }
        if copy_scalar_words(ins, &mut words) {
            continue;
        }
        record_scalar_write(ins, i, &mut words);
    }
    for (i, pair) in code.windows(2).enumerate() {
        if matches!(plan.sites.get(&i), Some(Site::Capture(_)))
            && pair[0].mnemonic == "s_mov_b64"
            && first_exec_lane(&pair[1])
        {
            plan.sites.insert(i + 1, Site::First(i));
        }
    }
    plan
}

impl Site {
    pub(super) fn emit(self, em: &mut Emitter<'_>, i: usize, ins: &Insn) -> Result<bool, String> {
        match self {
            Self::Capture(mode) => {
                let ballot = match mode {
                    ElectionLowering::Native32 => "WaveActiveBallot((exec_lo & 1u) != 0u).xy",
                    ElectionLowering::Shared64 => "tb_page_ballot(tb_lane, (exec_lo & 1u) != 0u)",
                };
                em.line(&format!("uint2 saved_exec{i} = {ballot};"));
                return Ok(false); // The guest save and its effect on EXEC still run.
            }
            Self::Predicate => {
                em.line(&format!("uint saved_lane{i} = exec_lo & 1u;"));
                return Ok(false);
            }
            Self::LaneMask(mode, lo, words) => {
                let lane = if mode == ElectionLowering::Native32 {
                    "WaveGetLaneIndex()"
                } else {
                    "tb_lane"
                };
                let mut bit = |j: usize| -> Result<String, String> {
                    let bits = match words[j] {
                        Word::Saved(def, _)
                            if matches!(
                                em.exec_snapshots.sites.get(&def),
                                Some(Site::Predicate)
                            ) =>
                        {
                            return Ok(format!("saved_lane{def}"))
                        }
                        Word::Saved(def, high) => {
                            format!("saved_exec{def}.{}", if high { "y" } else { "x" })
                        }
                        Word::Uniform => {
                            let reg = Opnd::Sgpr(lo + j as u8);
                            em.touch(&reg);
                            super::r(&reg)?
                        }
                    };
                    Ok(format!("(({bits} >> ({lane} & 31u)) & 1u)"))
                };
                let low = bit(0)?;
                let predicate = if mode == ElectionLowering::Native32 {
                    low
                } else {
                    format!("(({lane} & 32u) == 0u ? {low} : {})", bit(1)?)
                };
                if ins.mnemonic == "v_cndmask_b32" {
                    em.line(&format!("uint select_mask{i} = {predicate};"));
                    return Ok(false); // Keep normal VOP3 data modifiers and EXEC predication.
                }
                em.ws(&Opnd::ExecLo, &predicate)?;
                em.ws(&Opnd::ExecHi, "0u")?;
            }
            Self::First(def) => {
                em.ws(
                    &ins.dst[0],
                    &ff1_b64_expr(&format!("saved_exec{def}.x"), &format!("saved_exec{def}.y")),
                )?;
            }
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_word_load_ends_the_lifetime_of_an_overlapping_saved_pair() {
        // Save EXEC in s[30:31], elect a lane, read s31, load s[28:31], then read s30.
        let words = [
            0xbe9e_047eu32,
            0xbe98_147e,
            0x7e04_021f,
            0xf408_0702,
            0xfa00_0000,
            0xbf8c_c07f,
            0x7e08_021e,
        ];
        let bytes = words
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>();
        let code = super::super::disassemble(&bytes);
        let plan = analyze(&code, ElectionLowering::Shared64);
        assert!(matches!(
            plan.sites.get(&0),
            Some(Site::Capture(ElectionLowering::Shared64))
        ));
        assert!(matches!(plan.sites.get(&1), Some(Site::First(0))));
        assert_eq!(plan.inputs.get(&(2, 31)), Some(&(0, true)));
        assert!(!plan.inputs.contains_key(&(5, 30)));
    }
}
