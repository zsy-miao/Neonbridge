//! A console wave64 compute program on a host of 32-lane waves: a wave reduction inside a
//! per-lane branch, with that branch taken wave-wide ([`crate::structure::structure_wide`])
//! so that the two host waves can exchange their halves inside it; and programs in the
//! paired form (one host wave for each console wave, two lanes a thread: `super::paired`),
//! whose LDS, lane reads and masks cross the halves without barriers, checked against what
//! the console's 64 lanes compute. Lowered, validated by `dxil.dll` when present, and run on
//! the host GPU when it takes waves of 32 (WARP's are 4 lanes: neither form runs there).

use std::collections::HashSet;

use super::host::{Dispatch, Host, Root, View};
use super::{HostShader, lower, validator};
use crate::native::{
    ComputeIo, HwStage, NativeProgram, ResourceType, RootParam, RootType, StageIo, UsedResource,
};
use crate::{ShaderError, io, ir, resource, structure};

const S_ENDPGM: u32 = 0xBF81_0000;

/// 64 threads in one console wave. Each thread t holds the bit `1 << (t >> 1)`; the threads
/// below s0 (a root constant) OR theirs over the wave inside `if (t < s0) { ... }` and store
/// the result to dword 0 of the raw UAV.
/// `skip`: the branch past the body when no thread is below s0: `s_cbranch_execz`, or
/// `s_cbranch_scc0` on the SCC `s_and_saveexec` sets ("some lane is left").
fn program(skip: u32) -> NativeProgram {
    let mut words = vec![
        0x7E0A_0281, // v_mov_b32 v5, 1
        0x2C0C_0081, // v_lshrrev_b32 v6, 1, v0
        0x340E_0B06, // v_lshlrev_b32 v7, v6, v5
        0x7D88_0000, // v_cmp_gt_u32 vcc, s0, v0
        0xBE8C_246A, // s_and_saveexec_b64 s[12:13], vcc
        skip | 23,   // the branch to the restore of EXEC
    ];
    words.extend_from_slice(&super::wave::WAVE_OR_V7_TO_S7);
    words.extend_from_slice(&[
        0x7E00_0207, // v_mov_b32 v0, s7
        0x38 << 26 | 0x1C << 18,
        0x80 << 24 | 2 << 16, // buffer_store_dword v0, off, s[8:11], 0
        0xBEFE_040C, // s_mov_b64 exec, s[12:13]
    ]);
    native(&words)
}

/// A wave64 compute program of 64 threads (one console wave; the thread index in v0) with 4
/// root constants in s0..s3 and a raw UAV's descriptor in s[8:11], then `body`.
fn native(body: &[u32]) -> NativeProgram {
    native_of(body, true, 64)
}

/// The same for a console of 32-lane waves when `wave64` is not set, with `threads` threads.
fn native_of(body: &[u32], wave64: bool, threads: u32) -> NativeProgram {
    native_with(body, wave64, threads, 0)
}

/// [`native_of`] with `lds` bytes of LDS (512-byte granules).
fn native_with(body: &[u32], wave64: bool, threads: u32, lds: u32) -> NativeProgram {
    let mut words = vec![
        0xBE85_0380, // s_mov_b32 s5, 0
        0x3D << 26 | 0x02 << 18 | 8 << 6 | 2,
        125 << 25, // s_load_dwordx4 s[8:11], s[4:5], 0
    ];
    words.extend_from_slice(body);
    words.push(S_ENDPGM);
    NativeProgram {
        stage: HwStage::Cs,
        wave64,
        vgprs: 16,
        layout: vec![
            RootParam {
                root: RootType::Constants,
                table_offset_dwords: 0,
                sgprs: 4,
                base_register: 0,
                user_data: 0,
                slot: 0,
            },
            RootParam {
                root: RootType::Table,
                table_offset_dwords: 0,
                sgprs: 1,
                base_register: 0,
                user_data: 4,
                slot: 1,
            },
        ],
        used: vec![UsedResource {
            kind: ResourceType::Uav,
            root: RootType::Table,
            table_offset_dwords: 0,
            lower_bound: 0,
            stride: 0,
            dwords: 4,
            user_data: 4,
            slot: 1,
        }],
        io: StageIo::Cs(ComputeIo {
            rsrc2: 5 << 1 | lds.div_ceil(512) << 15, // 5 user SGPRs: s0..s4
            threads: [threads, 1, 1],
        }),
        code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
    }
}

/// `program()` for a host of 32-lane waves, with the branches closing `wide` taken wave-wide.
fn lowered(native: &NativeProgram, p: &ir::Program, wide: &HashSet<usize>) -> Result<HostShader, ShaderError> {
    lowered_on(native, p, wide, 32)
}

/// [`lowered`] for a host whose waves have at most `host_wave` lanes.
fn lowered_on(
    native: &NativeProgram,
    p: &ir::Program,
    wide: &HashSet<usize>,
    host_wave: u32,
) -> Result<HostShader, ShaderError> {
    let st = structure::structure_wide(p, wide).expect("structure");
    let res = resource::track(p, &native.layout, native.stage, Some(&st)).expect("resource::track");
    let io = io::raise(p, native).expect("io::raise");
    let was = lower::TEST_HOST_WAVE.replace(Some(host_wave));
    let lowered = lower::lower(&lower::Parts {
        native,
        p,
        st: &st,
        io: &io,
        res: &res,
        inputs: Vec::new(),
        outputs: Vec::new(),
        prims: Vec::new(),
        mesh_stride: 0,
        gs: None,
    });
    lower::TEST_HOST_WAVE.set(was);
    let lowered = lowered?;
    Ok(HostShader {
        dispatch_x_one: false,
        stage: HwStage::Cs,
        dxil: lowered.dxil,
        user_data: lowered.user_data,
        samplers: lowered.samplers,
        static_samplers: Vec::new(),
        root_cbvs: lowered.root_cbvs,
        root_srvs: lowered.root_srvs,
        root_uavs: lowered.root_uavs,
        root_indexed_views: res.root_indexed_views,
    })
}

/// 64 threads in one console wave: thread t holds the 16-bit float with bits 0x3C00 + 37 t
/// (1.0 and up, with odd mantissas, so that additions round); all of them store the wave's sum
/// (`WaveActiveSum` of halves) to dword 0. With the console's sum (its tree) and the terms'
/// sum added in a row, which differs from it.
fn half_sum_program() -> (NativeProgram, u16, u16) {
    let mut words = vec![
        0x340C_0085, // v_lshlrev_b32 v6, 5, v0
        0x340E_0082, // v_lshlrev_b32 v7, 2, v0
        0x4A0E_0F06, // v_add_nc_u32 v7, v6, v7
        0x4A0E_0F00, // v_add_nc_u32 v7, v0, v7
        0x4A0E_0EFF,
        0x0000_3C00, // v_add_nc_u32 v7, 0x3c00, v7
    ];
    // (the wave OR's words with v_add_f16 for v_or_b32: opcode 50 for 28)
    words.extend(super::wave::WAVE_OR_V7_TO_S7.iter().map(|&w| if w >> 25 == 28 { w & 0x01FF_FFFF | 50 << 25 } else { w }));
    words.extend_from_slice(&[
        0x7E00_0207, // v_mov_b32 v0, s7
        0x38 << 26 | 0x1C << 18,
        0x80 << 24 | 2 << 16, // buffer_store_dword v0, off, s[8:11], 0
    ]);
    // the console's sum: lanes 2k and 2k+1, then pairs, ... each addition rounded to 16 bits
    // (to nearest, ties to even); values here are normal numbers
    let wide = |h: u16| f32::from_bits((u32::from(h >> 10) + 112) << 23 | u32::from(h & 0x3FF) << 13);
    let narrow = |f: f32| {
        let b = f.to_bits();
        let h = ((b >> 23) - 112) << 10 | (b >> 13 & 0x3FF);
        let rest = b & 0x1FFF;
        (h + u32::from(rest > 0x1000 || rest == 0x1000 && h & 1 == 1)) as u16
    };
    let add = |a: u16, b: u16| narrow(wide(a) + wide(b));
    let terms: Vec<u16> = (0..64u16).map(|t| 0x3C00 + 37 * t).collect();
    let mut tree = terms.clone();
    while tree.len() > 1 {
        tree = tree.chunks(2).map(|c| add(c[0], c[1])).collect();
    }
    let in_a_row = terms.iter().copied().reduce(add).unwrap();
    (native(&words), tree[0], in_a_row)
}

#[test]
fn a_sum_of_halves_over_the_wave_is_the_consoles_tree_bit_for_bit() {
    let (native, tree, in_a_row) = half_sum_program();
    let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
    assert_eq!(p.blocks.iter().flat_map(|b| &b.insts).filter(|i| i.name() == "v_add_f16").count(), 6);
    let shader = lowered(&native, &p, &HashSet::new()).expect("lower");
    if let Ok(mut v) = validator::Validator::load(validator::Validator::default_path().as_deref()) {
        let mut copy = shader.dxil.clone();
        v.validate(&mut copy).expect("DXIL validation failed");
    }
    assert_ne!(tree, in_a_row, "these terms tell the tree from another order");
    let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(host) = Host::new(false) else {
        eprintln!("no SM 6.6 hardware device; not run");
        return;
    };
    let (root, pso) = match host.compute_pipeline(&shader) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("the hardware device does not take this program (waves of 32?): {e}; not run");
            return;
        }
    };
    let ud = vec![0u32; shader.user_data_dwords().max(1) as usize];
    let out = host
        .dispatch(
            &root,
            &pso,
            &Dispatch {
                roots: vec![(0, Root::Constants(ud))],
                views: &[View::RawUav(1)],
                samplers: &[],
                groups: [1, 1, 1],
            },
        )
        .unwrap_or_else(|e| panic!("dispatch failed: {e}"));
    // (the high half is lane 0's term's: zero)
    assert_eq!(out[0][0], u32::from(tree), "tree {tree:#x}, in a row {in_a_row:#x}");
    eprintln!("ran on the hardware device: the sum {tree:#x} of the console's tree");
}

