//! Eligibility and native LDS analysis for the opt-in CS_Visibility split.

use super::{Parts, all_lane_blocks};
use crate::gfx10::{Fields, Operand};
use crate::io::Input;
use crate::ir::{Inst, Program, Reg};
use crate::native::{HwStage, NativeProgram, ResourceType, RootType, StageIo, UsedResource};
use crate::structure::{Atom, Bool, LaneExit, Structured};

/// Only the audited Visibility native program is eligible for this experiment.
/// The digest covers raw GFX10 instructions, independent of container packing.
const VISIBILITY_NATIVE_DIGEST: [u8; 16] = [
    0x07, 0x9e, 0xfc, 0x2d, 0x6c, 0xad, 0x71, 0xdd, 0x00, 0x2a, 0xb6, 0x1e, 0xb0, 0xd7, 0xbe, 0x3b,
];
pub(super) const VISIBILITY_SNAPSHOT_SLOT: u32 = 1;

pub(super) fn split_lds_on() -> bool {
    #[cfg(test)]
    if let Some(on) = TEST_SPLIT_LDS.get() {
        return on;
    }
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_SPLIT_LDS").is_some_and(|v| v == "1"))
}

#[cfg(test)]
thread_local! {
    static TEST_SPLIT_LDS: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

fn visibility_identity(native: &NativeProgram) -> bool {
    let StageIo::Cs(cs) = native.io else {
        return false;
    };
    if native.stage != HwStage::Cs
        || !native.wave64
        || native.vgprs != 23
        || cs.threads != [64, 1, 1]
        || cs.rsrc2 != 0x808e
        || native.code.len() != 1792
        || crate::emit::container::hash(&native.code) != VISIBILITY_NATIVE_DIGEST
    {
        return false;
    }

    // The host gives the only guest CBV a private, immutable copy of the
    // descriptor table's b7. The verifier can then treat its loads as uniform.
    let cbv = UsedResource {
        kind: ResourceType::Cbv,
        root: RootType::Table,
        table_offset_dwords: 96,
        lower_bound: 7,
        stride: 0,
        dwords: 4,
        user_data: 4,
        slot: 0,
    };
    native
        .used
        .iter()
        .filter(|r| r.kind == ResourceType::Cbv)
        .eq([&cbv])
        && native
            .layout
            .iter()
            .filter(|r| r.user_data == 4 && r.table_offset_dwords == 96)
            .eq([&crate::native::RootParam {
                root: RootType::Table,
                table_offset_dwords: 96,
                sgprs: 1,
                base_register: 7,
                user_data: 4,
                slot: 0,
            }])
}

pub(super) fn visibility_initializer(x: &Parts) -> bool {
    if !visibility_identity(x.native) {
        return false;
    }
    // Certification was verified for the shallow, semantics-preserving EXEC
    // skip removal. Exclude all other fallback IR, even for this native code.
    if !crate::spill::build(x.native)
        .ok()
        .and_then(|p| crate::ir::without_skips(&p, false))
        .is_some_and(|p| p == *x.p)
    {
        return false;
    }

    // At 0x20, v0 becomes GroupId.x * 64 + ThreadIdInGroup.x. At
    // 0x58, the compare admits only global X zero. The same predicate
    // guards the three paired stores at 0x68..0x78 that zero slots 0..5.
    // With one group in X, every group has exactly one such writer.
    let entry = &x.p.blocks[0];
    let at = |offset| {
        entry
            .insts
            .iter()
            .enumerate()
            .find(|(_, i)| i.machine.offset == offset)
    };
    let Some((index_at, index)) = at(0x20) else {
        return false;
    };
    let Some(global_x) = index.write(Reg::V(0)) else {
        return false;
    };
    let Some((cmp_at, cmp)) = at(0x58) else {
        return false;
    };
    let Some(mask) = cmp.write(Reg::S(0)) else {
        return false;
    };
    let Some((_, zero)) = at(0x64) else {
        return false;
    };
    let Some(address_and_data) = zero.write(Reg::V(1)) else {
        return false;
    };
    x.st.reach[0] == Bool::TRUE
        && x.st.guards[0][index_at].is_none()
        && x.st.guards[0][cmp_at].is_none()
        && index
            .read(Reg::S(7))
            .is_some_and(|group_x| x.io.inputs.contains(&(group_x, Input::GroupId(0))))
        && index
            .read(Reg::V(0))
            .is_some_and(|thread_x| x.io.inputs.contains(&(thread_x, Input::ThreadIdInGroup(0))))
        && cmp.read(Reg::V(0)) == Some(global_x)
        && [(0x68, 4), (0x70, 2), (0x78, 0)]
            .into_iter()
            .all(|(offset, slot)| {
                let Some((i, store)) = at(offset) else {
                    return false;
                };
                matches!(store.fields, Fields::Ds { offset0, offset1, gds: false, .. }
                if offset0 == slot && offset1 == slot + 1)
                    && store.read(Reg::V(1)) == Some(address_and_data)
                    && x.st.guards[0][i].and_then(|g| x.st.bools.node(g))
                        == Some((Atom::Lane(mask), Bool::FALSE, Bool::TRUE))
            })
}

/// Try a split with per-access meetings if a branch reads a fixed LDS
/// dword that can be snapshotted for the whole group. Only the analysis
/// treats the branch as uniform. The original predicate is still emitted.
pub(super) fn lds_snapshot_meetings(
    p: &Program,
    st: &Structured,
    all: &[bool],
    lds_bytes: u32,
) -> Option<(Vec<bool>, Vec<(bool, bool)>)> {
    // The certificate proves convergence. Keep the candidate to operations
    // whose lane and memory behavior this lowering already preserves.
    if p.blocks
        .iter()
        .flat_map(|b| &b.insts)
        .any(|i| match i.fields {
            Fields::Ds { gds, .. } => {
                gds || !matches!(
                    i.name(),
                    "ds_read_b32"
                        | "ds_read2_b32"
                        | "ds_write_b32"
                        | "ds_write2_b32"
                        | "ds_add_u32"
                        | "ds_add_rtn_u32"
                )
            }
            _ => matches!(i.name(), "v_readfirstlane_b32" | "s_barrier"),
        })
    {
        return None;
    }
    let mut candidates =
        (0..st.exits.len()).filter(|&b| all[b] && lds_snapshot_branch(p, st, b, lds_bytes));
    let snapshot = candidates.next()?;
    if candidates.next().is_some() {
        return None;
    }
    let mut model = st.clone();
    if let LaneExit::Branch { when, .. } = &mut model.exits[snapshot] {
        *when = Bool::TRUE;
    }
    let all = all_lane_blocks(&model)?;
    if p.blocks.iter().enumerate().any(|(b, block)| {
        !all[b]
            && block
                .insts
                .iter()
                .any(|inst| matches!(inst.fields, Fields::Ds { .. }))
    }) {
        return None;
    }
    let mut meet = vec![(false, false); all.len()];
    meet[snapshot].1 = true;
    Some((all, meet))
}

fn lds_snapshot_branch(p: &Program, st: &Structured, b: usize, lds_bytes: u32) -> bool {
    let LaneExit::Branch { when, .. } = st.exits[b] else {
        return false;
    };
    let mut lane = None;
    for atom in st.bools.support(when) {
        match atom {
            Atom::Lane(v) if lane.is_none() => lane = Some(v),
            Atom::Uniform(_) => {}
            _ => return false,
        }
    }
    let Some(value) = lane else { return false };
    let ins = &p.blocks[b].insts;
    let unguarded_cmp = |(i, cmp): &(usize, &Inst)| {
        cmp.name() == "v_cmp_eq_u32"
            && st.guards[b][*i].is_none()
            && cmp.writes.iter().any(|&(_, v)| v == value)
    };
    let Some((cmp_at, cmp)) = ins.iter().enumerate().find(unguarded_cmp) else {
        return false;
    };
    let Fields::Valu(v) = &cmp.fields else {
        return false;
    };
    if !plain_lds_valu(v) {
        return false;
    }
    let reg = match v.src.as_slice() {
        [
            Operand::Vector { reg, dwords: 1 },
            Operand::Const(_) | Operand::Literal(_),
        ]
        | [
            Operand::Const(_) | Operand::Literal(_),
            Operand::Vector { reg, dwords: 1 },
        ] => Reg::V(*reg),
        _ => return false,
    };
    let Some(input) = cmp.read(reg) else {
        return false;
    };
    let mut ds = ins
        .iter()
        .enumerate()
        .filter(|(_, i)| matches!(i.fields, Fields::Ds { .. }));
    let Some((read_at, read)) = ds.next() else {
        return false;
    };
    if ds.next().is_some()
        || read_at >= cmp_at
        || read.name() != "ds_read_b32"
        || st.guards[b][read_at].is_some()
        || read.write(reg) != Some(input)
    {
        return false;
    }
    let Fields::Ds {
        addr: Some(Operand::Vector { reg, dwords: 1 }),
        offset0,
        offset1,
        gds: false,
        ..
    } = read.fields
    else {
        return false;
    };
    let reg = Reg::V(reg);
    let Some(address) = read.read(reg) else {
        return false;
    };
    let Some((i, mov)) = ins[..read_at]
        .iter()
        .enumerate()
        .find(|(_, mov)| mov.write(reg) == Some(address))
    else {
        return false;
    };
    let Fields::Valu(v) = &mov.fields else {
        return false;
    };
    if mov.name() != "v_mov_b32" || st.guards[b][i].is_some() || !plain_lds_valu(v) {
        return false;
    }
    let base = match v.src.as_slice() {
        [Operand::Const(k @ 128..=192)] => u32::from(*k - 128),
        [Operand::Literal(k)] => *k,
        _ => return false,
    };
    let offset = u32::from(u16::from_le_bytes([offset0, offset1]));
    base.checked_add(offset).is_some_and(|byte| {
        byte.is_multiple_of(4) && byte.checked_add(4).is_some_and(|end| end <= lds_bytes)
    })
}

fn plain_lds_valu(v: &crate::gfx10::Valu) -> bool {
    v.sdwa.is_none()
        && v.dpp.is_none()
        && v.neg == 0
        && v.abs == 0
        && v.neg_hi == 0
        && v.opsel == 0
        && v.opsel_hi == 0
        && !v.clamp
        && v.omod == 0
}

#[cfg(test)]
mod tests {
    use super::super::{TEST_HOST_WAVE, lds_meetings};
    use super::ResourceType;
    use crate::{ir, structure};

    const S_ENDPGM: u32 = 0xBF81_0000;

    #[test]
    #[ignore = "requires a local CS_Visibility capture in VICTORY_VISIBILITY_CAPTURE"]
    fn captured_visibility_certifies_a_wave32_split() {
        use crate::{Container, FourCC, native_programs};

        let wave_bounds = |dxil: &[u8]| {
            let container = Container::parse(dxil).unwrap();
            let psv = container.part(FourCC::PSV0).unwrap().data;
            // A four-byte info length precedes the PSV wave bounds.
            [20, 24].map(|offset| crate::dxbc::u32_at(psv, offset).unwrap())
        };
        let path =
            std::env::var_os("VICTORY_VISIBILITY_CAPTURE").expect("local CS_Visibility capture");
        let bytes = std::fs::read(path).unwrap();
        let c = Container::parse(&bytes).unwrap();
        let native = native_programs(c.part(FourCC::DXIL).unwrap().data)
            .unwrap()
            .remove(0);
        assert!(super::visibility_identity(&native));
        let mut modified = native.clone();
        modified.code[0] ^= 1;
        assert!(!super::visibility_identity(&modified));
        let mut modified = native;
        modified
            .used
            .iter_mut()
            .find(|r| r.kind == ResourceType::Cbv)
            .unwrap()
            .slot = 1;
        assert!(!super::visibility_identity(&modified));

        use crate::emit::shader::TEST_VIEW_LIMIT;
        TEST_HOST_WAVE.set(Some(32));
        for limit in [0, 999_999] {
            TEST_VIEW_LIMIT.set(Some(limit));
            super::TEST_SPLIT_LDS.set(Some(false));
            let baseline = crate::emit::translate(&bytes).unwrap();
            assert_eq!(wave_bounds(&baseline.dxil), [64, 64]);
            assert!(!baseline.dispatch_x_one);

            super::TEST_SPLIT_LDS.set(Some(true));
            let result = crate::emit::translate(&bytes).unwrap();
            if crate::emit::group_sync() == 0 {
                assert!(!result.dispatch_x_one);
                assert_eq!(result.dxil, baseline.dxil);
                continue;
            }
            assert_eq!(
                wave_bounds(&result.dxil),
                [32, 32],
                "host heap limit {limit}"
            );
            assert!(result.dispatch_x_one);
            #[cfg(windows)]
            {
                use crate::emit::validator::Validator;
                let mut dxil = result.dxil;
                eprintln!(
                    "CS_Visibility Wave32, {} DXIL bytes, heap limit {limit}",
                    dxil.len()
                );
                let mut validator = Validator::load(Validator::default_path().as_deref()).unwrap();
                validator.validate(&mut dxil).unwrap();
            }
        }
        TEST_HOST_WAVE.set(None);
        super::TEST_SPLIT_LDS.set(None);
        TEST_VIEW_LIMIT.set(None);
    }

    #[test]
    fn split_lds_snapshot_models_a_branch_without_changing_its_predicate() {
        for (address, load, first_lane, accepted) in [
            (0x7e02_0280u32, 0xd8d8_0000, false, true),
            (0x7e02_0300, 0xd8d8_0000, false, false),
            (0x7e02_0280, 0xd8e8_0000, false, false), // ds_read_u8
            (0x7e02_0280, 0xd8d8_0000, true, false),
        ] {
            let mut words = vec![
                address, // v_mov_b32 v1, 0 or v0
                0xd8d8_0004,
                0x0100_0001,                         // ds_read_b32 v1, v1 offset:4
                0x7d84_0280,                         // v_cmp_eq_u32 vcc, 0, v1
                0xbf87_0004 + u32::from(first_lane), // s_cbranch_vccnz end
                load,
                0x0200_0001, // ds_read_b32 or ds_read_u8 v2, v1
                0xd834_0000,
                0x0000_0201, // ds_write_b32 v1, v2
            ];
            if first_lane {
                words.push(0x7e00_0502); // v_readfirstlane_b32 s0, v2
            }
            words.push(S_ENDPGM);
            let code: Vec<_> = words.into_iter().flat_map(u32::to_le_bytes).collect();
            let p = ir::build(&code, true, 8).unwrap();
            let st = structure::structure(&p).unwrap();
            let orig = st.exits[0];
            let all = super::all_lane_blocks(&st).unwrap();
            assert!(lds_meetings(&p, &st, &all).is_none());
            let proposed = super::lds_snapshot_meetings(&p, &st, &all, 64);
            assert_eq!(proposed.is_some(), accepted);
            if let Some((all, meet)) = proposed {
                assert!(all.into_iter().all(|b| b));
                assert!(meet[0].1);
                assert_eq!(orig, st.exits[0]);
            }
        }
    }
}
