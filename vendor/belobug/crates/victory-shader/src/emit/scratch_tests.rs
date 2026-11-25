//! Private (scratch) memory: synthetic GFX10 compute programs whose lanes
//! keep data in `scratch_*` memory, lowered to DXIL, validated by
//! `dxil.dll` when present, and run on the host GPU and WARP against
//! values computed here.

use super::host::{Dispatch, Host, Root, View};
use super::{HostShader, lower, validator};
use crate::native::{
    ComputeIo, HwStage, NativeProgram, ResourceType, RootParam, RootType, StageIo, UsedResource,
};
use crate::{io, ir, resource, structure};

const S_ENDPGM: u32 = 0xBF81_0000;
/// Lanes per thread group: two 32-lane waves.
const THREADS: u32 = 64;
/// `saddr` of a scratch access with a VGPR address (`off`).
const OFF: u8 = 125;
/// `saddr` of a scratch access with neither address register.
const NONE: u8 = 0x7F;
const LIT: u16 = 255;

const fn sgpr(n: u8) -> u16 {
    n as u16
}

/// An inline integer constant (0-64).
const fn int(n: u8) -> u16 {
    128 + n as u16
}

/// SOP1 `s_mov_b32 sdst, src0`.
const fn s_mov_b32(sdst: u8, src0: u8) -> u32 {
    0xBE80_0000 | (sdst as u32) << 16 | 0x03 << 8 | src0 as u32
}

/// SMEM `s_load_dwordx4 s[dst:dst+3], s[base:base+1], 0`.
const fn s_load_dwordx4(dst: u8, base: u8) -> [u32; 2] {
    [
        0x3D << 26 | 0x02 << 18 | (dst as u32) << 6 | (base as u32 >> 1),
        125 << 25,
    ]
}

/// VOP1 `op vdst, src0` (`src0`: 0-105 SGPRs, 128+ constants).
const fn vop1(op: u16, vdst: u8, src0: u16) -> u32 {
    0x7E00_0000 | (vdst as u32) << 17 | (op as u32) << 9 | (src0 as u32 & 0x1FF)
}

/// VOP1 `v_mov_b32 vdst, v<src>`.
const fn v_mov_v(vdst: u8, src: u8) -> u32 {
    vop1(0x001, vdst, 256 + src as u16)
}

/// VOP2 `op vdst, src0, v<vsrc1>`.
const fn vop2(op: u8, vdst: u8, src0: u16, vsrc1: u8) -> u32 {
    (op as u32) << 25 | (vdst as u32) << 17 | (vsrc1 as u32) << 9 | (src0 as u32 & 0x1FF)
}

/// FLAT-encoded `scratch_*` (segment 1): `op vdst, vaddr, vdata, saddr
/// offset:offset`.
const fn scratch(op: u8, vdst: u8, vaddr: u8, vdata: u8, saddr: u8, offset: i16) -> [u32; 2] {
    [
        0x37 << 26 | (op as u32) << 18 | 1 << 14 | (offset as u32 & 0xFFF),
        (vdst as u32) << 24 | (saddr as u32) << 16 | (vdata as u32) << 8 | vaddr as u32,
    ]
}

/// A scratch load at `offset` with neither address register.
const fn load(op: u8, vdst: u8, offset: i16) -> [u32; 2] {
    scratch(op, vdst, 0, 0, NONE, offset)
}

/// A scratch store of `vdata` at `offset` with neither address register.
const fn store(op: u8, vdata: u8, offset: i16) -> [u32; 2] {
    scratch(op, 0, 0, vdata, NONE, offset)
}

// Scratch opcodes (RDNA 2 ISA, FLAT/GLOBAL/SCRATCH table).
const LOAD_UBYTE: u8 = 0x08;
const LOAD_SBYTE: u8 = 0x09;
const LOAD_USHORT: u8 = 0x0a;
const LOAD_SSHORT: u8 = 0x0b;
const LOAD_DWORD: u8 = 0x0c;
const LOAD_DWORDX2: u8 = 0x0d;
const LOAD_DWORDX4: u8 = 0x0e;
const LOAD_DWORDX3: u8 = 0x0f;
const STORE_BYTE: u8 = 0x18;
const STORE_BYTE_D16_HI: u8 = 0x19;
const STORE_SHORT: u8 = 0x1a;
const STORE_SHORT_D16_HI: u8 = 0x1b;
const STORE_DWORD: u8 = 0x1c;
const STORE_DWORDX2: u8 = 0x1d;
const STORE_DWORDX4: u8 = 0x1e;
const STORE_DWORDX3: u8 = 0x1f;
const LOAD_UBYTE_D16: u8 = 0x20;
const LOAD_SBYTE_D16_HI: u8 = 0x23;
const LOAD_SHORT_D16: u8 = 0x24;
const LOAD_SHORT_D16_HI: u8 = 0x25;