/// The console's 16-bit wave sum where the lanes at or past s0 have left the program (a
/// thread past the end of the data): on the console they add +0, and after each step of the
/// tree they hold their run's partial sum all the same. On a host of 64-lane waves (AMD) the
/// lanes that are here read each partner run's sum from a lane of it that is here.
#[test]
fn a_sum_of_halves_where_lanes_have_left_is_the_consoles_tree_bit_for_bit() {
    let mut words = vec![
        0x7DA8_0000, // v_cmpx_gt_u32 exec, s0, v0: the lanes below s0 stay
        0, // s_cbranch_execz to the end (filled in below)
        0x340C_0085, // v_lshlrev_b32 v6, 5, v0
        0x340E_0082, // v_lshlrev_b32 v7, 2, v0
        0x4A0E_0F06, // v_add_nc_u32 v7, v6, v7
        0x4A0E_0F00, // v_add_nc_u32 v7, v0, v7
        0x4A0E_0EFF,
        0x0000_3C00, // v_add_nc_u32 v7, 0x3c00, v7
    ];
    words.extend(super::wave::WAVE_OR_V7_TO_S7.iter().map(|&w| if w >> 25 == 28 { w & 0x01FF_FFFF | 50 << 25 } else { w }));
    words.extend_from_slice(&[
        0x7E00_0207, // v_mov_b32 v0, s7
        0x38 << 26 | 0x1C << 18,
        0x80 << 24 | 2 << 16, // buffer_store_dword v0, off, s[8:11], 0
    ]);
    // past every word after it, to the program's end
    words[1] = 0xBF88_0000 | (words.len() as u32 - 2);
    let native = native(&words);
    let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
    let shader = lowered_on(&native, &p, &HashSet::new(), 64).expect("lower");
    if let Ok(mut v) = validator::Validator::load(validator::Validator::default_path().as_deref()) {
        let mut copy = shader.dxil.clone();
        v.validate(&mut copy).expect("DXIL validation failed");
    }
    let wide = |h: u16| {
        if h == 0 {
            0.0
        } else {
            f32::from_bits((u32::from(h >> 10) + 112) << 23 | u32::from(h & 0x3FF) << 13)
        }
    };
    let narrow = |f: f32| {
        if f == 0.0 {
            return 0;
        }
        let b = f.to_bits();
        let h = ((b >> 23) - 112) << 10 | (b >> 13 & 0x3FF);
        let rest = b & 0x1FFF;
        (h + u32::from(rest > 0x1000 || rest == 0x1000 && h & 1 == 1)) as u16
    };
    let add = |a: u16, b: u16| narrow(wide(a) + wide(b));
    let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let mut ran = false;
    for stay in [64u32, 63, 40, 33, 32, 17, 5, 1] {
        let terms: Vec<u16> = (0..64u32)
            .map(|t| if t < stay { (0x3C00 + 37 * t) as u16 } else { 0 })
            .collect();
        let mut tree = terms.clone();
        while tree.len() > 1 {
            tree = tree.chunks(2).map(|c| add(c[0], c[1])).collect();
        }
        let Some(got) = on_the_card(&shader, [stay, 0]) else {
            return;
        };
        ran = true;
        // (the high half is lane 0's term's: zero)
        assert_eq!(got, u32::from(tree[0]), "{stay} lanes stay: got {got:#x}, the console's tree {:#x}", tree[0]);
    }
    if ran {
        eprintln!("ran on the hardware device: the console's tree with lanes gone, in every case");
    }
}

/// One `v_add_f16` whose exact result lies between two halves: 1 + 3/1024 (0x3C03) plus 2
/// (0x4000) is 3 + 1.5 steps of that range, which the console rounds up to 0x4202. The
/// host's `f32tof16` may give 0x4201 (toward zero); with `VICTORY_F16_NEAREST=1` the
/// translation gives the console's whatever the host does.
#[test]
fn a_16_bit_sum_rounds_to_nearest_when_asked_to() {
    let words = [
        0x7E0E_02FF,
        0x0000_3C03, // v_mov_b32 v7, 0x3c03
        0x7E0C_02FF,
        0x0000_4000, // v_mov_b32 v6, 0x4000
        0x6400_0D07, // v_add_f16 v0, v7, v6
        0x38 << 26 | 0x1C << 18,
        0x80 << 24 | 2 << 16, // buffer_store_dword v0, off, s[8:11], 0
    ];
    let native = native(&words);
    let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
    assert!(p.blocks.iter().flat_map(|b| &b.insts).any(|i| i.name() == "v_add_f16"));
    let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    for nearest in [false, true] {
        let was = lower::TEST_F16_NEAREST.replace(Some(nearest));
        let shader = lowered(&native, &p, &HashSet::new());
        lower::TEST_F16_NEAREST.set(was);
        let shader = shader.expect("lower");
        if let Ok(mut v) = validator::Validator::load(validator::Validator::default_path().as_deref()) {
            let mut copy = shader.dxil.clone();
            v.validate(&mut copy).expect("DXIL validation failed");
        }
        let Ok(host) = Host::new(false) else {
            eprintln!("no SM 6.6 hardware device; not run");
            return;
        };
        let Ok((root, pso)) = host.compute_pipeline(&shader) else {
            eprintln!("the hardware device does not take this program; not run");
            return;
        };
        let ud = vec![0u32; shader.user_data_dwords().max(1) as usize];
        let out = host
            .dispatch(
                &root,
                &pso,
                &Dispatch {
                    roots: vec![(0, Root::Constants(ud))],
                    views: &[View::RawUav(1)],
                    samplers: &[],
                    groups: [1, 1, 1],
                },
            )
            .unwrap_or_else(|e| panic!("dispatch failed: {e}"));
        let got = out[0][0] & 0xFFFF;
        eprintln!("v_add_f16 0x3c03 + 0x4000 on the hardware device, nearest {nearest}: {got:#x}");
        if nearest {
            assert_eq!(got, 0x4202);
        } else {
            assert!(got == 0x4201 || got == 0x4202, "{got:#x}");
        }
    }
}

#[test]
fn a_wave_or_inside_a_lane_branch_needs_the_branch_wave_wide() {
    for (skip, name) in [(0xBF88_0000, "s_cbranch_execz"), (0xBF84_0000, "s_cbranch_scc0")] {
        a_wave_or_inside(skip, name);
    }
}

fn a_wave_or_inside(skip: u32, name: &str) {
    let native = program(skip);
    let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
    let branch = (0..p.blocks.len())
        .find(|&b| p.blocks[b].insts.last().is_some_and(|i| i.name() == name))
        .expect("the branch");
    // per lane, the two host waves cannot meet inside the branch
    lower::take_needs();
    let per_lane = lowered(&native, &p, &HashSet::new());
    assert!(
        matches!(&per_lane, Err(ShaderError::Unsupported { what, .. }) if *what == "a wave reduction where not every lane runs"),
        "{:?}",
        per_lane.err()
    );
    // ... and the lowering names the block, around which that branch is the one to widen
    let needs = lower::take_needs();
    let st = structure::structure(&p).expect("structure");
    assert_eq!(lower::branches_around(&st, &needs), Some(vec![branch]));
    // (taken, past the body, when every lane's bit is clear)
    assert_eq!(st.wave_any[branch], Some(false));
    // wave-wide, every lane runs the reduction (EXEC stays a guard on what it does there)
    let wide = HashSet::from([branch]);
    let st = structure::structure_wide(&p, &wide).expect("structure");
    let inside = match p.blocks[branch].exit {
        ir::Exit::Branch { not_taken, .. } => not_taken,
        _ => unreachable!(),
    };
    assert!(st.guards[inside].iter().any(Option::is_some));
    let shader = lowered(&native, &p, &wide).expect("lower");
    if let Ok(mut v) = validator::Validator::load(validator::Validator::default_path().as_deref()) {
        let mut copy = shader.dxil.clone();
        v.validate(&mut copy).expect("DXIL validation failed");
    }
    // On a GPU that takes waves of 32 (two host waves for the console's one): the OR is of
    // both halves, whichever half's lanes take the branch.
    let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(host) = Host::new(false) else {
        eprintln!("no SM 6.6 hardware device; not run");
        return;
    };
    let (root, pso) = match host.compute_pipeline(&shader) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("the hardware device does not take this program (waves of 32?): {e}; not run");
            return;
        }
    };
    // (threads below s0 take the branch; thread t gives bit t >> 1)
    for (below, expected) in [(64u32, u32::MAX), (40, 0x000F_FFFF), (33, 0x0001_FFFF), (10, 0x1F), (1, 1)] {
        let mut ud = vec![0u32; shader.user_data_dwords().max(1) as usize];
        for u in &shader.user_data {
            if let (RootType::Constants, 0, 0) = (u.root, u.slot, u.dword) {
                ud[u.index as usize] = below;
            }
        }
        let out = host
            .dispatch(
                &root,
                &pso,
                &Dispatch {
                    roots: vec![(0, Root::Constants(ud))],
                    views: &[View::RawUav(1)],
                    samplers: &[],
                    groups: [1, 1, 1],
                },
            )
            .unwrap_or_else(|e| panic!("dispatch failed: {e}"));
        assert_eq!(out[0][0], expected, "{name}: {below} threads in the branch");
    }
    eprintln!("ran on the hardware device ({name}): the OR of both halves in every case");
}

