//! Scalars that can carry a value taken from one lane of the wave.
//!
//! A register is marked with `Sym::NoLaneRead` under `KEY + register` while no value that
//! reached it came out of a VGPR (`v_readfirstlane_b32`, `v_readlane_b32`, a lowered wave
//! operation). The mark follows scalar arithmetic and scalar loads and, like the other
//! dataflow symbols, must hold on every path into a block.
use super::Sym;
use crate::rdna2::{clobbers_scalar, Enc, Insn, Opnd};
use std::collections::HashMap;

const KEY: u16 = 4000;
const SCC: u16 = 253;

fn register(o: &Opnd) -> Option<u16> {
    Some(match o {
        Opnd::Sgpr(n) => u16::from(*n),
        Opnd::VccLo => 106,
        Opnd::VccHi => 107,
        Opnd::M0 => 124,
        // Lane spill slots share the resource dataflow's numbering.
        Opnd::Temp(n) => 1000 + n.checked_sub(super::LANE_TEMP_BASE)?,
        _ => return None,
    })
}

/// At entry every scalar is a launch argument.
pub(super) fn entry(sym: &mut HashMap<u16, Sym>) {
    for r in (0..108).chain([124, SCC]) {
        sym.insert(KEY + r, Sym::NoLaneRead);
    }
}

pub(super) fn clean(sym: &HashMap<u16, Sym>, reg: u16) -> bool {
    sym.contains_key(&(KEY + reg))
}

pub(super) fn step(ins: &Insn, sym: &mut HashMap<u16, Sym>, one_word_masks: bool) {
    if ins.mnemonic == "v_nop" {
        return;
    }
    let scalar = matches!(ins.enc, Enc::Sop1 | Enc::Sop2 | Enc::Sopk | Enc::Sopc | Enc::Smem);
    let vector = matches!(ins.enc, Enc::Vop1 | Enc::Vop2 | Enc::Vop3 | Enc::Vopc);
    let mut written: Vec<u16> = (0..106u8).filter(|&r| clobbers_scalar(ins, r)).map(u16::from).collect();
    if one_word_masks && ins.mnemonic.starts_with("v_cmp") {
        written.retain(|r| ins.dst.iter().filter_map(register).any(|d| d == *r));
    }
    for r in ins.dst.iter().filter_map(register).filter(|r| *r > 105) {
        written.push(r);
        if r == 106 { written.push(107); }
    }
    if vector {
        // Comparisons write a pair in wave64 and VOP3B names its carry outside the
        // operand list: neither leaves a heap symbol behind.
        for r in &written {
            if matches!(sym.get(r), Some(Sym::HeapOffset | Sym::HeapSamplerOffset | Sym::HeapDescriptor(_)
                | Sym::HeapWord(_) | Sym::HeapSamplerIndex)) {
                sym.remove(r);
            }
        }
    }
    // Comparison and carry masks hold one bit per lane.
    let mask = vector && (ins.mnemonic.starts_with("v_cmp") || ins.mnemonic.contains("_co_")
        || matches!(ins.mnemonic.as_str(), "v_mad_u64_u32" | "v_mad_i64_i32" | "v_div_scale_f32" | "v_div_scale_f64"));
    let ok = mask || (scalar && !ins.src.iter().any(|o| matches!(o, Opnd::Vgpr(_))) && {
        let width = match ins.enc {
            Enc::Smem if ins.op >= 8 => 4, // buffer descriptor
            Enc::Smem => 2,
            _ if ins.mnemonic.contains("64") => 2,
            _ => 1,
        };
        let mut reads: Vec<u16> = ins.src.iter().filter_map(register).flat_map(|r| r..r + width).collect();
        match ins.enc {
            Enc::Smem => reads.extend(ins.raw.get(1).map(|w| (w >> 25) as u16).filter(|r| *r < 108)),
            // S_CMOVK and the read-modify-write forms.
            Enc::Sopk => reads.extend(ins.dst.iter().filter_map(register)),
            _ => {}
        }
        if ["s_cselect", "s_cmov", "s_addc", "s_subb"].iter().any(|m| ins.mnemonic.starts_with(m)) {
            reads.push(SCC);
        }
        reads.iter().all(|r| *r >= 108 && *r < 1000 && *r != 124 || clean(sym, *r))
    });
    if scalar && ins.enc != Enc::Smem && !matches!(ins.mnemonic.as_str(), "s_mov_b32" | "s_mov_b64" | "s_movk_i32" | "s_brev_b32") {
        written.push(SCC);
    }
    for r in written {
        if ok {
            sym.insert(KEY + r, Sym::NoLaneRead);
        } else {
            sym.remove(&(KEY + r));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(words: &[u32]) -> HashMap<u16, Sym> {
        let isa: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let mut sym = HashMap::new();
        entry(&mut sym);
        for ins in crate::rdna2::disassemble(&isa) {
            step(&ins, &mut sym, false);
        }
        sym
    }

    #[test]
    fn launch_arguments_and_scalar_arithmetic_stay_clean() {
        let sym = run(&[0x8f00850a]); // s0 = s10 << 5
        assert!(clean(&sym, 0) && clean(&sym, 10));
    }

    #[test]
    fn a_lane_value_taints_what_is_computed_from_it() {
        let sym = run(&[
            0x7e000505, // s0 = readfirstlane(v5)
            0x8f018500, // s1 = s0 << 5
            0x8f02850a, // s2 = s10 << 5
        ]);
        assert!(!clean(&sym, 0) && !clean(&sym, 1));
        assert!(clean(&sym, 2));
        // An independent value makes the register clean again.
        assert!(clean(&run(&[0x7e000505, 0x8f00850a]), 0));
    }

    #[test]
    fn a_scalar_load_is_tainted_by_its_offset() {
        // s3 = buffer[s28:31][s0], from a lane value and from a launch argument.
        assert!(!clean(&run(&[0x7e000505, 0xf42000ce, 0x00000000]), 3));
        assert!(clean(&run(&[0xf42000ce, 0x00000000]), 3));
    }

    #[test]
    fn comparison_masks_are_per_lane_bits_and_drop_heap_symbols() {
        let isa: Vec<u8> = [0xd4030000u32, 0x00010016].into_iter().flat_map(u32::to_le_bytes).collect();
        let ins = crate::rdna2::disassemble(&isa); // s[0:1] = v_cmp_le_f32(s22, 0)
        let mut sym = HashMap::from([(0, Sym::HeapOffset), (1, Sym::HeapWord(1)), (2, Sym::HeapOffset)]);
        entry(&mut sym);
        step(&ins[0], &mut sym, false);
        assert!(clean(&sym, 0));
        assert!(!sym.contains_key(&0) && !sym.contains_key(&1) && sym.contains_key(&2));
    }
}
