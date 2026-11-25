//! Console wave64 compute programs on hosts with and without 64-lane waves:
//! hand-assembled GFX10 code, dispatched on every hardware adapter, checked
//! lane by lane against what the console's lane layout gives. On a host
//! without 64-lane waves, programs whose lanes read only their own 32-lane
//! half run as two host waves of 32 (see `lower::Lower::new`).

use super::host::{Dispatch, Host, Root, View, adapters};
use super::tests::{compute, emit_for};
use crate::native::{RootParam, RootType, StageIo};

const S_ENDPGM: u32 = 0xBF81_0000;

/// The wave size a DXIL container's `PSV0` requires (minimum = maximum).
fn wave_size(dxil: &[u8]) -> Option<u32> {
    let c = crate::Container::parse(dxil).ok()?;
    let psv = c.part(crate::FourCC::PSV0)?.data;
    let u = |at: usize| crate::dxbc::u32_at(psv, at).ok();
    let (min, max) = (u(4 + 16)?, u(4 + 20)?);
    (min == max).then_some(min)
}

/// A program over `threads` x 1 x 1 lanes per group with a UAV table in
/// user SGPR 0 and the group id in s1: lane t of group g stores the
/// result of `body` (v0 = t in, v2 out) at dword g * threads + t.
fn program(body: &[u32], threads: u32) -> crate::NativeProgram {
    let mut words = vec![
        0xBE82_0300, // s_mov_b32 s2, s0
        0xBE83_0380, // s_mov_b32 s3, 0
        0xF408_0101,
        0xFA00_0000, // s_load_dwordx4 s[4:7], s[2:3], null
        0x9308_FF01,
        threads * 4, // s_mul_i32 s8, s1, threads * 4
        0xBF8C_C07F, // s_waitcnt lgkmcnt(0)
    ];
    words.extend_from_slice(body);
    words.extend_from_slice(&[
        0x3406_0082, // v_lshlrev_b32 v3, 2, v0
        0x4A06_0608, // v_add_nc_u32 v3, s8, v3
        0xE070_1000,
        0x8001_0203, // buffer_store_dword v2, v3, s[4:7], 0 offen
        S_ENDPGM,
    ]);
    let mut p = compute(&words, true);
    p.layout = vec![RootParam {
        root: RootType::Table,
        table_offset_dwords: 0,
        sgprs: 1,
        base_register: 0,
        user_data: 0,
        slot: 0,
    }];
    // One user SGPR, then the group id x.
    p.io = StageIo::Cs(crate::native::ComputeIo {
        rsrc2: 1 << 1 | 1 << 7,
        threads: [threads, 1, 1],
    });
    p
}

/// Swaps the 16-lane rows of each 32 (`ds_swizzle` xor 16), then mirrors
/// each row (DPP): lane t ends with t ^ 31, every read inside its half.
const IN_HALF: &[u32] = &[
    0xD8D4_401F,
    0x0100_0000, // ds_swizzle_b32 v1, v0 offset:swizzle(BITMASK_PERM, and 0x1f, xor 0x10)
    0x7E04_02FA,
    0xFF01_4001, // v_mov_b32_dpp v2, v1 row_mirror row_mask:0xf bank_mask:0xf
];

fn run(host: &Host, native: &crate::NativeProgram, groups: u32) -> Result<Vec<u32>, String> {
    let target = host.target();
    let cs = emit_for(native, target).map_err(|e| e.to_string())?;
    // The validator, where one is installed: drivers take some modules it
    // refuses.
    if let Ok(mut v) =
        super::validator::Validator::load(super::validator::Validator::default_path().as_deref())
    {
        v.validate(&mut cs.dxil.clone())
            .map_err(|e| format!("validation: {e}"))?;
    }
    let (root, pso) = host.compute_pipeline(&cs)?;
    let data = vec![0; (cs.user_data_dwords() as usize).max(1)];
    let StageIo::Cs(io) = &native.io else {
        unreachable!()
    };
    let n = io.threads.iter().product::<u32>() * groups;
    let views = [View::RawUav(n)];
    let out = host.dispatch(
        &root,
        &pso,
        &Dispatch {
            roots: vec![(0, Root::Constants(data))],
            views: &views,
            samplers: &[],
            groups: [groups, 1, 1],
        },
    )?;
    Ok(out.into_iter().next().unwrap_or_default())
}

