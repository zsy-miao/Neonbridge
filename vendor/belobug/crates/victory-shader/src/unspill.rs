//! Root parameters read from the user-data spill table.
//!
//! A program whose root parameters do not all fit in user SGPRs gets the
//! rest in memory: the hardware sets a pointer (here an input SGPR pair no
//! root parameter covers) and the program loads the parameters through it
//! with scalar loads, the first spilled user-data dword at offset 0. The
//! Xbox BVH builder's compute programs do this: two user SGPRs, the pointer
//! (its high half masked to 40 bits), and every root parameter from user
//! data 16 on loaded at `4 * (user_data - 16)`.
//!
//! The host passes every root parameter itself, so such a load reads values
//! the root signature already provides. [`unspill`] replaces it: its results
//! become the program's inputs in the SGPRs the layout names for those user
//! data dwords (as if they had not been spilled), which resource tracking
//! and lowering then read as root parameters. Confirmed over the corpus:
//! of the 104 such loads in the 52 BVH builder programs, every dword read
//! lies in a root parameter; the 10 loads that also cover dwords past the
//! last parameter (a 6- or 3-dword constant range loaded with x8 or x4) do
//! not read those dwords, which [`unspill`] requires.

use std::collections::HashMap;

use crate::gfx10::{Fields, NULL, Operand};
use crate::ir::{Program, Reg, Value};
use crate::native::{HwStage, RootParam};