/// Runs `shader` on the hardware device with the root constants `s0`, `s1` and gives dword 0
/// of the UAV; None (and a line) without a device that takes it.
fn on_the_card(shader: &HostShader, constants: [u32; 2]) -> Option<u32> {
    let Ok(host) = Host::new(false) else {
        eprintln!("no SM 6.6 hardware device; not run");
        return None;
    };
    let Ok((root, pso)) = host.compute_pipeline(shader) else {
        eprintln!("the hardware device does not take this program; not run");
        return None;
    };
    let mut ud = vec![0u32; shader.user_data_dwords().max(1) as usize];
    for u in &shader.user_data {
        if let (RootType::Constants, 0, d @ 0..=1) = (u.root, u.slot, u.dword) {
            ud[u.index as usize] = constants[d as usize];
        }
    }
    let out = host
        .dispatch(
            &root,
            &pso,
            &Dispatch {
                roots: vec![(0, Root::Constants(ud))],
                views: &[View::RawUav(1)],
                samplers: &[],
                groups: [1, 1, 1],
            },
        )
        .unwrap_or_else(|e| panic!("dispatch failed: {e}"));
    Some(out[0][0])
}

#[test]
fn local_waves_select_their_console_bits_from_a_constant_mask() {
    let words = [
        0xD760_000E,
        0x0001_0100, // v_readlane_b32 s14, v0, 0: require console lane numbering
        0xBE8C_0380, // s_mov_b32 s12, 0
        0xBE8D_03C1, // s_mov_b32 s13, -1
        0xD501_0001,
        0x0031_0280, // v_cndmask_b32 v1, 0, 1, s[12:13]
        0x3406_0082, // v_lshlrev_b32 v3, 2, v0
        0xE070_1000,
        0x8002_0103, // buffer_store_dword v1, v3, s[8:11], 0 offen
    ];
    let native = native(&words);
    let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
    let shader = lower::with_local_waves_any(|| lowered(&native, &p, &HashSet::new()))
        .expect("lower a constant mask on two host waves");
    if let Ok(mut v) = validator::Validator::load(validator::Validator::default_path().as_deref()) {
        let mut copy = shader.dxil.clone();
        v.validate(&mut copy).expect("DXIL validation failed");
    }
    let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(host) = Host::new(false) else {
        eprintln!("no SM 6.6 hardware device; not run");
        return;
    };
    let Ok((root, pso)) = host.compute_pipeline(&shader) else {
        eprintln!("the hardware device does not take this program; not run");
        return;
    };
    let out = host
        .dispatch(
            &root,
            &pso,
            &Dispatch {
                roots: vec![(0, Root::Constants(vec![0; shader.user_data_dwords() as usize]))],
                views: &[View::RawUav(64)],
                samplers: &[],
                groups: [1, 1, 1],
            },
        )
        .expect("dispatch the constant mask");
    assert_eq!(&out[0][..32], &[0; 32]);
    assert_eq!(&out[0][32..], &[1; 32]);
    eprintln!("ran on the hardware device: each half selected its own mask bits");
}

/// A 32-lane console wave. A register holds a lane mask on one path (the lanes below s0)
/// and a number on the other, and is read as a number after the join, as the trace programs
/// keep their lanes' masks and their node numbers on one stack
/// (`deferred_tiled:CS_LaserLightShadowRaytracing`, s46 at 0x104, 0x75c and 0x188). The
/// mask arrives as the ballot of its lanes.
const MASK_OR_NUMBER: [u32; 8] = [
    0x7D88_0000, // v_cmp_gt_u32 vcc_lo, s0, v0
    0xBE94_036A, // s_mov_b32 s20, vcc_lo
    0xBF06_8001, // s_cmp_eq_u32 s1, 0
    0xBF85_0001, // s_cbranch_scc1 over the next
    0xBE94_0385, // s_mov_b32 s20, 5
    0x7E00_0214, // v_mov_b32 v0, s20
    0x38 << 26 | 0x1C << 18,
    0x80 << 24 | 2 << 16, // buffer_store_dword v0, off, s[8:11], 0
];

/// The program above reads a lane mask as a number (on the path that kept it), so it needs
/// the console's 32 lanes in a host wave: it declares `WaveSize(32)`. Without it, a host of
/// 16-lane waves gave only 16 of the mask's bits (#356).
#[test]
fn a_lane_mask_joined_with_a_number_needs_the_consoles_wave_size() {
    let native = native_of(&MASK_OR_NUMBER, false, 32);
    let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
    let st = structure::structure(&p).expect("structure");
    let res =
        resource::track(&p, &native.layout, native.stage, Some(&st)).expect("resource::track");
    let io = io::raise(&p, &native).expect("io::raise");
    let was = lower::TEST_HOST_WAVE.replace(Some(32));
    let lowered = lower::lower(&lower::Parts {
        gs: None,
        native: &native,
        p: &p,
        st: &st,
        io: &io,
        res: &res,
        inputs: Vec::new(),
        outputs: Vec::new(),
        prims: Vec::new(),
        mesh_stride: 0,
    });
    lower::TEST_HOST_WAVE.set(was);
    assert_eq!(lowered.expect("lower").wave_size, Some(32));
}

#[test]
fn a_lane_mask_joined_with_a_number_is_read_as_the_number_it_is() {
    let native = native_of(&MASK_OR_NUMBER, false, 32);
    let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
    let shader = lowered(&native, &p, &HashSet::new()).expect("lower");
    if let Ok(mut v) = validator::Validator::load(validator::Validator::default_path().as_deref()) {
        let mut copy = shader.dxil.clone();
        v.validate(&mut copy).expect("DXIL validation failed");
    }
    let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    for (below, other, expected) in [(32u32, 0u32, u32::MAX), (10, 0, 0x3FF), (0, 0, 0), (10, 1, 5)] {
        let Some(got) = on_the_card(&shader, [below, other]) else { return };
        assert_eq!(got, expected, "{below} lanes in the mask, s1 = {other}");
    }
    eprintln!("ran on the hardware device: the mask's bits, or the number");
}

/// A 32-lane console wave. `if (t < s0) { ... SCC = (s1 == 7) }` and a branch on SCC after
/// the join: the skip brings the SCC of `s_and_saveexec` ("some lane is left") along
/// (`rtdirectionalshadows:CS_RTShadowTrace32HQ_TiledFarOnly`, 0x360-0x39c). On the console
/// the skip is taken by the wave, so with some lane inside every lane sees the inner test.
/// A lane that skipped on its own would see "some lane is left" instead: refused per lane,
/// right with the branch taken wave-wide. Lane 31 stores what it saw.
#[test]
fn scc_of_a_mask_operation_joined_with_a_scalar_test_needs_the_skip_wave_wide() {
    let words = [
        0x7D88_0000, // v_cmp_gt_u32 vcc_lo, s0, v0
        0xBE8D_3C6A, // s_and_saveexec_b32 s13, vcc_lo
        0xBF88_0001, // s_cbranch_execz over the next
        0xBF06_8701, // s_cmp_eq_u32 s1, 7
        0xBEFE_030D, // s_mov_b32 exec_lo, s13
        0xBF85_0002, // s_cbranch_scc1 to "seven"
        0x7E0A_0281, // v_mov_b32 v5, 1
        0xBF82_0001, // s_branch over the next
        0x7E0A_0282, // seven: v_mov_b32 v5, 2
        0x7DA4_009F, // v_cmpx_eq_u32 exec_lo, 31, v0
        0x38 << 26 | 0x1C << 18,
        0x80 << 24 | 2 << 16 | 5 << 8, // buffer_store_dword v5, off, s[8:11], 0
    ];
    let native = native_of(&words, false, 32);
    let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
    let branch = (0..p.blocks.len())
        .find(|&b| p.blocks[b].insts.last().is_some_and(|i| i.name() == "s_cbranch_execz"))
        .expect("the skip");
    lower::take_needs();
    let per_lane = lowered(&native, &p, &HashSet::new());
    assert!(
        matches!(&per_lane, Err(ShaderError::Unsupported { what, .. }) if *what == "a lane mask read as data"),
        "{:?}",
        per_lane.err()
    );
    let st = structure::structure(&p).expect("structure");
    assert_eq!(lower::branches_around(&st, &lower::take_needs()), Some(vec![branch]));
    let shader = lowered(&native, &p, &HashSet::from([branch])).expect("lower");
    if let Ok(mut v) = validator::Validator::load(validator::Validator::default_path().as_deref()) {
        let mut copy = shader.dxil.clone();
        v.validate(&mut copy).expect("DXIL validation failed");
    }
    let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    // (lanes below s0 inside, s1) -> what lane 31 stores: 2 if SCC was set at the branch
    for (below, s1, expected) in [(0u32, 7u32, 1u32), (10, 7, 2), (10, 3, 1), (32, 3, 1), (32, 7, 2)] {
        let Some(got) = on_the_card(&shader, [below, s1]) else { return };
        assert_eq!(got, expected, "{below} lanes inside, s1 = {s1}");
    }
    eprintln!("ran on the hardware device: the inner test's SCC for every lane once some lane is inside");
}