/// A 64-lane compute program (two 32-lane waves) with 4 root constants in
/// `s0..s3` and the thread id in `v0`: `body`, then VGPRs `outputs` stored
/// to dwords `lane * outputs.len() + k` of the raw UAV at heap index 0.
fn build_cs(body: &[u32], outputs: &[u8]) -> HostShader {
    let mut words = vec![s_mov_b32(5, 0x80)];
    words.extend_from_slice(&s_load_dwordx4(8, 4));
    words.extend_from_slice(body);
    // v40 = the lane's first output byte.
    words.extend([vop2(0x0b, 40, LIT, 0), 4 * outputs.len() as u32]);
    for (k, &v) in outputs.iter().enumerate() {
        // buffer_store_dword v, v40, s[8:11], 0 offen offset:4k
        words.extend([
            0x38 << 26 | 0x1C << 18 | 1 << 12 | (4 * k as u32),
            0x80 << 24 | 2 << 16 | (v as u32) << 8 | 40,
        ]);
    }
    words.push(S_ENDPGM);
    let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let native = NativeProgram {
        stage: HwStage::Cs,
        wave64: false,
        vgprs: 48,
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
            rsrc2: 5 << 1,
            threads: [THREADS, 1, 1],
        }),
        code,
    };
    let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
    let res = resource::track(&p, &native.layout, native.stage).expect("resource::track");
    let io = io::raise(&p, &native).expect("io::raise");
    let st = structure::structure(&p).expect("structure");
    let lowered = lower::lower(&lower::Parts {
        native: &native,
        p: &p,
        st: &st,
        io: &io,
        res: &res,
        inputs: Vec::new(),
        outputs: Vec::new(),
        prims: Vec::new(),
        mesh_stride: 0,
        target: crate::emit::Target::ANY,
        psv: None,
        console_cf: false,
        per_lane_only: false,
    })
    .expect("lower");
    let shader = HostShader {
        stage: HwStage::Cs,
        dxil: lowered.dxil,
        user_data: lowered.user_data,
        samplers: lowered.samplers,
        static_samplers: Vec::new(),
        root_cbvs: lowered.root_cbvs,
        root_srvs: lowered.root_srvs,
        root_uavs: lowered.root_uavs,
        stride_table: lowered.stride_table,
    };
    if let Ok(mut v) = validator::Validator::load(validator::Validator::default_path().as_deref()) {
        let mut copy = shader.dxil.clone();
        let ours = copy[4..20].to_vec();
        v.validate(&mut copy).expect("DXIL validation failed");
        assert_eq!(copy[4..20], ours[..], "container hash mismatch");
    }
    shader
}

/// Runs `shader` on the hardware GPU and WARP for each set of root
/// constants and compares every lane's outputs with `want(constants, lane)`.
fn run(
    shader: &HostShader,
    outputs: usize,
    cases: &[[u32; 4]],
    want: impl Fn([u32; 4], u32) -> Vec<u32>,
) {
    let mut ran = 0;
    for (label, warp) in [("hardware", false), ("warp", true)] {
        let Ok(host) = Host::new(warp) else {
            eprintln!("no SM 6.6 {label} device; skipped");
            continue;
        };
        let (root, pso) = host
            .compute_pipeline(shader)
            .unwrap_or_else(|e| panic!("{label}: compute_pipeline failed: {e}"));
        for &k in cases {
            let mut ud = vec![0u32; shader.user_data_dwords().max(1) as usize];
            for u in &shader.user_data {
                ud[u.index as usize] = match (u.root, u.slot) {
                    (RootType::Constants, 0) => k[u.dword as usize],
                    (RootType::Table, 1) => 0,
                    other => panic!("unexpected user data {other:?}"),
                };
            }
            let out = host
                .dispatch(
                    &root,
                    &pso,
                    &Dispatch {
                        roots: vec![(0, Root::Constants(ud))],
                        views: &[View::RawUav(THREADS * outputs as u32)],
                        samplers: &[],
                        groups: [1, 1, 1],
                    },
                )
                .unwrap_or_else(|e| panic!("{label}: dispatch failed: {e}"));
            for lane in 0..THREADS {
                let at = lane as usize * outputs;
                let got: Vec<String> = out[0][at..at + outputs]
                    .iter()
                    .map(|x| format!("{x:#010x}"))
                    .collect();
                let want: Vec<String> =
                    want(k, lane).iter().map(|x| format!("{x:#010x}")).collect();
                assert_eq!(got, want, "{label}: lane {lane}, constants {k:x?}");
            }
        }
        ran += 1;
    }
    assert!(ran > 0, "neither hardware nor WARP SM 6.6 device available");
}