/// Rewrites scalar loads from the user-data spill table into reads of the
/// user SGPRs the root layout names. Leaves the program unchanged where
/// anything does not fit (other programs, unexpected loads): those fail
/// later with the reason they always did.
pub fn unspill(p: &mut Program, layout: &[RootParam], stage: HwStage) {
    if layout.is_empty() {
        return;
    }
    let covered = |code: u8| {
        layout.iter().any(|r| {
            let first = u32::from(stage.user_sgpr(r.user_data));
            (first..first + r.sgprs).contains(&u32::from(code))
        })
    };
    let input_reg: HashMap<Value, Reg> = p.inputs.iter().map(|&(r, v)| (v, r)).collect();
    let input_of: HashMap<Reg, Value> = p.inputs.iter().copied().collect();

    // Defining instruction of each value, for following copies.
    let mut def: HashMap<Value, (usize, usize)> = HashMap::new();
    for (b, block) in p.blocks.iter().enumerate() {
        for (i, inst) in block.insts.iter().enumerate() {
            for &(_, v) in &inst.writes {
                def.insert(v, (b, i));
            }
        }
    }
    // The input a value copies (`s_mov_b32`), or for a high half also masks
    // with a constant (`s_and_b32`).
    let origin = |mut v: Value, high: bool| -> Option<Reg> {
        for _ in 0..8 {
            if let Some(&r) = input_reg.get(&v) {
                return Some(r);
            }
            let &(b, i) = def.get(&v)?;
            let inst = &p.blocks[b].insts[i];
            let Fields::Sop { src, .. } = &inst.fields else {
                return None;
            };
            let scalar = |o: &Operand| match *o {
                Operand::Scalar { code, .. } => inst.read(Reg::S(code)),
                _ => None,
            };
            v = match (inst.name(), src.as_slice()) {
                ("s_mov_b32", [s]) => scalar(s)?,
                ("s_and_b32", [a, b]) if high => match (a, b) {
                    (Operand::Scalar { .. }, Operand::Literal(_) | Operand::Const(_)) => scalar(a)?,
                    (Operand::Literal(_) | Operand::Const(_), Operand::Scalar { .. }) => scalar(b)?,
                    _ => return None,
                },
                _ => return None,
            };
        }
        None
    };

    // Every value some instruction or phi reads.
    let mut read: HashMap<Value, usize> = HashMap::new();
    for block in &p.blocks {
        for phi in &block.phis {
            for a in &phi.args {
                *read.entry(*a).or_default() += 1;
            }
        }
        for inst in &block.insts {
            for &(_, v) in &inst.reads {
                *read.entry(v).or_default() += 1;
            }
        }
    }

    let mut next = p
        .inputs
        .iter()
        .map(|i| i.1)
        .chain(p.blocks.iter().flat_map(|b| {
            b.phis
                .iter()
                .map(|phi| phi.value)
                .chain(b.insts.iter().flat_map(|i| i.writes.iter().map(|w| w.1)))
        }))
        .map(|v| v.0 + 1)
        .max()
        .unwrap_or(0);
    let mut new_inputs: HashMap<Reg, Value> = HashMap::new();
    let mut replace: HashMap<Value, Value> = HashMap::new();
    let mut remove: Vec<(usize, usize)> = Vec::new();

    for (b, block) in p.blocks.iter().enumerate() {
        'inst: for (i, inst) in block.insts.iter().enumerate() {
            let name = inst.name();
            if !name.starts_with("s_load_dword") {
                continue;
            }
            let Fields::Smem {
                data: Some(Operand::Scalar { dwords, .. }),
                base: Operand::Scalar { code: base, .. },
                offset,
                soffset,
                ..
            } = &inst.fields
            else {
                continue;
            };
            if !matches!(soffset, Operand::Scalar { code: NULL, .. })
                || *offset < 0
                || offset % 4 != 0
            {
                continue;
            }
            let (Some(lo), Some(hi)) = (inst.read(Reg::S(*base)), inst.read(Reg::S(base + 1)))
            else {
                continue;
            };
            let (Some(Reg::S(lo_reg)), Some(Reg::S(hi_reg))) =
                (origin(lo, false), origin(hi, true))
            else {
                continue;
            };
            if hi_reg != lo_reg.wrapping_add(1) || covered(lo_reg) || covered(hi_reg) {
                continue;
            }
            // The first spilled user-data dword: the lowest root parameter
            // whose SGPRs lie past the pointer.
            let Some(first) = layout
                .iter()
                .filter(|r| stage.user_sgpr(r.user_data) > hi_reg)
                .map(|r| r.user_data)
                .min()
            else {
                continue;
            };
            let mut pending = Vec::new();
            for (k, &(reg, v)) in inst.writes.iter().enumerate().take(usize::from(*dwords)) {
                let Reg::S(_) = reg else { continue 'inst };
                let user_data = u32::from(first) + (*offset as u32) / 4 + k as u32;
                let in_param = layout.iter().any(|r| {
                    (u32::from(r.user_data)..u32::from(r.user_data) + r.sgprs).contains(&user_data)
                });
                if !in_param {
                    if read.contains_key(&v) {
                        continue 'inst;
                    }
                    continue;
                }
                let Ok(user_data) = u8::try_from(user_data) else {
                    continue 'inst;
                };
                let sgpr = u32::from(stage.user_sgpr(user_data));
                if sgpr >= 106 {
                    continue 'inst;
                }
                let target = Reg::S(sgpr as u8);
                // The register must not already arrive with another value.
                if input_of.contains_key(&target) {
                    continue 'inst;
                }
                pending.push((target, v));
            }
            for (target, v) in pending {
                let n = *new_inputs.entry(target).or_insert_with(|| {
                    next += 1;
                    Value(next - 1)
                });
                replace.insert(v, n);
            }
            remove.push((b, i));
        }
    }
    if remove.is_empty() {
        return;
    }
    for block in &mut p.blocks {
        for phi in &mut block.phis {
            for a in &mut phi.args {
                if let Some(&n) = replace.get(a) {
                    *a = n;
                }
            }
        }
        for inst in &mut block.insts {
            for r in &mut inst.reads {
                if let Some(&n) = replace.get(&r.1) {
                    r.1 = n;
                }
            }
        }
    }
    for &(b, i) in remove.iter().rev() {
        p.blocks[b].insts.remove(i);
    }
    let mut added: Vec<(Reg, Value)> = new_inputs.into_iter().collect();
    added.sort();
    p.inputs.extend(added);
    p.inputs.sort();
}

#[cfg(test)]
mod tests {
    use super::unspill;
    use crate::ShaderError;
    use crate::native::{HwStage, RootParam, RootType};
    use crate::resource::track;

    const S_ENDPGM: u32 = 0xBF81_0000;

