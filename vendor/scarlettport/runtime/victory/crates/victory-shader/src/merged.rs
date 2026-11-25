//! The geometry half of a merged ES-GS stage, made ready to run as a host
//! geometry shader.
//!
//! On the console the export (vertex) half and the geometry half of a
//! merged stage are one hardware program: the driver links them, and the
//! export half hands over with `s_setpc_b64` to the address in its system
//! SGPRs s6-s7. The geometry half (a `Gs` program) starts with the
//! merged-stage system SGPRs: s3 holds the wave's lane counts (bits 8-15:
//! geometry invocations) and the wave's LDS place, and user data from s8.
//! Two things in it mean nothing on the host, where one host thread is one
//! invocation:
//!
//! - EXEC made from the invocation count (`s_bfe_i32 sN, s3, 8 bits at 8;
//!   s_bfm_b64 exec, sN, 0`, then `s_cselect_b64 exec, -1, exec` for a full
//!   wave): every host thread is an invocation, so EXEC stays as it enters;
//! - the place of the invocation's output vertices in LDS, partly loaded
//!   through user-data dwords 0-1 (a 40-bit pointer: s8, s9 & 0xff) from a
//!   table of the driver's: the host writes outputs by their place within
//!   the vertex (`io`), not by address, so those dwords read as 0.
//!
//! [`prepare`] rewrites both away in the IR.

use crate::gfx10::{Fields, NULL, Operand};
use crate::ir::{EXEC, Program, Reg, Value};
use crate::native::{HwStage, NativeProgram};

/// Register codes past the machine's that the rewritten loads' dwords
/// become entry values of: no instruction writes them, and the lowering
/// reads such an input as 0.
const DRIVER_TABLE: u8 = 240;