#[test]
fn a_private_array_is_per_lane_and_dynamically_indexed() {
    let mut body = vec![
        // v1 = lane * 0x10001 + k0
        vop2(0x0b, 1, LIT, 0),
        0x10001,
        vop2(0x25, 1, sgpr(0), 1),
        vop1(0x001, 2, int(0)),
    ];
    // a[j] = v1 + j for j < 8, through a VGPR address.
    for j in 0..8u8 {
        body.push(vop2(0x25, 3, int(j), 1));
        body.extend(scratch(STORE_DWORD, 0, 2, 3, OFF, 4 * i16::from(j)));
    }
    body.extend([
        // v4 = ((lane ^ k1) & 7) * 4: another lane's index in most lanes
        vop2(0x1d, 4, sgpr(1), 0),
        vop2(0x1b, 4, int(7), 4),
        vop2(0x1a, 4, int(2), 4),
    ]);
    body.extend(scratch(LOAD_DWORD, 5, 4, 0, OFF, 0));
    // Read it back again from the end of the array (offset -28 off v6 = 28).
    body.push(vop1(0x001, 6, int(28)));
    body.extend(scratch(LOAD_DWORD, 7, 6, 0, OFF, -28));
    let shader = build_cs(&body, &[5, 7]);
    let cases = [
        [0, 0, 0, 0],
        [0x1234_0000, 5, 0, 0],
        [0xDEAD_0000, 0xFFFF_FFFF, 0, 0],
    ];
    run(&shader, 2, &cases, |k, lane| {
        let v1 = lane.wrapping_mul(0x10001).wrapping_add(k[0]);
        vec![v1 + ((lane ^ k[1]) & 7), v1]
    });
}

#[test]
fn wide_accesses_through_an_sgpr_base_and_out_of_range() {
    let mut body = vec![
        // v1 = lane + k0; a[0] = v1 (no address register).
        vop2(0x25, 1, sgpr(0), 0),
    ];
    body.extend(store(STORE_DWORD, 1, 0));
    // v[6:9] = v1 + 1..4, stored at k2 + 16 (an SGPR base), then two
    // dwords read back at k2 + 20.
    for j in 0..4u8 {
        body.push(vop2(0x25, 6 + j, int(j + 1), 1));
    }
    body.push(s_mov_b32(6, 2));
    body.extend(scratch(STORE_DWORDX4, 0, 0, 6, 6, 16));
    body.extend(scratch(LOAD_DWORDX2, 10, 0, 0, 6, 20));
    body.extend(load(LOAD_DWORD, 12, 0));
    // x3 and x2 stores and an x4 and x3 load at a constant place.
    body.extend(store(STORE_DWORDX3, 7, 64));
    body.extend(store(STORE_DWORDX2, 6, 76));
    body.extend(load(LOAD_DWORDX4, 13, 64));
    body.extend(load(LOAD_DWORDX3, 17, 68));
    let shader = build_cs(&body, &[10, 11, 12, 13, 14, 15, 16, 17, 18, 19]);
    let cap = lower::SCRATCH_CAP;
    let cases = [
        [7, 0, 0, 0],
        [7, 0, 64, 0],
        // The store's last two dwords and the load's second are past the end.
        [9, 0, cap - 24, 0],
        // The store's first dword would be dword 0 if the index wrapped.
        [9, 0, 0x3FFF_FFF0, 0],
        // -16: the address wraps to 0, in range.
        [3, 0, 0xFFFF_FFF0, 0],
    ];
    run(&shader, 10, &cases, |k, lane| {
        let v1 = lane + k[0];
        let mut mem = vec![0u32; cap as usize / 4];
        mem[0] = v1;
        for j in 0..4 {
            let at = k[2].wrapping_add(16 + 4 * j) as usize / 4;
            if at < mem.len() {
                mem[at] = v1 + 1 + j;
            }
        }
        let read = |mem: &[u32], byte: u32| mem.get(byte as usize / 4).copied().unwrap_or(0);
        let mut out = vec![
            read(&mem, k[2].wrapping_add(20)),
            read(&mem, k[2].wrapping_add(24)),
        ];
        out.push(read(&mem, 0));
        mem[16..19].copy_from_slice(&[v1 + 2, v1 + 3, v1 + 4]);
        mem[19..21].copy_from_slice(&[v1 + 1, v1 + 2]);
        out.extend_from_slice(&mem[16..20]);
        out.extend_from_slice(&mem[17..20]);
        out
    });
}