/// A console wave32 pixel shader run on a GPU that launches pixel shaders in
/// 64-lane waves (AMD RDNA). Each 32-lane half must keep its own lane index
/// (`v_mbcnt_lo`), ballot (`s_ff1`), lane read (`v_readlane_b32`) and
/// first-lane read (`v_readfirstlane_b32`), so upper-half lanes 32..63 never
/// read from lanes 0..31.
#[test]
fn wave32_pixel_shader_runs_as_32_lane_halves_on_wave64_hosts() {
    use super::container::SigElement;
    use super::host::Draw;
    let words: [u32; 22] = [
        // v0 = SV_Position.x, v1 = SV_Position.y -> v4 = y * 16 + x (0..255)
        0x7E04_0F00, // v_cvt_u32_f32 v2, v0
        0x7E06_0F01, // v_cvt_u32_f32 v3, v1
        0x3406_0684, // v_lshlrev_b32 v3, 4, v3
        0x3808_0702, // v_or_b32 v4, v2, v3
        // v5 = v_mbcnt_lo_u32_b32(-1, 0): console lane within its 32-lane wave
        0xD765_0005,
        0x0001_00C1,
        // s0 = s_ff1_i32_b32(ballot(v5 == 7)), s1 = v_readlane_b32(v4, s0)
        0x7D84_0A87, // v_cmp_eq_u32 vcc_lo, 7, v5
        0xBE80_136A, // s_ff1_i32_b32 s0, vcc_lo
        0xD760_0001,
        0x0000_0104, // v_readlane_b32 s1, v4, s0
        // narrow EXEC to v5 >= 3, then s3 = v_readfirstlane_b32(v4)
        0x7D86_0A83, // v_cmp_le_u32 vcc_lo, 3, v5
        0xBE82_3C6A, // s_and_saveexec_b32 s2, vcc_lo
        0x7E06_0504, // v_readfirstlane_b32 s3, v4
        0xBEFE_0302, // s_mov_b32 exec_lo, s2
        // Export (my_lane, picked_lane7, first_ge3, pix_id) as floats to mrt0
        0x7E0C_0D05, // v_cvt_f32_u32 v6, v5
        0x7E0E_0C01, // v_cvt_f32_u32 v7, s1
        0x7E10_0C03, // v_cvt_f32_u32 v8, s3
        0x7E12_0D04, // v_cvt_f32_u32 v9, v4
        0xF800_080F,
        0x0908_0706, // exp mrt0 v6, v7, v8, v9 done
        S_ENDPGM,
        S_ENDPGM,
    ];
    let native = NativeProgram {
        stage: HwStage::Ps,
        wave64: false,
        vgprs: 16,
        layout: vec![],
        used: vec![],
        io: StageIo::Ps {
            inputs: vec![],
            input_addr: (1 << 8) | (1 << 9),
            db_shader_control: 0,
            patches: vec![],
        },
        code: words[..21].iter().flat_map(|w| w.to_le_bytes()).collect(),
    };
    let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
    let st = structure::structure(&p).expect("structure");
    let res = resource::track(&p, &native.layout, native.stage, Some(&st)).expect("track");
    let io = io::raise(&p, &native).expect("io::raise");
    let pos = SigElement {
        name: "SV_Position".into(),
        index: 0,
        kind: 3,
        comp: 9,
        interp: 4,
        row: Some(0),
        col: 0,
        cols: 4,
        usage: 0x3,
        half: false,
    };
    let target = SigElement {
        name: "SV_Target".into(),
        index: 0,
        kind: 16,
        comp: 9,
        interp: 0,
        row: Some(0),
        col: 0,
        cols: 4,
        usage: 0xF,
        half: false,
    };
    let lowered = lower::lower(&lower::Parts {
        gs: None,
        native: &native,
        p: &p,
        st: &st,
        io: &io,
        res: &res,
        inputs: vec![pos],
        outputs: vec![target],
        prims: vec![],
        mesh_stride: 0,
    })
    .expect("lower");
    let ps = HostShader {
        dispatch_x_one: false,
        stage: HwStage::Ps,
        dxil: lowered.dxil,
        user_data: lowered.user_data,
        samplers: lowered.samplers,
        static_samplers: vec![],
        root_cbvs: lowered.root_cbvs,
        root_srvs: lowered.root_srvs,
        root_uavs: lowered.root_uavs,
        root_indexed_views: vec![],
    };
    if let Ok(mut v) = validator::Validator::load(validator::Validator::default_path().as_deref()) {
        let mut copy = ps.dxil.clone();
        v.validate(&mut copy).expect("DXIL validation failed");
    }
    let vs = {
        // Full-screen triangle from SV_VertexID (v0 = 0, 1, 2):
        // (-1,-1,0,1), (3,-1,0,1), (-1,3,0,1).
        let vs_words: [u32; 13] = [
            0x3602_0081, // v_and_b32 v1, 1, v0
            0x3402_0282, // v_lshlrev_b32 v1, 2, v1
            0x7E02_0D01, // v_cvt_f32_u32 v1, v1
            0x0602_02F3, // v_add_f32 v1, -1.0, v1
            0x3604_0082, // v_and_b32 v2, 2, v0
            0x3404_0481, // v_lshlrev_b32 v2, 1, v2
            0x7E04_0D02, // v_cvt_f32_u32 v2, v2
            0x0604_04F3, // v_add_f32 v2, -1.0, v2
            0x7E06_0280, // v_mov_b32 v3, 0
            0x7E08_02F2, // v_mov_b32 v4, 1.0
            0xF800_08CF,
            0x0403_0201, // exp pos0 v1, v2, v3, v4 done
            S_ENDPGM,
        ];
        let vn = NativeProgram {
            stage: HwStage::Vs,
            wave64: false,
            vgprs: 8,
            layout: vec![],
            used: vec![],
            io: StageIo::Vs {
                inputs: vec![],
                outputs: vec![],
            },
            code: vs_words.iter().flat_map(|w| w.to_le_bytes()).collect(),
        };
        let vp = ir::build(&vn.code, vn.wave64, vn.vgprs).expect("vs ir::build");
        let vst = structure::structure(&vp).expect("vs structure");
        let vres = resource::track(&vp, &vn.layout, vn.stage, Some(&vst)).expect("vs track");
        let vio = io::raise(&vp, &vn).expect("vs io::raise");
        let vpos = SigElement {
            name: "SV_Position".into(),
            index: 0,
            kind: 3,
            comp: 9,
            interp: 4,
            row: Some(0),
            col: 0,
            cols: 4,
            usage: 0xF,
            half: false,
        };
        let vl = lower::lower(&lower::Parts {
            gs: None,
            native: &vn,
            p: &vp,
            st: &vst,
            io: &vio,
            res: &vres,
            inputs: vec![],
            outputs: vec![vpos],
            prims: vec![],
            mesh_stride: 0,
        })
        .expect("vs lower");
        HostShader {
            dispatch_x_one: false,
            stage: HwStage::Vs,
            dxil: vl.dxil,
            user_data: vl.user_data,
            samplers: vl.samplers,
            static_samplers: vec![],
            root_cbvs: vl.root_cbvs,
            root_srvs: vl.root_srvs,
            root_uavs: vl.root_uavs,
            root_indexed_views: vec![],
        }
    };
    let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(host) = Host::new(false) else {
        eprintln!("no SM 6.6 hardware device; not run");
        return;
    };
    let (root, pso) = host.pipeline(&vs, &ps).expect("graphics pipeline");
    let pixels = host
        .draw(
            &root,
            &pso,
            &Draw {
                roots: vec![],
                textures: &[],
                samplers: &[],
                vertices: &[],
                stride: 0,
                size: (16, 16),
                mesh_groups: 0,
            },
        )
        .expect("draw");
    assert_eq!(pixels.len(), 256);
    let mut lane7_seen = HashSet::new();
    let mut lane3_seen = HashSet::new();
    for (idx, px) in pixels.iter().enumerate() {
        let my_lane = px[0] as u32;
        let picked7 = px[1] as u32;
        let first_ge3 = px[2] as u32;
        let pix_id = px[3] as u32;
        assert_eq!(pix_id, idx as u32, "pixel {idx} id");
        assert!(
            my_lane < 32,
            "pixel {idx}: v_mbcnt_lo gave lane {my_lane} >= 32 (upper half of a 64-lane host wave?)"
        );
        if my_lane == 7 {
            assert_eq!(
                picked7, pix_id,
                "pixel {idx} (lane 7): v_readlane_b32 read {picked7} instead of own half's {pix_id}"
            );
            lane7_seen.insert(picked7);
        }
        if my_lane == 3 {
            assert_eq!(
                first_ge3, pix_id,
                "pixel {idx} (lane 3): v_readfirstlane_b32 read {first_ge3} instead of own half's {pix_id}"
            );
            lane3_seen.insert(first_ge3);
        }
    }
    assert!(
        lane7_seen.len() >= 8,
        "expected >= 8 distinct 32-lane half-waves for 256 pixels, saw {}",
        lane7_seen.len()
    );
    assert!(
        lane3_seen.len() >= 8,
        "expected >= 8 distinct 32-lane half-waves for v_readfirstlane, saw {}",
        lane3_seen.len()
    );
    eprintln!(
        "ran on the hardware device: {} distinct 32-lane halves across 256 pixels",
        lane7_seen.len()
    );
}