    fn param(root: RootType, user_data: u8, sgprs: u32) -> RootParam {
        RootParam {
            root,
            table_offset_dwords: 0,
            sgprs,
            base_register: 0,
            user_data,
            slot: 0,
        }
    }

    fn program(words: &[u32]) -> crate::ir::Program {
        let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        crate::ir::build(&code, false, 256).unwrap()
    }

    /// The pointer in s[0:1], copied to s[2:3] with its high half masked.
    const POINTER: [u32; 3] = [
        0xBE82_0300, // s_mov_b32 s2, s0
        0x8703_FF01, // s_and_b32 s3, s1, lit
        0x0000_00FF,
    ];

    fn spilled_layout() -> Vec<RootParam> {
        vec![
            param(RootType::Descriptor, 16, 2),
            param(RootType::Descriptor, 18, 2),
            param(RootType::Constants, 20, 3),
        ]
    }

    #[test]
    fn a_root_descriptor_loaded_from_the_spill_table_is_the_root_parameter() {
        let mut words = POINTER.to_vec();
        words.extend([
            0xF404_0201,
            0xFA00_0008, // s_load_dwordx2 s[8:9], s[2:3], 0x8
            0xF400_0284,
            0xFA00_0010, // s_load_dword s10, s[8:9], 0x10
            S_ENDPGM,
        ]);
        let layout = spilled_layout();
        let before = program(&words);
        assert_eq!(
            track(&before, &layout, HwStage::Cs).unwrap_err(),
            ShaderError::Unsupported {
                offset: 0x0c,
                what: "an address in an SGPR that is no root parameter"
            }
        );
        let mut p = before.clone();
        unspill(&mut p, &layout, HwStage::Cs);
        // The table load is gone; the second root descriptor arrives in s18:s19.
        assert!(
            p.blocks
                .iter()
                .flat_map(|b| &b.insts)
                .all(|i| i.machine.offset != 0x0c)
        );
        assert!(p.inputs.iter().any(|i| i.0 == crate::ir::Reg::S(18)));
        assert!(p.inputs.iter().any(|i| i.0 == crate::ir::Reg::S(19)));
        let r = track(&p, &layout, HwStage::Cs).unwrap();
        assert_eq!(r.scalar.len(), 1);
        assert_eq!(r.scalar[0].1.param, 1);
        assert_eq!(r.scalar[0].1.offset, 0x10);
    }

    #[test]
    fn padding_dwords_past_the_last_parameter_may_be_loaded_but_not_read() {
        // x4 at 0x10 covers user data 20-23; the constants hold 20-22.
        let mut words = POINTER.to_vec();
        words.extend([
            0xF408_0201,
            0xFA00_0010, // s_load_dwordx4 s[8:11], s[2:3], 0x10
            0xBE8C_0308, // s_mov_b32 s12, s8
            S_ENDPGM,
        ]);
        let layout = spilled_layout();
        let mut p = program(&words);
        unspill(&mut p, &layout, HwStage::Cs);
        assert!(
            p.blocks
                .iter()
                .flat_map(|b| &b.insts)
                .all(|i| !i.name().starts_with("s_load"))
        );
        assert!(p.inputs.iter().any(|i| i.0 == crate::ir::Reg::S(20)));

        // Reading the padding dword keeps the load (and its old failure).
        let mut words = POINTER.to_vec();
        words.extend([
            0xF408_0201,
            0xFA00_0010, // s_load_dwordx4 s[8:11], s[2:3], 0x10
            0xBE8C_030B, // s_mov_b32 s12, s11
            S_ENDPGM,
        ]);
        let mut p = program(&words);
        let before = p.clone();
        unspill(&mut p, &layout, HwStage::Cs);
        assert_eq!(p, before);
    }

    #[test]
    fn loads_through_a_root_parameter_are_left_alone() {
        // s[16:17] is the first root descriptor: an ordinary load through it.
        let words = [
            0xF404_0208,
            0xFA00_0000, // s_load_dwordx2 s[8:9], s[16:17], 0x0
            S_ENDPGM,
        ];
        let layout = spilled_layout();
        let mut p = program(&words);
        let before = p.clone();
        unspill(&mut p, &layout, HwStage::Cs);
        assert_eq!(p, before);
    }
}