#[test]
fn bytes_and_shorts_extend_and_keep_the_other_half() {
    let mut body = vec![
        // v1 = k0 ^ lane, a[0] = v1, a[1] = a[2] = k3.
        vop2(0x1d, 1, sgpr(0), 0),
        vop1(0x001, 10, sgpr(3)),
        vop1(0x001, 12, int(8)),
    ];
    body.extend(store(STORE_DWORD, 1, 0));
    body.extend(store(STORE_DWORD, 10, 4));
    body.extend(scratch(STORE_DWORD, 0, 12, 10, OFF, 0));
    body.extend(load(LOAD_UBYTE, 2, 0));
    body.extend(load(LOAD_SBYTE, 3, 1));
    body.extend(load(LOAD_USHORT, 4, 2));
    body.extend(load(LOAD_SSHORT, 5, 2));
    // D16 loads into VGPRs holding k3.
    for v in 6..10 {
        body.push(v_mov_v(v, 10));
    }
    body.extend(load(LOAD_SHORT_D16, 6, 0));
    body.extend(load(LOAD_SBYTE_D16_HI, 7, 3));
    body.extend(load(LOAD_UBYTE_D16, 8, 1));
    body.extend(load(LOAD_SHORT_D16_HI, 9, 2));
    // Into a[1]: byte 1, bytes 2-3 from v1's high half, byte 0 from its
    // bits 16-23; into a[2] (VGPR address): its low half.
    body.extend(store(STORE_BYTE, 1, 5));
    body.extend(store(STORE_SHORT_D16_HI, 1, 6));
    body.extend(store(STORE_BYTE_D16_HI, 1, 4));
    body.extend(load(LOAD_DWORD, 11, 4));
    body.extend(scratch(STORE_SHORT, 0, 12, 1, OFF, 0));
    body.extend(scratch(LOAD_DWORD, 13, 12, 0, OFF, 0));
    let shader = build_cs(&body, &[2, 3, 4, 5, 6, 7, 8, 9, 11, 13]);
    let cases = [
        [0x80FF_7F01, 0, 0, 0xAAAA_BBBB],
        [0x7F80_FF80, 0, 0, 0x1234_5678],
        [0x0000_0000, 0, 0, 0xFFFF_FFFF],
    ];
    run(&shader, 10, &cases, |k, lane| {
        let v1 = k[0] ^ lane;
        let b = |n: u32| v1 >> (8 * n) & 0xFF;
        let sb = |n: u32| (b(n) as u8 as i8) as i32 as u32;
        let half1 = v1 >> 16;
        let (hi3, lo3) = (k[3] & 0xFFFF_0000, k[3] & 0xFFFF);
        vec![
            b(0),
            sb(1),
            half1,
            (half1 as u16 as i16) as i32 as u32,
            hi3 | (v1 & 0xFFFF),
            (sb(3) & 0xFFFF) << 16 | lo3,
            hi3 | b(1),
            half1 << 16 | lo3,
            (v1 & 0xFFFF_0000) | b(0) << 8 | (v1 >> 16 & 0xFF),
            hi3 | (v1 & 0xFFFF),
        ]
    });
}

#[test]
fn a_store_in_lanes_off_does_not_happen() {
    let mut body = vec![
        // v1 = k0 + lane; a[0] = k1, then a[0] = v1 where lane < k2.
        vop2(0x25, 1, sgpr(0), 0),
        vop1(0x001, 2, sgpr(1)),
    ];
    body.extend(store(STORE_DWORD, 2, 0));
    body.extend([
        0x3E << 25 | 0xC4 << 17 | sgpr(2) as u32, // v_cmp_gt_u32 vcc, s2, v0
        0xBE87_3C6A,                              // s_and_saveexec_b32 s7, vcc_lo
        0xBF88_0002,                              // s_cbranch_execz over the store
    ]);
    body.extend(store(STORE_DWORD, 1, 0));
    body.push(0x887E_077E); // s_or_b32 exec_lo, exec_lo, s7
    body.extend(load(LOAD_DWORD, 3, 0));
    let shader = build_cs(&body, &[3]);
    let cases = [
        [100, 7, 0, 0],
        [100, 7, 5, 0],
        [100, 7, 33, 0],
        [100, 7, 64, 0],
    ];
    run(&shader, 1, &cases, |k, lane| {
        vec![if lane < k[2] { k[0] + lane } else { k[1] }]
    });
}