/// One console wave of 64 threads, thread t holding t + 1. A branch on s0 (the wave's) picks
/// `WaveActiveMax` (s0 not 0) or `WaveActiveMin`, and the two reductions share their end:
/// each arm saves EXEC and reduces up to the read of lane 32, the join reads lane 0 and
/// restores EXEC (`deferred_tiled:CS_WaterTransmittanceOpticalDepthProcessing`, 0xb3c-0xbe0).
/// Neither block holds a whole section; `v_permlanex16_b32` lowered on its own reads the lanes
/// the host has, so the translation is the one with the join's code in each arm
/// ([`ir::whole_sections`]), where each section is whole.
#[test]
fn two_reductions_that_share_their_end_translate_as_two_sections() {
    // an arm: save EXEC, the identity `init`, then `op` over the wave up to lane 32's read
    let arm = |init: u32, op: u32| {
        let mut w = vec![
            0xBE8C_25C1, // s_or_saveexec_b64 s[12:13], -1
            0x7E04_02FF,
            init,        // v_mov_b32 v2, init
            0xD501_0002,
            0x0032_0902, // v_cndmask_b32 v2, v2, v4, s[12:13]
        ];
        for dpp in [0xDE54_C102, 0x9360_1202, 0x0009_2402] {
            w.extend([op << 25 | 0x0004_04E9, dpp]); // v_<op>_f32 v2, v2, v2 dpp8:[...]
        }
        w.extend([
            op << 25 | 0x0004_04FA,
            0xFF09_4002, // v_<op>_f32 v2, v2, v2 row_mirror bound_ctrl:1
            0xD778_0003,
            0x0201_0102,               // v_permlanex16_b32 v3, v2, 0, 0
            op << 25 | 0x0004_0503,    // v_<op>_f32 v2, v3, v2
            0xD760_000E,
            0x0001_4102,               // v_readlane_b32 s14, v2, 32
            op << 25 | 0x0004_040E,    // v_<op>_f32 v2, s14, v2
        ]);
        w
    };
    let mut words = vec![
        0x7E08_0D00, // v_cvt_f32_u32 v4, v0
        0x0608_08F2, // v_add_f32 v4, 1.0, v4
        0xBF06_8000, // s_cmp_eq_u32 s0, 0
        0xBF85_0014, // s_cbranch_scc1 to the minimum
    ];
    words.extend(arm(0xFF80_0000, 0x10)); // the maximum from -inf
    words.push(0xBF82_0013); // s_branch to the join
    words.extend(arm(0x7F80_0000, 0x0F)); // the minimum from +inf
    words.extend([
        0xD760_000E,
        0x0001_0102, // v_readlane_b32 s14, v2, 0
        0xBEFE_040C, // s_mov_b64 exec, s[12:13]
        0x7E00_020E, // v_mov_b32 v0, s14
        0x38 << 26 | 0x1C << 18,
        0x80 << 24 | 2 << 16, // buffer_store_dword v0, off, s[8:11], 0
    ]);
    let native = native(&words);
    let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
    // the section starts in the arms and ends in the join; afterwards each block holds whole ones
    let sections = |p: &ir::Program| (0..p.blocks.len()).map(|b| super::wave::sections(p, b).len()).sum::<usize>();
    assert_eq!(sections(&p), 0);
    let q = ir::whole_sections(&p).expect("sections to join");
    assert_eq!(sections(&q), 2);
    let st = structure::structure(&p).expect("structure");
    let res = resource::track(&p, &native.layout, native.stage, Some(&st)).expect("resource::track");
    let io = io::raise(&p, &native).expect("io::raise");
    let bytes = crate::dxbc::build(&[]);
    let c = super::Container::parse(&bytes).expect("container");
    let st_q = structure::structure(&q).expect("structure");
    let io_q = io::raise(&q, &native).expect("io::raise");
    let res_q = resource::track(&q, &native.layout, native.stage, Some(&st_q)).expect("track");
    let was = lower::TEST_HOST_WAVE.replace(Some(32));
    let joined = super::emit_any(&c, &native, &q, &st_q, &io_q, &res_q);
    let shader = super::emit(&c, &native, &p, &st, &io, &res);
    lower::TEST_HOST_WAVE.set(was);
    let shader = shader.expect("emit");
    let joined = joined.expect("the joined sections");
    assert!(shader.dxil == joined.dxil, "the split sections' translation was taken");
    if let Ok(mut v) = validator::Validator::load(validator::Validator::default_path().as_deref()) {
        let mut copy = shader.dxil.clone();
        v.validate(&mut copy).expect("DXIL validation failed");
    }
    let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    for (s0, expected) in [(1u32, 64.0f32), (0, 1.0)] {
        let Some(got) = on_the_card(&shader, [s0, 0]) else { return };
        assert_eq!(f32::from_bits(got), expected, "s0 = {s0}");
    }
    eprintln!("ran on the hardware device: the maximum or the minimum over the console's 64 lanes");
}

/// One console wave of 64 threads summing doubles over the lanes below s0 as the title's
/// water volumetrics count their lanes (`water_shading:CS_TransmissionVolumetrics`,
/// 0x5c4-0x68c): `v_add_f64` of each dword pair moved by DPP, `v_permlanex16` and lane 32,
/// the sum moved to s[20:21] and taken back as a float (`v_cvt_f32_f64`). The terms here are
/// the thread index as a double (`v_cvt_f64_f32`), so the sum is s0 (s0 - 1) / 2.
#[test]
fn a_wave_sum_of_doubles_is_the_consoles() {
    let mut words = vec![
        0x7E3E_0D00, // v_cvt_f32_u32 v31, v0
        0x7E3A_211F, // v_cvt_f64_f32 v[29:30], v31
        0x7D88_0000, // v_cmp_gt_u32 vcc, s0, v0
        0xBE8C_046A, // s_mov_b64 s[12:13], vcc
        0xBE90_25C1, // s_or_saveexec_b64 s[16:17], -1
        0x7E04_0280, // v_mov_b32 v2, 0
        0x7E06_0280, // v_mov_b32 v3, 0
        0x8792_100C, // s_and_b64 s[18:19], s[12:13], s[16:17]
        0xD501_0002,
        0x004A_3B02, // v_cndmask_b32 v2, v2, v29, s[18:19]
        0xD501_0003,
        0x004A_3D03, // v_cndmask_b32 v3, v3, v30, s[18:19]
    ];
    // v_mov_b32 v4, v2 / v5, v3 with each DPP pattern, then v_add_f64 v[2:3], v[4:5], v[2:3]
    for (code, dpp) in [(0xE9, 0xDE54_C100), (0xE9, 0x9360_1200), (0xE9, 0x0009_2400), (0xFA, 0xFF09_4000)] {
        words.extend([0x7E08_0200 | code, dpp | 2, 0x7E0A_0200 | code, dpp | 3, 0xD564_0002, 0x0002_0504]);
    }
    words.extend([
        0xD778_0004,
        0x0201_0102, // v_permlanex16_b32 v4, v2, 0, 0
        0xD778_0005,
        0x0201_0103, // v_permlanex16_b32 v5, v3, 0, 0
        0xD564_0002,
        0x0002_0504, // v_add_f64 v[2:3], v[4:5], v[2:3]
        0xD760_0012,
        0x0001_4102, // v_readlane_b32 s18, v2, 32
        0xD760_0013,
        0x0001_4103, // v_readlane_b32 s19, v3, 32
        0xD564_0002,
        0x0002_0412, // v_add_f64 v[2:3], s[18:19], v[2:3]
        0xD760_0012,
        0x0001_0102, // v_readlane_b32 s18, v2, 0
        0xD760_0013,
        0x0001_0103, // v_readlane_b32 s19, v3, 0
        0xBE94_0312, // s_mov_b32 s20, s18
        0xBE95_0313, // s_mov_b32 s21, s19
        0xBEFE_0410, // s_mov_b64 exec, s[16:17]
        0x7E00_1E14, // v_cvt_f32_f64 v0, s[20:21]
        0x38 << 26 | 0x1C << 18,
        0x80 << 24 | 2 << 16, // buffer_store_dword v0, off, s[8:11], 0
    ]);
    let native = native(&words);
    let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
    // the section leaves the sum's two dwords in s20 and s21
    let block = (0..p.blocks.len()).find(|&b| !super::wave::sections(&p, b).is_empty()).expect("a section");
    let sums = super::wave::sections(&p, block)
        .into_values()
        .flat_map(|s| s.shapes.into_values())
        .filter(|s| matches!(s, super::wave::Shape::Sum64 { .. }))
        .count();
    assert!(sums >= 2, "{sums} dwords of 64-bit sums");
    let st = structure::structure(&p).expect("structure");
    let res = resource::track(&p, &native.layout, native.stage, Some(&st)).expect("resource::track");
    let io = io::raise(&p, &native).expect("io::raise");
    let bytes = crate::dxbc::build(&[]);
    let c = super::Container::parse(&bytes).expect("container");
    let was = lower::TEST_HOST_WAVE.replace(Some(32));
    let shader = super::emit(&c, &native, &p, &st, &io, &res);
    lower::TEST_HOST_WAVE.set(was);
    let shader = shader.expect("emit");
    if let Ok(mut v) = validator::Validator::load(validator::Validator::default_path().as_deref()) {
        let mut copy = shader.dxil.clone();
        v.validate(&mut copy).expect("DXIL validation failed");
    }
    let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    for s0 in [64u32, 40, 33, 1, 0] {
        let Some(got) = on_the_card(&shader, [s0, 0]) else { return };
        let want = (s0 * s0.saturating_sub(1) / 2) as f32;
        assert_eq!(f32::from_bits(got), want, "the lanes below {s0}");
    }
    eprintln!("ran on the hardware device: the sums of doubles over the console's 64 lanes");
}