/// Applies the rewrites of the module to `p`, the IR of `native`; nothing
/// changes for other stages. Returns how many instructions went.
pub fn prepare(p: &mut Program, native: &NativeProgram) -> usize {
    if native.stage != HwStage::Gs {
        return 0;
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
    let mut fresh = || {
        next += 1;
        Value(next - 1)
    };
    let mut renamed: std::collections::HashMap<Value, Value> = Default::default();
    let mut gone: Vec<(usize, usize)> = Vec::new();

    // EXEC from the invocation count, in the entry block: its values become
    // EXEC's entry values.
    let halves: &[u8] = if p.wave64 { &[EXEC, EXEC + 1] } else { &[EXEC] };
    let mut entry_exec = Vec::new();
    for &r in halves {
        let v = match p.inputs.iter().find(|i| i.0 == Reg::S(r)) {
            Some(i) => i.1,
            None => {
                let v = fresh();
                p.inputs.push((Reg::S(r), v));
                v
            }
        };
        entry_exec.push((r, v));
    }
    if let Some(block) = p.blocks.first() {
        for (i, inst) in block.insts.iter().enumerate() {
            let to_exec = matches!(&inst.fields,
                Fields::Sop { dst: Some(Operand::Scalar { code: EXEC, .. }), .. });
            let ok = match (inst.name(), &inst.fields) {
                ("s_bfm_b64", _) => true,
                // `s_cselect_b64 exec, -1, exec`: a full wave keeps all lanes.
                ("s_cselect_b64", Fields::Sop { src, .. }) => {
                    matches!(src[..], [Operand::Const(193), Operand::Scalar { code: EXEC, .. }])
                }
                _ => false,
            };
            if !(to_exec && ok) {
                continue;
            }
            for &(r, v) in &inst.writes {
                if let Some(&(_, e)) = entry_exec.iter().find(|e| Reg::S(e.0) == r) {
                    renamed.insert(v, e);
                }
            }
            gone.push((0, i));
        }
    }

    // Loads through user-data dwords 0-1 (s8, s9 & 0xff, and copies).
    let entry = |reg: Reg, p: &Program| p.inputs.iter().find(|i| i.0 == reg).map(|i| i.1);
    if let (Some(low), Some(high)) = (entry(Reg::S(8), p), entry(Reg::S(9), p)) {
        let (mut lows, mut highs, raw) = (vec![low], Vec::new(), vec![high]);
        let mut raws = raw;
        loop {
            let mut changed = false;
            for inst in p.blocks.iter().flat_map(|b| &b.insts) {
                let Fields::Sop { dst: Some(Operand::Scalar { code: d, .. }), src, .. } = &inst.fields else {
                    continue;
                };
                let Some(out) = inst.write(Reg::S(*d)) else { continue };
                let read = |op: &Operand| match *op {
                    Operand::Scalar { code, .. } if code != NULL => inst.read(Reg::S(code)),
                    _ => None,
                };
                let set = match (inst.name(), &src[..]) {
                    ("s_mov_b32", [s]) => match read(s) {
                        Some(v) if lows.contains(&v) => Some(&mut lows),
                        Some(v) if highs.contains(&v) => Some(&mut highs),
                        Some(v) if raws.contains(&v) => Some(&mut raws),
                        _ => None,
                    },
                    ("s_and_b32", [x, y]) => {
                        let masked = |a: &Operand, m: &Operand| {
                            matches!(*m, Operand::Literal(0xff)) && read(a).is_some_and(|v| raws.contains(&v))
                        };
                        (masked(x, y) || masked(y, x)).then_some(&mut highs)
                    }
                    _ => None,
                };
                if let Some(set) = set
                    && !set.contains(&out)
                {
                    set.push(out);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        let mut k = 0u8;
        for (b, block) in p.blocks.iter().enumerate() {
            for (i, inst) in block.insts.iter().enumerate() {
                let Fields::Smem { base: Operand::Scalar { code: base, .. }, .. } = inst.fields else {
                    continue;
                };
                if !inst.name().starts_with("s_load_dword")
                    || !inst.read(Reg::S(base)).is_some_and(|v| lows.contains(&v))
                    || !inst.read(Reg::S(base + 1)).is_some_and(|v| highs.contains(&v))
                {
                    continue;
                }
                for &(_, v) in &inst.writes {
                    let code = DRIVER_TABLE.saturating_add(k);
                    k = k.saturating_add(1);
                    let value = fresh();
                    p.inputs.push((Reg::S(code), value));
                    renamed.insert(v, value);
                }
                gone.push((b, i));
            }
        }
    }

    if gone.is_empty() {
        return 0;
    }
    // (a renamed value may be read by a rewritten instruction: follow to the end)
    let target = |mut v: Value| {
        while let Some(&to) = renamed.get(&v) {
            v = to;
        }
        v
    };
    for block in &mut p.blocks {
        for phi in &mut block.phis {
            phi.args.iter_mut().for_each(|a| *a = target(*a));
        }
        for inst in &mut block.insts {
            inst.reads.iter_mut().for_each(|r| r.1 = target(r.1));
        }
    }
    for (b, block) in p.blocks.iter_mut().enumerate() {
        let mut i = 0;
        block.insts.retain(|_| {
            i += 1;
            !gone.contains(&(b, i - 1))
        });
    }
    gone.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::StageIo;

    const S_ENDPGM: u32 = 0xBF81_0000;

    fn gs(words: &[u32]) -> NativeProgram {
        NativeProgram {
            stage: HwStage::Gs,
            wave64: true,
            vgprs: 32,
            layout: vec![],
            used: vec![],
            io: StageIo::None,
            code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
        }
    }

    /// The voxelizer's geometry prologue: EXEC from s3, and the output
    /// place loaded through s8-s9.
    const PROLOGUE: [u32; 10] = [
        0x9400_FF03, 0x0008_0008, // s_bfe_i32 s0, s3, 8 bits at 8
        0x92FE_8000, //             s_bfm_b64 exec, s0, 0
        0xBF06_C000, //             s_cmp_eq_u32 s0, 64
        0x85FE_7EC1, //             s_cselect_b64 exec, -1, exec
        0x8701_FF09, 0x0000_00FF, // s_and_b32 s1, s9, 0xff
        0xBE80_0308, //             s_mov_b32 s0, s8
        0xF400_0000, 0xFA00_00B8, // s_load_dword s0, s[0:1], 0xb8
    ];

    #[test]
    fn the_exec_prologue_and_driver_table_load_go() {
        let mut words = PROLOGUE.to_vec();
        words.extend([
            0x7E00_0200, // v_mov_b32 v0, s0
            S_ENDPGM,
        ]);
        let native = gs(&words);
        let mut p = crate::ir::build(&native.code, true, 32).unwrap();
        assert_eq!(prepare(&mut p, &native), 3);
        let names: Vec<_> = p.blocks[0].insts.iter().map(|i| i.name()).collect();
        assert!(!names.contains(&"s_bfm_b64") && !names.contains(&"s_cselect_b64"));
        assert!(!names.contains(&"s_load_dword"));
        // The move reads EXEC as it entered and the load's dword as an entry value.
        let mov = p.blocks[0].insts.iter().find(|i| i.name() == "v_mov_b32").unwrap();
        let exec = p.inputs.iter().find(|i| i.0 == Reg::S(EXEC)).unwrap().1;
        assert_eq!(mov.read(Reg::S(EXEC)), Some(exec));
        let s0 = mov.read(Reg::S(0)).unwrap();
        assert!(p.inputs.iter().any(|i| i == &(Reg::S(DRIVER_TABLE), s0)));
    }

    #[test]
    fn other_stages_are_left_alone() {
        let mut native = gs(&[PROLOGUE.to_vec(), vec![S_ENDPGM]].concat());
        native.stage = HwStage::Vs;
        let mut p = crate::ir::build(&native.code, true, 32).unwrap();
        let before = p.clone();
        assert_eq!(prepare(&mut p, &native), 0);
        assert_eq!(p, before);
    }
}
