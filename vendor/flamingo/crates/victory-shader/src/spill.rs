//! The spilled user-data table of a compute program.
//!
//! A compute program has as many user SGPRs as `COMPUTE_PGM_RSRC2` says. A
//! root signature that needs more dwords than that keeps none of its
//! parameters in registers: user-data dwords 0 and 1 then hold the 40-bit
//! address of a table in memory (`s0`, and `s1 & 0xff` as bits 32-39), and
//! the parameter the layout declares at user-data index N (16 or above) is
//! at byte `4 * (N - 16)` of that table. The code reads it with
//! `s_load_dword*` through that pointer. (Every compute program of the
//! console's ray-tracing structure builder, `XboxBvhBuilder.dll`, is of this
//! kind; measured there: 118 of 118 such loads land on a declared parameter.)
//!
//! [`user_data`] rewrites those loads away: each dword they read becomes the
//! entry value of the SGPR its user-data index names, as if the parameter
//! had arrived in a register. Resource tracking and lowering then treat a
//! spilled parameter like any other (the host binds user data by root slot
//! and dword, not by register).

use crate::ShaderError;
use crate::gfx10::{Fields, NULL, Operand};
use crate::ir::{self, Program, Reg, Value};
use crate::native::{HwStage, NativeProgram, StageIo};

/// The user-data index of the table's first dword.
const FIRST: u32 = 16;

/// [`ir::build`] of `native`'s code as the hardware runs it, with the loads
/// of a spilled user-data table rewritten ([`user_data`]).
pub fn build(native: &NativeProgram) -> Result<Program, ShaderError> {
    let mut p = ir::build(&native.patched_code(), native.wave64, native.vgprs)?;
    user_data(&mut p, native);
    Ok(p)
}

/// Whether `native` is a compute program that keeps its root parameters in
/// a spilled user-data table (see the module): the entries no register
/// holds, as (first user-data index, dwords), if the table starts at index
/// 16 and the program has the two registers of its address.
fn spilled(native: &NativeProgram) -> Option<Vec<(u32, u32)>> {
    let StageIo::Cs(cs) = &native.io else {
        return None;
    };
    if native.stage != HwStage::Cs {
        return None;
    }
    let registers = u32::from(cs.user_sgprs());
    let spilled: Vec<(u32, u32)> = native
        .layout
        .iter()
        .filter(|e| e.sgprs > 0 && u32::from(e.user_data) >= registers)
        .map(|e| (u32::from(e.user_data), e.sgprs))
        .collect();
    // Only the one arrangement seen: the table starts at index 16.
    (spilled.iter().map(|e| e.0).min() == Some(FIRST) && registers >= 2).then_some(spilled)
}

/// Whether `native` reads its root parameters from a spilled user-data
/// table. (In the title exactly the compute programs of the
/// console's ray-tracing structure builder do.)
pub fn is_spilled(native: &NativeProgram) -> bool {
    spilled(native).is_some()
}