#[test]
fn in_half_lane_reads_match_the_console_on_every_adapter() {
    let Ok(names) = adapters() else {
        eprintln!("no DXGI; skipped");
        return;
    };
    let mut ran = 0;
    for name in names {
        let Ok(host) = Host::on(&name) else {
            eprintln!("{name}: no SM 6.6 device; skipped");
            continue;
        };
        let target = host.target();
        // One console wave, two per group, and a group whose second wave
        // has its lower half only.
        for threads in [64, 128, 32] {
            let p = program(IN_HALF, threads);
            let cs = emit_for(&p, target).unwrap_or_else(|e| panic!("{name}: {e}"));
            let split = !target.runs(64);
            assert_eq!(
                wave_size(&cs.dxil),
                Some(if split { 32 } else { 64 }),
                "{name}: the declared wave size"
            );
            let groups = 3;
            let out = run(&host, &p, groups).unwrap_or_else(|e| panic!("{name}: {e}"));
            let want: Vec<u32> = (0..groups)
                .flat_map(|_| (0..threads).map(|t| t ^ 31))
                .collect();
            assert_eq!(out, want, "{name}: {threads} threads (split {split})");
            eprintln!("{name}: {threads} threads per group exact (split {split})");
        }
        ran += 1;
    }
    if ran == 0 {
        eprintln!("no hardware adapter; skipped");
    }
}

/// 3-D groups: the console forms waves from the flat thread id (x
/// fastest). Lane f (f = x + X * (y + Y * z)) stores f ^ 31 at dword f.
fn program_3d(threads: [u32; 3]) -> crate::NativeProgram {
    let [x, y, _] = threads;
    let (sx, sxy) = (x.trailing_zeros(), (x * y).trailing_zeros());
    assert!(x.is_power_of_two() && y.is_power_of_two());
    let words = [
        0xBE82_0300, // s_mov_b32 s2, s0
        0xBE83_0380, // s_mov_b32 s3, 0
        0xF408_0101,
        0xFA00_0000,               // s_load_dwordx4 s[4:7], s[2:3], null
        0xBF8C_C07F,               // s_waitcnt lgkmcnt(0)
        0x340A_0200 | (128 + sx),  // v_lshlrev_b32 v5, log2 X, v1
        0x340C_0400 | (128 + sxy), // v_lshlrev_b32 v6, log2 XY, v2
        0x4A08_0B00,               // v_add_nc_u32 v4, v0, v5
        0x4A08_0D04,               // v_add_nc_u32 v4, v4, v6
        0xD8D4_401F,
        0x0100_0004, // ds_swizzle_b32 v1, v4 offset:swizzle(BITMASK_PERM, and 0x1f, xor 0x10)
        0x7E04_02FA,
        0xFF01_4001, // v_mov_b32_dpp v2, v1 row_mirror
        0x3406_0882, // v_lshlrev_b32 v3, 2, v4
        0xE070_1000,
        0x8001_0203, // buffer_store_dword v2, v3, s[4:7], 0 offen
        S_ENDPGM,
    ];
    let mut p = compute(&words, true);
    p.layout = program(&[], 64).layout;
    // One user SGPR; thread ids x, y and z in v0..v2.
    p.io = StageIo::Cs(crate::native::ComputeIo {
        rsrc2: 1 << 1 | 2 << 11,
        threads,
    });
    p
}

#[test]
fn three_dimensional_groups_keep_the_consoles_lanes() {
    let Ok(names) = adapters() else {
        eprintln!("no DXGI; skipped");
        return;
    };
    for name in names {
        let Ok(host) = Host::on(&name) else {
            continue;
        };
        let split = !host.target().runs(64);
        for threads in [
            [4, 4, 4],
            [8, 8, 2],
            [2, 4, 8],
            [16, 2, 4],
            [8, 8, 4],
            [4, 8, 8],
        ] {
            let p = program_3d(threads);
            let n: u32 = threads.iter().product();
            let out = run(&host, &p, 1).unwrap_or_else(|e| panic!("{name}: {e}"));
            let want: Vec<u32> = (0..n).map(|f| f ^ 31).collect();
            assert_eq!(out, want, "{name}: {threads:?} (split {split})");
            eprintln!("{name}: {threads:?} exact (split {split})");
        }
    }
}

