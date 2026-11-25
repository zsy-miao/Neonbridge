//! Jump threading for uniform diamonds. One arm can set a flag that fixes
//! the join's next branch. Registers recycled on that arm then never reach
//! the other successor, even if an ordinary SSA phi joins their values.
use crate::gfx10::{Fields, Operand, Valu};
use crate::ir::{Exit, Program, Reg, Value};
use crate::structure::{Atom, Bool, LaneExit, Structured};
use std::collections::HashMap;

fn plain(v: &Valu) -> bool {
    v.abs == 0
        && v.neg == 0
        && v.opsel == 0
        && !v.clamp
        && v.omod == 0
        && v.sdwa.is_none()
        && v.dpp.is_none()
}

fn immediate(op: Operand) -> Option<u32> {
    match op {
        Operand::Const(c) => crate::resource::inline(c),
        Operand::Literal(n) => Some(n),
        _ => None,
    }
}

pub(super) fn constant_edges(p: &Program, st: &Structured) -> Vec<(usize, usize, usize)> {
    let mut constants = HashMap::new();
    for (b, block) in p.blocks.iter().enumerate() {
        for (i, inst) in block.insts.iter().enumerate() {
            if inst.name() == "v_mov_b32"
                && st.guards[b][i].is_none()
                && let Fields::Valu(v) = &inst.fields
                && plain(v)
                && let [src] = v.src.as_slice()
                && let Some(word) = immediate(*src)
                && let [(_, dst)] = inst.writes.as_slice()
            {
                constants.insert(*dst, word);
            }
        }
    }
    let mut edges = Vec::new();
    for (join, block) in p.blocks.iter().enumerate() {
        let LaneExit::Branch {
            when,
            taken,
            not_taken,
        } = st.exits[join]
        else {
            continue;
        };
        if when == Bool::TRUE || when == Bool::FALSE {
            continue;
        }
        // Keep memory accesses and wave operations at their original
        // reconvergence point. Only duplicate scalar ALU and comparisons.
        if !block.insts.iter().all(|i| match &i.fields {
            Fields::Sop { .. } => true,
            Fields::Sopp { .. } => i.name().starts_with("s_cbranch_"),
            Fields::Valu(_) => i.name().starts_with("v_cmp_"),
            _ => false,
        }) {
            continue;
        }
        let [a, b] = block.preds.as_slice() else {
            continue;
        };
        // A triangle has a direct edge from the fork to the join. A full
        // diamond has an unconditional jump at the end of each arm.
        let arm_from = |arm: usize| match p.blocks[arm].preds.as_slice() {
            &[fork] if p.blocks[arm].exit == Exit::Jump(join) => Some(fork),
            _ => None,
        };
        let fork = match (arm_from(*a), arm_from(*b)) {
            (Some(f), Some(g)) if f == g => f,
            (_, Some(f)) if f == *a => f,
            (Some(f), _) if f == *b => f,
            _ => continue,
        };
        if fork == join || a == b {
            continue;
        }
        let LaneExit::Branch { when: entry, .. } = st.exits[fork] else {
            continue;
        };
        if !st
            .bools
            .support(entry)
            .iter()
            .all(|a| matches!(a, Atom::Uniform(_)))
        {
            continue;
        }
        for (edge, &pred) in block.preds.iter().enumerate() {
            let value = |v: Value| {
                let input = block
                    .phis
                    .iter()
                    .find(|p| p.value == v)
                    .map_or(v, |p| p.args[edge]);
                constants.get(&input).copied()
            };
            // Restrict the branch's decision diagram to known comparisons.
            // Unknown tests stop the proof. Keep the original join for the
            // other arm, which may still go either way.
            let mut cond = when;
            while let Some((Atom::Lane(id), lo, hi)) = st.bools.node(cond) {
                let Some(inst) = block
                    .insts
                    .iter()
                    .find(|i| i.writes.iter().any(|&(_, v)| v == id))
                else {
                    break;
                };
                let Fields::Valu(v) = &inst.fields else { break };
                let [left, right] = v.src.as_slice() else {
                    break;
                };
                let operand = |op| match op {
                    Operand::Vector { reg, dwords: 1 } => value(inst.read(Reg::V(reg))?),
                    _ => immediate(op),
                };
                let (Some(x), Some(y)) = (operand(*left), operand(*right)) else {
                    break;
                };
                if !plain(v) {
                    break;
                }
                let yes = match inst.name() {
                    "v_cmp_eq_u32" | "v_cmp_eq_i32" => x == y,
                    "v_cmp_ne_u32" | "v_cmp_ne_i32" => x != y,
                    _ => break,
                };
                cond = if yes { hi } else { lo };
            }
            if cond == Bool::TRUE || cond == Bool::FALSE {
                edges.push((
                    pred,
                    join,
                    if cond == Bool::TRUE { taken } else { not_taken },
                ));
                break;
            }
        }
    }
    edges
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir;
    use crate::structure;

    // s8 initially holds a descriptor address. One arm overwrites it with
    // a buffer load and sets v1. Only that arm can use s8 as the stored value.
    const WORDS: [u32; 19] = [
        0xbe810380, // s1 = 0 (table address high)
        0xf4080200, 0xfa000000, // load SRV into s[8:11]
        0xf4080300, 0xfa000020, // load UAV into s[12:15]
        0x7e020280, // v1 = 0
        0xbf068002, 0xbf850003, // if s2 == 0, jump to join
        0xf4200204, 0xfa00000c, // s8 = buffer.Load(12)
        0x7e020281, // v1 = 1
        0x7d840280, 0xbf870002, // join: if v1 == 0, jump to constant
        0x7e060208, 0xbf820001, // v3 = s8, jump to store
        0x7e060293, // constant: v3 = 19
        0xe0700000, 0x80030300, // store v3 at byte 0
        0xbf810000, // end
    ];

    fn program(words: &[u32]) -> Program {
        ir::build(
            &words
                .iter()
                .flat_map(|w| w.to_le_bytes())
                .collect::<Vec<_>>(),
            false,
            4,
        )
        .unwrap()
    }

    #[test]
    fn only_the_buffer_load_reaches_the_numeric_read() {
        let p = program(&WORDS);
        let st = structure::structure(&p).unwrap();
        let edges = constant_edges(&p, &st);
        assert_eq!(edges, [(0, 2, 4)]);
        let p = ir::thread_edges(&p, &edges).unwrap();
        let load = &p.blocks[1].insts[0];
        assert_eq!(p.blocks[3].insts[0].read(Reg::S(8)), load.write(Reg::S(8)));
        assert_eq!(
            p.blocks[0].exit,
            Exit::Branch {
                taken: 6,
                not_taken: 1
            }
        );
        assert_eq!(p.blocks[6].exit, Exit::Jump(4));
        assert_eq!(p.blocks[6].insts.len(), p.blocks[2].insts.len());
    }

    #[test]
    fn leaves_unknown_flags_and_divergent_forks_alone() {
        let mut words = WORDS;
        words[5] = 0x7e020202; // v1 = s2
        words[10] = 0x7e020203; // v1 = s3
        let p = program(&words);
        assert!(constant_edges(&p, &structure::structure(&p).unwrap()).is_empty());
        let mut words = WORDS;
        words[6] = 0x7d840080; // compare 0 with lane-dependent v0
        words[7] = 0xbf870003; // branch on VCC
        let p = program(&words);
        assert!(constant_edges(&p, &structure::structure(&p).unwrap()).is_empty());
    }
}