// ---- the paired form: a host wave of 32 for each console wave, two lanes a thread ----

// Operands as the hardware numbers them: SGPR n is n, VCC 106 (its high dword 107), EXEC 126,
// an inline integer n 128 + n (-1: 193), a literal (the next dword) 255, VGPR n 256 + n.
const VCC: u32 = 106;
const EXEC: u32 = 126;
const LIT: u32 = 255;

const fn v(n: u32) -> u32 {
    256 + n
}

const fn k(n: u32) -> u32 {
    128 + n
}

const fn sop1(op: u32, sdst: u32, src0: u32) -> u32 {
    0xBE80_0000 | sdst << 16 | op << 8 | src0
}

const fn sop2(op: u32, sdst: u32, src0: u32, src1: u32) -> u32 {
    0x8000_0000 | op << 23 | sdst << 16 | src1 << 8 | src0
}

/// A branch by `imm` dwords from the next instruction (or `s_barrier`, `imm` 0).
const fn sopp(op: u32, imm: i16) -> u32 {
    0xBF80_0000 | op << 16 | imm as u16 as u32
}

const fn vop1(op: u32, vdst: u32, src0: u32) -> u32 {
    0x7E00_0000 | vdst << 17 | op << 9 | src0
}

const fn vop2(op: u32, vdst: u32, src0: u32, vsrc1: u32) -> u32 {
    op << 25 | vdst << 17 | vsrc1 << 9 | src0
}

const fn vopc(op: u32, src0: u32, vsrc1: u32) -> u32 {
    0x7C00_0000 | op << 17 | vsrc1 << 9 | src0
}

const fn vop3(op: u32, dst: u32, src0: u32, src1: u32) -> [u32; 2] {
    [0xD400_0000 | op << 16 | dst, src0 | src1 << 9]
}

const fn ds(op: u32, vdst: u32, addr: u32, data: u32, offset: u32) -> [u32; 2] {
    [0xD800_0000 | op << 18 | offset, vdst << 24 | data << 8 | addr]
}

/// `buffer_store_dword v<data>, v<addr>, s[8:11], 0 offen offset:<offset>`
const fn store(data: u32, addr: u32, offset: u32) -> [u32; 2] {
    [0xE070_1000 | offset, 0x8002_0000 | data << 8 | addr]
}

/// `v<regs>` stored as each thread's dwords of the UAV, the thread's index (v0) times 16 on.
fn stores(regs: [u32; 4]) -> Vec<u32> {
    let mut w = vec![vop2(0x1A, 15, k(4), 0)]; // v_lshlrev_b32 v15, 4, v0
    for (n, r) in regs.into_iter().enumerate() {
        w.extend(store(r, 15, 4 * n as u32));
    }
    w
}

/// `native` in the paired form on a host of 32-lane waves, validated by `dxil.dll` when
/// present. Where no other form gives the host the program (each still asks for 64-lane
/// waves, or fails), it is also what [`super::emit`] gives it.
fn paired(native: &NativeProgram) -> HostShader {
    let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
    let st = structure::structure(&p).expect("structure");
    let res = resource::track(&p, &native.layout, native.stage, Some(&st)).expect("resource::track");
    let io = io::raise(&p, native).expect("io::raise");
    let bytes = crate::dxbc::build(&[]);
    let c = super::Container::parse(&bytes).expect("container");
    let was = lower::TEST_HOST_WAVE.replace(Some(32));
    let others = super::emit_forms(&c, native, &p, &st, &io, &res);
    let shader = super::paired(&c, native, &p, &io, &res);
    let chosen = super::emit(&c, native, &p, &st, &io, &res);
    lower::TEST_HOST_WAVE.set(was);
    let (shader, wave) = shader.expect("the paired form");
    assert_eq!(wave, Some(32));
    if !matches!(others, Ok((_, wave)) if wave != Some(64)) {
        assert_eq!(chosen.expect("emit").dxil, shader.dxil, "emit's choice");
    }
    if let Ok(mut v) = validator::Validator::load(validator::Validator::default_path().as_deref()) {
        let mut copy = shader.dxil.clone();
        v.validate(&mut copy).expect("DXIL validation failed");
    }
    shader
}

/// `shader` on the hardware device: one group, root constants `s0..s3`, a raw UAV of `dwords`
/// zeroed dwords, which come back; None (and a line) without a device that takes it.
fn run_paired(shader: &HostShader, constants: [u32; 4], dwords: u32) -> Option<Vec<u32>> {
    let Ok(host) = Host::new(false) else {
        eprintln!("no SM 6.6 hardware device; not run");
        return None;
    };
    let (root, pso) = match host.compute_pipeline(shader) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("the hardware device does not take this program: {e}; not run");
            return None;
        }
    };
    let mut ud = vec![0u32; shader.user_data_dwords().max(1) as usize];
    for u in &shader.user_data {
        if let (RootType::Constants, 0, d @ 0..=3) = (u.root, u.slot, u.dword) {
            ud[u.index as usize] = constants[d as usize];
        }
    }
    let mut out = host
        .dispatch(
            &root,
            &pso,
            &Dispatch {
                roots: vec![(0, Root::Constants(ud))],
                views: &[View::RawUav(dwords)],
                samplers: &[],
                groups: [1, 1, 1],
            },
        )
        .unwrap_or_else(|e| panic!("dispatch failed: {e}"));
    Some(out.swap_remove(0))
}

/// Panics at the first dword of `got` that is not the console's (`want`), naming the thread
/// and which of its four dwords.
fn same_as_console(got: &[u32], want: &[u32]) {
    assert_eq!(got.len(), want.len());
    if let Some(n) = (0..want.len()).find(|&n| got[n] != want[n]) {
        panic!("thread {}, dword {}: {:#x}, the console's {:#x}", n / 4, n % 4, got[n], want[n]);
    }
}

/// One console wave of 64 threads with 256 bytes of LDS. The even lanes write their dword in
/// one arm of a per-lane branch; the odd lanes theirs in the other arm, then, in that arm and
/// with no barrier, read their mirror lane's (63 - n, an even lane's); the even lanes read
/// theirs (an odd lane's) after the join. Then lanes 5 and 40 are read (`v_readlane_b32`), the
/// lanes holding more than 250 are counted (`s_bcnt1_i32_b64` of the compare's mask) and
/// counted below each lane (`v_mbcnt_lo/hi`): each of these takes lanes of both halves.
#[test]
fn paired_lanes_share_lds_and_lanes_across_halves_without_barriers() {
    let mut w = vec![vop2(0x1A, 1, k(2), 0)]; // v_lshlrev_b32 v1, 2, v0: this lane's dword
    w.extend(vop3(0x169, 2, v(0), k(7))); // v_mul_lo_u32 v2, v0, 7
    w.extend([
        vop2(0x26, 5, k(63), 0), // v_sub_nc_u32 v5, 63, v0: the mirror lane
        vop2(0x1A, 5, k(2), 5),  // v_lshlrev_b32 v5, 2, v5: its dword
        vop2(0x1B, 3, k(1), 0),  // v_and_b32 v3, 1, v0
        vopc(0xC2, k(0), 3),     // v_cmp_eq_u32 vcc, 0, v3: the even lanes
        sop1(0x24, 12, VCC),     // s_and_saveexec_b64 s[12:13], vcc
        sopp(0x08, 4),           // s_cbranch_execz past the even lanes' arm
        vop2(0x25, 4, LIT, 2),   // v_add_nc_u32 v4, 100, v2
        100,
    ]);
    w.extend(ds(0x0D, 0, 1, 4, 0)); // ds_write_b32 v1, v4
    w.extend([
        sop2(0x15, EXEC, 12, EXEC), // s_andn2_b64 exec, s[12:13], exec: the odd lanes
        sopp(0x08, 6),              // s_cbranch_execz past their arm
        vop2(0x25, 4, LIT, 2),      // v_add_nc_u32 v4, 200, v2
        200,
    ]);
    w.extend(ds(0x0D, 0, 1, 4, 0)); // ds_write_b32 v1, v4
    w.extend(ds(0x36, 6, 5, 0, 0)); // ds_read_b32 v6, v5: an even lane's, from the other arm
    w.extend([
        sop1(0x04, EXEC, 12), // s_mov_b64 exec, s[12:13]
        sop1(0x24, 12, VCC),  // s_and_saveexec_b64 s[12:13], vcc: the even lanes again
    ]);
    w.extend(ds(0x36, 6, 5, 0, 0)); // ds_read_b32 v6, v5: an odd lane's
    w.push(sop1(0x04, EXEC, 12)); // s_mov_b64 exec, s[12:13]
    w.extend(vop3(0x360, 14, v(6), k(5))); // v_readlane_b32 s14, v6, 5
    w.extend(vop3(0x360, 15, v(6), k(40))); // v_readlane_b32 s15, v6, 40
    w.extend([
        sop2(0x00, 14, 14, 15), // s_add_u32 s14, s14, s15
        vopc(0xC1, LIT, 6),     // v_cmp_lt_u32 vcc, 250, v6
        250,
        sop1(0x10, 16, VCC), // s_bcnt1_i32_b64 s16, vcc
    ]);
    w.extend(vop3(0x365, 7, VCC, k(0))); // v_mbcnt_lo_u32_b32 v7, vcc_lo, 0
    w.extend(vop3(0x366, 7, VCC + 1, v(7))); // v_mbcnt_hi_u32_b32 v7, vcc_hi, v7
    w.extend([
        vop1(0x01, 9, 14),  // v_mov_b32 v9, s14
        vop1(0x01, 10, 16), // v_mov_b32 v10, s16
    ]);
    w.extend(stores([6, 9, 10, 7]));
    let shader = paired(&native_with(&w, true, 64, 256));
    // the console: lane n's dword is 7n + 100 (even n) or 7n + 200, and lane n reads 63 - n's
    let dword = |n: u32| 7 * n + if n.is_multiple_of(2) { 100 } else { 200 };
    let read: Vec<u32> = (0..64).map(|n| dword(63 - n)).collect();
    let big = (0..64).filter(|&n| read[n] > 250).fold(0u64, |m, n| m | 1 << n);
    assert!(big as u32 != 0 && big >> 32 != 0, "lanes of both halves counted");
    let want: Vec<u32> = (0..64)
        .flat_map(|n| [read[n], read[5] + read[40], big.count_ones(), (big & ((1 << n) - 1)).count_ones()])
        .collect();
    let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let Some(got) = run_paired(&shader, [0; 4], 256) else { return };
    same_as_console(&got, &want);
    eprintln!("ran on the hardware device: LDS, lane reads and masks across the halves as on the console");
}