/// Rewrites the loads of `native`'s spilled user-data table in `p`; returns
/// how many loads it replaced. Nothing changes unless the program is a
/// compute program whose layout has entries past its user SGPRs, the first
/// at index 16. A load is left alone (and resource tracking then refuses it
/// as before) when its offset is not a constant on a dword, or when a dword
/// it reads and the program uses is in no layout entry.
pub fn user_data(p: &mut Program, native: &NativeProgram) -> usize {
    let Some(spilled) = spilled(native) else {
        return 0;
    };
    let entry = |reg: Reg, p: &Program| p.inputs.iter().find(|i| i.0 == reg).map(|i| i.1);
    let (Some(low), Some(high)) = (entry(Reg::S(0), p), entry(Reg::S(1), p)) else {
        return 0;
    };

    // The pointer's halves: `s0` and its copies, `s1 & 0xff` and its copies.
    let mut lows = vec![low];
    let mut highs: Vec<Value> = Vec::new();
    let mut raw_highs = vec![high];
    // Instructions that only form the pointer: dropped when nothing reads
    // them any more.
    let mut forming: Vec<(usize, usize)> = Vec::new();
    loop {
        let mut changed = false;
        for (b, block) in p.blocks.iter().enumerate() {
            for (i, inst) in block.insts.iter().enumerate() {
                let Fields::Sop { dst, src, .. } = &inst.fields else {
                    continue;
                };
                let Some(Operand::Scalar { code: d, .. }) = *dst else {
                    continue;
                };
                let Some(out) = inst.write(Reg::S(d)) else {
                    continue;
                };
                let read = |op: &Operand| match *op {
                    Operand::Scalar { code, .. } if code != NULL => inst.read(Reg::S(code)),
                    _ => None,
                };
                let found = match (inst.name(), &src[..]) {
                    ("s_mov_b32", [s]) => match read(s) {
                        Some(v) if lows.contains(&v) => Some(&mut lows),
                        Some(v) if highs.contains(&v) => Some(&mut highs),
                        Some(v) if raw_highs.contains(&v) => Some(&mut raw_highs),
                        _ => None,
                    },
                    ("s_and_b32", [x, y]) => {
                        let masked = |a: &Operand, m: &Operand| {
                            matches!(*m, Operand::Literal(0xff))
                                && read(a).is_some_and(|v| raw_highs.contains(&v))
                        };
                        (masked(x, y) || masked(y, x)).then_some(&mut highs)
                    }
                    _ => None,
                };
                if let Some(set) = found
                    && !set.contains(&out)
                {
                    set.push(out);
                    forming.push((b, i));
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    if highs.is_empty() {
        return 0;
    }

    // Values some instruction or phi reads.
    let mut read_somewhere = std::collections::HashSet::new();
    for block in &p.blocks {
        read_somewhere.extend(block.phis.iter().flat_map(|phi| phi.args.iter().copied()));
        read_somewhere.extend(block.insts.iter().flat_map(|i| i.reads.iter().map(|r| r.1)));
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

    // Each load through the pointer: its dwords become entry values.
    let mut replaced: std::collections::HashMap<Value, Value> = Default::default();
    let mut gone: Vec<(usize, usize)> = Vec::new();
    let mut inputs = p.inputs.clone();
    for (b, block) in p.blocks.iter().enumerate() {
        'insts: for (i, inst) in block.insts.iter().enumerate() {
            let Fields::Smem {
                data: Some(Operand::Scalar { code: first, .. }),
                base: Operand::Scalar { code: base, .. },
                offset,
                soffset,
                ..
            } = inst.fields
            else {
                continue;
            };
            if !inst.name().starts_with("s_load_dword")
                || !matches!(soffset, Operand::Scalar { code: NULL, .. } | Operand::Const(128))
                || offset < 0
                || offset % 4 != 0
                || !inst.read(Reg::S(base)).is_some_and(|v| lows.contains(&v))
                || !inst.read(Reg::S(base + 1)).is_some_and(|v| highs.contains(&v))
            {
                continue;
            }
            let mut dwords = Vec::new();
            for &(reg, v) in &inst.writes {
                let Reg::S(code) = reg else {
                    continue 'insts;
                };
                let Some(k) = code.checked_sub(first) else {
                    continue 'insts;
                };
                let index = FIRST + offset as u32 / 4 + u32::from(k);
                let declared = spilled.iter().any(|&(at, n)| index >= at && index < at + n);
                if !declared {
                    if read_somewhere.contains(&v) {
                        continue 'insts;
                    }
                    continue;
                }
                // SGPR codes end at 105; no root signature reaches there.
                let Ok(code) = u8::try_from(index) else {
                    continue 'insts;
                };
                if code > 105 {
                    continue 'insts;
                }
                dwords.push((Reg::S(code), v));
            }
            for (reg, v) in dwords {
                let value = match inputs.iter().find(|e| e.0 == reg) {
                    Some(e) => e.1,
                    None => {
                        let value = Value(next);
                        next += 1;
                        inputs.push((reg, value));
                        value
                    }
                };
                replaced.insert(v, value);
            }
            gone.push((b, i));
        }
    }
    if gone.is_empty() {
        return 0;
    }
    p.inputs = inputs;
    let renamed = |v: &mut Value| {
        if let Some(&to) = replaced.get(v) {
            *v = to;
        }
    };
    for block in &mut p.blocks {
        for phi in &mut block.phis {
            phi.args.iter_mut().for_each(renamed);
        }
        for inst in &mut block.insts {
            inst.reads.iter_mut().for_each(|r| renamed(&mut r.1));
        }
    }
    // The loads go, then what only formed the pointer (last first, so a
    // copy goes before what it copied).
    let mut drop = gone.clone();
    loop {
        let read_somewhere = {
            let mut used = std::collections::HashSet::new();
            for (b, block) in p.blocks.iter().enumerate() {
                used.extend(block.phis.iter().flat_map(|phi| phi.args.iter().copied()));
                for (i, inst) in block.insts.iter().enumerate() {
                    if !drop.contains(&(b, i)) {
                        used.extend(inst.reads.iter().map(|r| r.1));
                    }
                }
            }
            used
        };
        let before = drop.len();
        for &site in &forming {
            let inst = &p.blocks[site.0].insts[site.1];
            if !drop.contains(&site) && inst.writes.iter().all(|w| !read_somewhere.contains(&w.1)) {
                drop.push(site);
            }
        }
        if drop.len() == before {
            break;
        }
    }
    for (b, block) in p.blocks.iter_mut().enumerate() {
        let mut i = 0;
        block.insts.retain(|_| {
            i += 1;
            !drop.contains(&(b, i - 1))
        });
    }
    // The pointer's registers are no inputs once nothing reads them.
    let mut read = std::collections::HashSet::new();
    for block in &p.blocks {
        read.extend(block.phis.iter().flat_map(|phi| phi.args.iter().copied()));
        read.extend(block.insts.iter().flat_map(|i| i.reads.iter().map(|r| r.1)));
    }
    p.inputs
        .retain(|&(reg, v)| !matches!(reg, Reg::S(0 | 1)) || read.contains(&v));
    gone.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::{ComputeIo, RootParam, RootType};

    fn param(root: RootType, user_data: u8, sgprs: u32, slot: u8) -> RootParam {
        RootParam {
            root,
            table_offset_dwords: 0,
            sgprs,
            base_register: 0,
            user_data,
            slot,
        }
    }

    /// A compute program with `registers` user SGPRs and the group id after
    /// them.
    fn native(words: &[u32], registers: u32, layout: Vec<RootParam>) -> NativeProgram {
        NativeProgram {
            stage: HwStage::Cs,
            wave64: false,
            vgprs: 256,
            layout,
            used: vec![],
            io: StageIo::Cs(ComputeIo {
                rsrc2: registers << 1 | 1 << 7,
                threads: [1, 1, 1],
            }),
            code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
        }
    }

    // s_and_b32 s3, s1, 0xff; s_mov_b32 s2, s0; s_load_dword s5, s[2:3],
    // 0x10; s_load_dwordx2 s[0:1], s[2:3], 0; s_endpgm.
    const TABLE: [u32; 8] = [
        0x8703_FF01,
        0x0000_00FF,
        0xBE82_0300,
        0xF400_0141,
        0xFA00_0010,
        0xF404_0001,
        0xFA00_0000,
        0xBF81_0000,
    ];

    #[test]
    fn loads_of_the_spilled_table_become_entry_registers() {
        let n = native(
            &TABLE,
            4,
            vec![
                param(RootType::Descriptor, 16, 2, 0),
                param(RootType::Descriptor, 18, 2, 1),
                param(RootType::Constants, 20, 16, 2),
            ],
        );
        let mut p = ir::build(&n.code, false, 256).unwrap();
        assert_eq!(user_data(&mut p, &n), 2);
        let names: Vec<&str> = p.blocks.iter().flat_map(|b| &b.insts).map(|i| i.name()).collect();
        assert!(!names.iter().any(|n| n.starts_with("s_load_")), "{names:?}");
        // The pointer is gone with the loads; byte 0x10 is index 20, the
        // first root constant, and bytes 0-7 are the first root view.
        let mut regs: Vec<Reg> = p.inputs.iter().map(|i| i.0).collect();
        regs.sort();
        assert_eq!(regs, [Reg::S(16), Reg::S(17), Reg::S(20)]);
    }

    /// Flamingo: a spilled program (the structure builder's kind) in groups of 64 asks the
    /// host for a partial `DispatchX`'s thread count along x, the one axis with more than
    /// one thread, and only there (`emit::USER_DISPATCH_THREADS`).
    #[test]
    fn a_spilled_program_asks_for_its_dispatch_thread_count() {
        use crate::emit::USER_DISPATCH_THREADS;
        let mut n = native(
            &TABLE,
            4,
            vec![
                param(RootType::Descriptor, 16, 2, 0),
                param(RootType::Descriptor, 18, 2, 1),
                param(RootType::Constants, 20, 16, 2),
            ],
        );
        n.io = StageIo::Cs(ComputeIo {
            rsrc2: 4 << 1 | 1 << 7,
            threads: [64, 1, 1],
        });
        assert!(is_spilled(&n));
        let p = build(&n).unwrap();
        let st = crate::structure::structure(&p).unwrap();
        let res = crate::resource::track(&p, &n.layout, n.stage, Some(&st)).unwrap();
        let io = crate::io::raise(&p, &n).unwrap();
        let container = crate::dxbc::build(&[]);
        let c = crate::Container::parse(&container).unwrap();
        let shader = crate::emit::emit(&c, &n, &p, &st, &io, &res).unwrap();
        let asked: Vec<u32> = shader
            .user_data
            .iter()
            .filter(|u| u.root == RootType::Other(USER_DISPATCH_THREADS))
            .map(|u| u.dword)
            .collect();
        assert_eq!(asked, [0]);
    }

    #[test]
    fn parameters_in_registers_are_left_alone() {
        // The same code where s0 and s1 are a root view the layout declares.
        let n = native(&TABLE, 6, vec![param(RootType::Descriptor, 0, 2, 0)]);
        let mut p = ir::build(&n.code, false, 256).unwrap();
        let before = p.clone();
        assert_eq!(user_data(&mut p, &n), 0);
        assert_eq!(p, before);
    }

    #[test]
    fn a_used_dword_no_parameter_declares_keeps_its_load() {
        // As above with `s_mov_b32 s6, s5` before the end: the dword at
        // byte 0x10 (index 20) is read, and here nothing declares it.
        let mut words = TABLE.to_vec();
        words.insert(7, 0xBE86_0305);
        let n = native(&words, 4, vec![param(RootType::Descriptor, 16, 2, 0)]);
        let mut p = ir::build(&n.code, false, 256).unwrap();
        assert_eq!(user_data(&mut p, &n), 1);
        let loads = p
            .blocks
            .iter()
            .flat_map(|b| &b.insts)
            .filter(|i| i.name().starts_with("s_load_"))
            .count();
        assert_eq!(loads, 1);
        // The pointer is still read, by the load that stays.
        assert!(p.inputs.iter().any(|i| i.0 == Reg::S(0)));
    }
}