/// Runs `body` (v0 = t in, v2 out) on every hardware adapter in groups of
/// `threads` and checks lane t against `want(t)`; on a host without
/// 64-lane waves it must have translated in the console flow.
fn console_flow_case(label: &str, body: &[u32], threads: u32, want: impl Fn(u32) -> u32) {
    flow_case(label, body, threads, want, true);
}

/// [`console_flow_case`], checking hosts with 64-lane waves (the per-lane
/// form) only when `native` says.
fn flow_case(label: &str, body: &[u32], threads: u32, want: impl Fn(u32) -> u32, native: bool) {
    let Ok(names) = adapters() else {
        eprintln!("no DXGI; skipped");
        return;
    };
    for name in names {
        let Ok(host) = Host::on(&name) else {
            continue;
        };
        let p = program(body, threads);
        let split = !host.target().runs(64);
        let groups = 2;
        let out = run(&host, &p, groups).unwrap_or_else(|e| panic!("{name}: {label}: {e}"));
        let expect: Vec<u32> = (0..groups).flat_map(|_| (0..threads).map(&want)).collect();
        if !split && !native {
            if out != expect {
                eprintln!("{name}: {label}: the per-lane form differs (known)");
            }
            continue;
        }
        assert_eq!(
            out, expect,
            "{name}: {label}, {threads} threads (split {split})"
        );
        eprintln!("{name}: {label}, {threads} threads exact (split {split})");
    }
}

/// A mask counted whole: 40 lanes of the console wave, 32 in one half and
/// 8 in the other.
#[test]
fn masks_read_whole_count_both_halves() {
    let body = [
        0x7D88_00A8, // v_cmp_gt_u32 vcc, 40, v0
        0xBE89_106A, // s_bcnt1_i32_b64 s9, vcc
        0x7E04_0209, // v_mov_b32 v2, s9
    ];
    console_flow_case("s_bcnt1 of lanes below 40", &body, 64, |_| 40);
    // 48 threads: the upper half is partial.
    console_flow_case("s_bcnt1 of lanes below 40", &body, 48, |_| 40);
    // 32 threads: no upper half at all.
    console_flow_case("s_bcnt1 of lanes below 40", &body, 32, |_| 32);
}

/// A lane of the other half: lane 40 is the upper half's lane 8.
#[test]
fn lane_reads_cross_halves() {
    let body = [
        0xD760_0009,
        0x0001_5100, // v_readlane_b32 s9, v0, 40
        0x7E04_0209, // v_mov_b32 v2, s9
    ];
    console_flow_case("v_readlane 40", &body, 64, |_| 40);
}

/// A branch only lanes below 20 take (all in the lower half), with a lane
/// read inside: the upper half has no lane there but still meets the lower
/// one at the exchange.
#[test]
fn exchanges_inside_a_branch_one_half_skips() {
    let body = [
        0x7E04_0281, // v_mov_b32 v2, 1
        0x7D88_0094, // v_cmp_gt_u32 vcc, 20, v0
        0xBE8A_246A, // s_and_saveexec_b64 s[10:11], vcc
        0xBF88_0003, // s_cbranch_execz +3
        0xD760_0009,
        0x0001_1500, // v_readlane_b32 s9, v0, 10
        0x7E04_0209, // v_mov_b32 v2, s9
        0x88FE_0A7E, // s_or_b64 exec, exec, s[10:11]
    ];
    console_flow_case("if (lane < 20) v2 = readlane(10)", &body, 64, |t| {
        if t < 20 { 10 } else { 1 }
    });
}

/// Groups of several console waves need the halves to meet without a
/// group barrier: not yet.
#[test]
fn console_flow_is_for_one_console_wave_per_group() {
    let body = [
        0xD760_0009,
        0x0001_5100, // v_readlane_b32 s9, v0, 40
        0x7E04_0209, // v_mov_b32 v2, s9
    ];
    let nvidia = super::Target {
        lanes_min: 32,
        lanes_max: 32,
    };
    assert!(emit_for(&program(&body, 64), nvidia).is_ok());
    assert!(emit_for(&program(&body, 128), nvidia).is_err());
}