/// Two console waves of 64 in a group of 128 (two host waves of 32, 512 bytes of LDS). In
/// each, lane 0 zeroes the wave's dword of LDS; every lane loops (lane & 3) + 1 + wave times,
/// adding its lane number to its sum and 1 to the wave's dword each round (`ds_add_u32`; the
/// loop's branch is the wave's: it goes round while some lane is left); after the loop each
/// lane reads the wave's total, with no barrier; lanes 40-63 read their first lane's sum
/// (`v_readfirstlane_b32`: lane 40, an upper lane), then lanes 20-63 (lane 20, a lower one,
/// though upper lanes have one too); past the barrier, each lane reads the other wave's total.
#[test]
fn paired_lanes_loop_apart_and_meet_the_other_wave_at_the_barrier() {
    let mut w = vec![
        vop2(0x16, 1, k(6), 0),  // v_lshrrev_b32 v1, 6, v0: the console wave
        vop2(0x1B, 2, k(63), 0), // v_and_b32 v2, 63, v0: the lane in it
        vop2(0x1A, 11, k(2), 1), // v_lshlrev_b32 v11, 2, v1: the wave's dword
        vop1(0x01, 12, k(0)),    // v_mov_b32 v12, 0
        sop1(0x04, 22, EXEC),    // s_mov_b64 s[22:23], exec
        vopc(0xD2, k(0), 2),     // v_cmpx_eq_u32 exec, 0, v2: lane 0
    ];
    w.extend(ds(0x0D, 0, 11, 12, 0)); // ds_write_b32 v11, v12
    w.extend([
        sop1(0x04, EXEC, 22),   // s_mov_b64 exec, s[22:23]
        vop2(0x1B, 3, k(3), 2), // v_and_b32 v3, 3, v2
        vop2(0x25, 3, k(1), 3), // v_add_nc_u32 v3, 1, v3
        vop2(0x25, 3, v(1), 3), // v_add_nc_u32 v3, v1, v3: the rounds
        vop1(0x01, 4, k(0)),    // v_mov_b32 v4, 0
        vop1(0x01, 13, k(1)),   // v_mov_b32 v13, 1
        sop1(0x04, 20, EXEC),   // s_mov_b64 s[20:21], exec
        vop2(0x25, 4, v(2), 4), // loop: v_add_nc_u32 v4, v2, v4
    ]);
    w.extend(ds(0x00, 0, 11, 13, 0)); // ds_add_u32 v11, v13
    w.extend([
        vop2(0x25, 3, 193, 3),       // v_add_nc_u32 v3, -1, v3
        vopc(0xC5, k(0), 3),         // v_cmp_ne_u32 vcc, 0, v3
        sop2(0x0F, EXEC, EXEC, VCC), // s_and_b64 exec, exec, vcc
        sopp(0x09, -7),              // s_cbranch_execnz loop
        sop1(0x04, EXEC, 20),        // s_mov_b64 exec, s[20:21]
    ]);
    w.extend(ds(0x36, 5, 11, 0, 0)); // ds_read_b32 v5, v11: the wave's total
    w.extend([
        sop1(0x04, 24, EXEC), // s_mov_b64 s[24:25], exec
        vopc(0xD1, k(39), 2), // v_cmpx_lt_u32 exec, 39, v2: lanes 40-63
        vop1(0x02, 6, v(4)),  // v_readfirstlane_b32 s6, v4
        sop1(0x04, EXEC, 24), // s_mov_b64 exec, s[24:25]
        vopc(0xD1, k(19), 2), // v_cmpx_lt_u32 exec, 19, v2: lanes 20-63
        vop1(0x02, 7, v(4)),  // v_readfirstlane_b32 s7, v4
        sop1(0x04, EXEC, 24), // s_mov_b64 exec, s[24:25]
        vop1(0x01, 6, 6),     // v_mov_b32 v6, s6
        vop1(0x01, 9, 7),     // v_mov_b32 v9, s7
        0xD400_0000 | 0x346 << 16 | 6, // v_lshl_add_u32 v6, v9, 16, v6
        v(9) | k(16) << 9 | v(6) << 18,
        sopp(0x0A, 0),            // s_barrier
        vop2(0x1D, 14, k(4), 11), // v_xor_b32 v14, 4, v11: the other wave's dword
    ]);
    w.extend(ds(0x36, 7, 14, 0, 0)); // ds_read_b32 v7, v14
    w.extend(stores([4, 5, 6, 7]));
    let shader = paired(&native_with(&w, true, 128, 512));
    let rounds = |wave: u32, lane: u32| (lane & 3) + 1 + wave;
    let total = |wave: u32| (0..64).map(|lane| rounds(wave, lane)).sum::<u32>();
    let first = |wave: u32, lane: u32| lane * rounds(wave, lane);
    let want: Vec<u32> = (0..128)
        .flat_map(|t| {
            let (wave, lane) = (t / 64, t % 64);
            [first(wave, lane), total(wave), first(wave, 20) << 16 | first(wave, 40), total(1 - wave)]
        })
        .collect();
    let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let Some(got) = run_paired(&shader, [0; 4], 512) else { return };
    same_as_console(&got, &want);
    eprintln!("ran on the hardware device: two console waves, their loops and LDS as on the console");
}

/// One console wave: `ds_bpermute_b32` from the mirror lane; a DPP row shift whose row mask
/// leaves rows 0 and 3 alone (rows 2 and 3 are lanes 32-63: rows are counted in console lanes); a
/// `ds_swizzle_b32` swapping the two 16-lane rows of each 32 lanes; `v_writelane_b32` of s0
/// into lane 45.
#[test]
fn paired_lanes_permute_shift_and_write_lanes_of_both_halves() {
    let mut w = vop3(0x169, 1, v(0), k(3)).to_vec(); // v_mul_lo_u32 v1, v0, 3
    w.extend([
        vop2(0x25, 1, k(1), 1),  // v_add_nc_u32 v1, 1, v1
        vop2(0x26, 2, k(63), 0), // v_sub_nc_u32 v2, 63, v0
        vop2(0x1A, 2, k(2), 2),  // v_lshlrev_b32 v2, 2, v2: the mirror lane's address
    ]);
    w.extend(ds(0xB3, 3, 2, 1, 0)); // ds_bpermute_b32 v3, v2, v1
    w.extend([
        vop1(0x01, 4, LIT), // v_mov_b32 v4, 1000
        1000,
        vop1(0x01, 4, 0xFA), // v_mov_b32 v4, v1 row_shr:1 row_mask:0x6 bank_mask:0xf
        0x6 << 28 | 0xF << 24 | 0x111 << 8 | 1,
    ]);
    w.extend(ds(0x35, 5, 1, 0, 0x401F)); // ds_swizzle_b32 v5, v1 offset:swizzle(BITMASK_PERM, xor 16)
    w.push(vop1(0x01, 6, v(0))); // v_mov_b32 v6, v0
    w.extend(vop3(0x361, 6, 0, k(45))); // v_writelane_b32 v6, s0, 45
    w.extend(stores([3, 4, 5, 6]));
    let shader = paired(&native_with(&w, true, 64, 0));
    let s0 = 0xABCD_1234;
    let x = |n: u32| 3 * n + 1;
    let want: Vec<u32> = (0..64)
        .flat_map(|n| {
            let shifted = if matches!(n >> 4, 1 | 2) && n & 15 != 0 { x(n - 1) } else { 1000 };
            let swizzled = x(n & 32 | (n & 31) ^ 16);
            [x(63 - n), shifted, swizzled, if n == 45 { s0 } else { n }]
        })
        .collect();
    let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let Some(got) = run_paired(&shader, [s0, 0, 0, 0], 256) else { return };
    same_as_console(&got, &want);
    eprintln!("ran on the hardware device: permutes, row masks, swizzles and lane writes as on the console");
}