/// A loop lane t runs t times, with a lane read of the other half in
/// every iteration: the lower half's lanes are done after 31 iterations,
/// the upper half's go on to 63, and the console wave loops while any lane
/// does. After it, s9 holds lane 40's id, as the last iteration read it.
///
/// The per-lane form got this wrong on every host (#225); hosts with
/// 64-lane waves now run it in the console flow too, without exchanges.
#[test]
fn loops_run_while_any_lane_of_the_console_wave_does() {
    let body = [
        0xBE89_0380, // s_mov_b32 s9, 0
        0x7E04_0280, // v_mov_b32 v2, 0
        0x7D8A_0080, // v_cmp_ne_u32 vcc, 0, v0
        0xBE8A_246A, // s_and_saveexec_b64 s[10:11], vcc
        0xBF88_0006, // s_cbranch_execz end
        // loop:
        0xD760_0009,
        0x0001_5100, // v_readlane_b32 s9, v0, 40
        0x4A04_0481, // v_add_nc_u32 v2, 1, v2
        0x7D82_0102, // v_cmp_lt_u32 vcc, v2, v0
        0x87FE_6A7E, // s_and_b64 exec, exec, vcc
        0xBF89_FFFA, // s_cbranch_execnz loop
        // end:
        0x88FE_0A7E, // s_or_b64 exec, exec, s[10:11]
        0x4A04_0409, // v_add_nc_u32 v2, s9, v2
    ];
    console_flow_case("loop t times reading lane 40", &body, 64, |t| t + 40);
}

/// DPP's row mask names the console's rows: with `row_mask:0x3` only
/// rows 0 and 1 (lanes 0 to 31) write, so the upper half keeps its value
/// although its host rows are 0 and 1 too.
#[test]
fn dpp_row_masks_name_console_rows() {
    let body = [
        0x7E04_0281, // v_mov_b32 v2, 1
        0x7E04_02FA,
        0x3F00_E400, // v_mov_b32_dpp v2, v0 quad_perm:[0,1,2,3] row_mask:0x3 bank_mask:0xf
    ];
    console_flow_case(
        "DPP row_mask 0x3",
        &body,
        64,
        |t| if t < 32 { t } else { 1 },
    );
}

/// `v_writelane` ignores EXEC: inside a branch only lanes below 20 take,
/// it still writes lane 40. The per-lane form never had lane 40 there.
#[test]
fn lane_writes_reach_lanes_outside_the_branch() {
    let body = [
        0x7E04_0281, // v_mov_b32 v2, 1
        0x7D88_0094, // v_cmp_gt_u32 vcc, 20, v0
        0xBE8A_246A, // s_and_saveexec_b64 s[10:11], vcc
        0xBF88_0004, // s_cbranch_execz +4
        0xBE89_03FF,
        0x0000_004D, // s_mov_b32 s9, 77
        0xD761_0002,
        0x0001_5009, // v_writelane_b32 v2, s9, 40
        0x88FE_0A7E, // s_or_b64 exec, exec, s[10:11]
    ];
    console_flow_case("if (lane < 20) writelane(40, 77)", &body, 64, |t| {
        if t == 40 { 77 } else { 1 }
    });
}

/// The loop form compilers emit: lanes that are done are added to an exit
/// mask that starts as 0, EXEC loses them, and the mask restores EXEC
/// after the loop. Lane t runs t + 1 iterations with a read of lane 40 in
/// each; afterwards every lane has s9 = 40 and counts all 64 lanes in the
/// exit mask.
#[test]
fn loops_with_an_exit_mask_grown_from_zero() {
    let body = [
        0x4A02_0081, // v_add_nc_u32 v1, 1, v0
        0xBE8A_0480, // s_mov_b64 s[10:11], 0
        0xBE89_0380, // s_mov_b32 s9, 0
        // loop:
        0xD760_0009,
        0x0001_5100, // v_readlane_b32 s9, v0, 40
        0x4E02_0281, // v_subrev_nc_u32 v1, 1, v1
        0x7D84_0280, // v_cmp_eq_u32 vcc, 0, v1
        0x888A_0A6A, // s_or_b64 s[10:11], vcc, s[10:11]
        0x8AFE_0A7E, // s_andn2_b64 exec, exec, s[10:11]
        0xBF89_FFF9, // s_cbranch_execnz loop
        0x88FE_0A7E, // s_or_b64 exec, exec, s[10:11]
        0xBE8C_100A, // s_bcnt1_i32_b64 s12, s[10:11]
        0x7E04_0209, // v_mov_b32 v2, s9
        0x4A04_040C, // v_add_nc_u32 v2, s12, v2
    ];
    console_flow_case("exit-mask loop reading lane 40", &body, 64, |_| 40 + 64);
}