/// [`program`]'s wave OR inside a per-lane branch, in the paired form: the OR of both halves'
/// lanes, whichever take the branch; nothing stored when none does.
#[test]
fn paired_lanes_or_a_wave_inside_a_lane_branch() {
    for (skip, name) in [(0xBF88_0000, "s_cbranch_execz"), (0xBF84_0000, "s_cbranch_scc0")] {
        let shader = paired(&program(skip));
        let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        let cases = [(64u32, u32::MAX), (40, 0x000F_FFFF), (33, 0x0001_FFFF), (10, 0x1F), (1, 1), (0, 0)];
        for (below, expected) in cases {
            let Some(got) = on_the_card(&shader, [below, 0]) else { return };
            assert_eq!(got, expected, "{name}: {below} threads in the branch");
        }
    }
    eprintln!("ran on the hardware device: the OR of both halves in every case");
}

/// One console wave of 64 threads, thread t holding the bit `1 << (t >> 1)`: the threads below
/// s0 OR theirs over the wave inside `if (t < s0) { ... }` ([`super::wave::WAVE_OR_V7_TO_S7`]
/// into s7, which holds 0x77 before), and every thread stores s7 after the branch: the wave's
/// OR, or 0x77 when no thread took the branch and the wave skipped it. A wave-wide value used
/// past a per-lane branch, as the title's tile classification and light clustering programs
/// use theirs: no form in two host waves takes it (they would meet inside the branch, and the
/// skip cannot be left out: s7 would no longer keep 0x77), and the approximate form, each host
/// wave a wave of its own, gives each half its own OR, 0x77 to the upper half while s0 <= 32.
/// The title's 39 such programs ran approximate; [`super::emit`] now gives the paired form.
#[test]
fn a_wave_value_used_past_a_lane_branch_takes_the_paired_form_not_an_approximate_one() {
    let mut w = vec![
        sop1(0x03, 7, LIT), // s_mov_b32 s7, 0x77
        0x77,
        vop1(0x01, 5, k(1)),    // v_mov_b32 v5, 1
        vop2(0x16, 6, k(1), 0), // v_lshrrev_b32 v6, 1, v0
        vop2(0x1A, 7, v(6), 5), // v_lshlrev_b32 v7, v6, v5
        vopc(0xC4, 0, 0),       // v_cmp_gt_u32 vcc, s0, v0
        sop1(0x24, 12, VCC),    // s_and_saveexec_b64 s[12:13], vcc
        sopp(0x08, 20),         // s_cbranch_execz past the OR
    ];
    w.extend_from_slice(&super::wave::WAVE_OR_V7_TO_S7);
    w.extend([
        sop1(0x04, EXEC, 12), // s_mov_b64 exec, s[12:13]
        vop1(0x01, 8, 7),     // v_mov_b32 v8, s7
    ]);
    w.extend(stores([8, 7, 6, 0]));
    let native = native_with(&w, true, 64, 0);
    let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
    let st = structure::structure(&p).expect("structure");
    let res = resource::track(&p, &native.layout, native.stage, Some(&st)).expect("resource::track");
    let io = io::raise(&p, &native).expect("io::raise");
    let bytes = crate::dxbc::build(&[]);
    let c = super::Container::parse(&bytes).expect("container");
    let was = lower::TEST_HOST_WAVE.replace(Some(32));
    let pairs = super::paired(&c, &native, &p, &io, &res);
    let chosen = super::emit(&c, &native, &p, &st, &io, &res);
    // (as before: the paired form only where no other form takes the program)
    let off = super::TEST_PAIRS.replace(Some(false));
    let approximate = super::emit(&c, &native, &p, &st, &io, &res);
    super::TEST_PAIRS.set(off);
    lower::TEST_HOST_WAVE.set(was);
    let (pairs, wave) = pairs.expect("the paired form");
    assert_eq!(wave, Some(32));
    let (chosen, approximate) = (chosen.expect("emit"), approximate.expect("an approximate form"));
    assert!(chosen.dxil == pairs.dxil, "emit's choice is not the paired form");
    assert!(approximate.dxil != pairs.dxil, "without the paired form, emit gives it anyway");
    if let Ok(mut v) = validator::Validator::load(validator::Validator::default_path().as_deref()) {
        let mut copy = chosen.dxil.clone();
        v.validate(&mut copy).expect("DXIL validation failed");
    }
    // the console: every thread stores the OR of the threads below s0, or 0x77 with none
    let want = |s0: u32| -> Vec<u32> {
        let or = (0..64u32).filter(|&t| t < s0).fold(0u32, |m, t| m | 1 << (t >> 1));
        let s7 = if s0 == 0 { 0x77 } else { or };
        (0..64u32).flat_map(|t| [s7, 1 << (t >> 1), t >> 1, t]).collect()
    };
    let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    for s0 in [64u32, 40, 33, 32, 10, 1, 0] {
        let Some(got) = run_paired(&chosen, [s0, 0, 0, 0], 256) else { return };
        same_as_console(&got, &want(s0));
    }
    // each half on its own: with s0 = 10 the upper half took no branch and kept 0x77
    let Some(halves) = run_paired(&approximate, [10, 0, 0, 0], 256) else { return };
    assert_eq!((want(10)[0], want(10)[4 * 40]), (0x1F, 0x1F));
    assert_eq!((halves[0], halves[4 * 40]), (0x1F, 0x77), "the approximate form's lanes 0 and 40");
    eprintln!("ran on the hardware device: the wave's OR past the branch in every lane, not each half's");
}

/// [`half_sum_program`] in the paired form: each half's tree over its 32 lanes, then the two
/// halves' sums, the console's last addition.
#[test]
fn paired_lanes_sum_halves_as_the_consoles_tree() {
    let (native, tree, _) = half_sum_program();
    let shader = paired(&native);
    let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let Some(got) = on_the_card(&shader, [0, 0]) else { return };
    assert_eq!(got, u32::from(tree));
    eprintln!("ran on the hardware device: the sum {tree:#x} of the console's tree");
}

/// The lens-flare scans' allocation (`lens_flares:CS_ScanMip1`, 0x5c0-0x61c): in a branch
/// of the lanes below s0, the first lane adds the wave's count times a size (s1) to a
/// counter, and each lane takes base + below × size with `v_mad_u32_u24`. That is the
/// idiom a host wave may run alone ([`super::idiom::wave_local`]): each half reserves its own
/// count's slots. The same with the count where the base goes is not.
#[test]
fn an_allocation_in_units_of_a_size_is_the_idiom() {
    let program = |base: u32| -> Vec<u8> {
        let words = [
            0x7D88_0000, // v_cmp_gt_u32 vcc, s0, v0
            0xBE8C_246A, // s_and_saveexec_b64 s[12:13], vcc
            0xBF88_0016, // s_cbranch_execz past the allocation
            0xBE82_047E, // s_mov_b64 s[2:3], exec
            0xBE84_147E, // s_ff1_i32_b64 s4, exec
            0xD765_0004,
            0x0001_00C1, // v_mbcnt_lo_u32_b32 v4, -1, 0
            0xD766_0004,
            0x0002_08C1, // v_mbcnt_hi_u32_b32 v4, -1, v4: the lane
            0xBE86_047E, // s_mov_b64 s[6:7], exec
            0x7DA4_0804, // v_cmpx_eq_u32 exec, s4, v4: the first lane
            0xBE82_1002, // s_bcnt1_i32_b64 s2, s[2:3]: the count
            0x7E02_0201, // v_mov_b32 v1, s1: the size
            0x160A_0202, // v_mul_u32_u24 v5, s2, v1
            0x7E16_0280, // v_mov_b32 v11, 0
            0xE0C8_6000,
            0x8002_050B, // buffer_atomic_add v5, v11, s[8:11], 0 idxen glc
            0xBEFE_0406, // s_mov_b64 exec, s[6:7]
            0xD765_0004,
            0x0001_007E, // v_mbcnt_lo_u32_b32 v4, exec_lo, 0
            0xD766_0004,
            0x0002_087F, // v_mbcnt_hi_u32_b32 v4, exec_hi, v4: the lanes below
            0x7E0A_0505, // v_readfirstlane_b32 s5, v5: the base
            0xD543_0006,
            0x0002_0304 | base << 18, // v_mad_u32_u24 v6, v4, v1, s<base>
            0xBEFE_040C, // s_mov_b64 exec, s[12:13]
            S_ENDPGM,
        ];
        words.iter().flat_map(|w| w.to_le_bytes()).collect()
    };
    let idiom = |code: &[u8]| {
        let p = ir::build(code, true, 256).unwrap();
        let st = structure::structure(&p).unwrap();
        super::idiom::wave_local(&p, &st)
    };
    assert_eq!(idiom(&program(5)), Ok(()));
    assert!(idiom(&program(2)).is_err(), "the count as the base");
}
