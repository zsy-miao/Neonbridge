//! Byte and halfword LDS accesses: synthetic GFX10 compute programs lowered
//! to DXIL (validated by `dxil.dll` when present) and run on the host GPU and
//! WARP, against a model of the console's LDS.

use super::host::{Dispatch, Host, Root, View};
use super::{HostShader, lower, validator};
use crate::native::{
    ComputeIo, HwStage, NativeProgram, ResourceType, RootParam, RootType, StageIo, UsedResource,
};
use crate::{io, ir, resource, structure};

const S_ENDPGM: u32 = 0xBF81_0000;
/// `s_waitcnt` with every counter at 0.
const S_WAITCNT_0: u32 = 0xBF8C_0000;
const S_BARRIER: u32 = 0xBF8A_0000;

/// Inline constants as source operands.
const fn int(n: i8) -> u16 {
    if n >= 0 {
        128 + n as u16
    } else {
        (192 - n as i16) as u16
    }
}

const fn vgpr(n: u8) -> u16 {
    256 + n as u16
}

/// SOP1 `s_mov_b32 sdst, src0`.
const fn s_mov_b32(sdst: u8, src0: u16) -> u32 {
    0xBE80_0000 | (sdst as u32) << 16 | 0x03 << 8 | (src0 as u32 & 0xFF)
}

/// SMEM `s_load_dwordx4 s[dst:dst+3], s[base:base+1], 0`.
const fn s_load_dwordx4(dst: u8, base: u8) -> [u32; 2] {
    [
        0x3D << 26 | 0x02 << 18 | (dst as u32) << 6 | (base as u32 >> 1),
        125 << 25,
    ]
}

/// VOP1 `op vdst, src0`.
const fn vop1(op: u16, vdst: u8, src0: u16) -> u32 {
    0x7E00_0000 | (vdst as u32) << 17 | (op as u32) << 9 | (src0 as u32 & 0x1FF)
}

/// VOP2 `op vdst, src0, vsrc1`.
const fn vop2(op: u8, vdst: u8, src0: u16, vsrc1: u8) -> u32 {
    (op as u32) << 25 | (vdst as u32) << 17 | (vsrc1 as u32) << 9 | (src0 as u32 & 0x1FF)
}

const fn v_mov_b32(vdst: u8, src0: u16) -> u32 {
    vop1(0x001, vdst, src0)
}
const fn v_lshlrev_b32(vdst: u8, shift: u16, v: u8) -> u32 {
    vop2(0x1A, vdst, shift, v)
}
const fn v_and_b32(vdst: u8, src0: u16, v: u8) -> u32 {
    vop2(0x1B, vdst, src0, v)
}
const fn v_add_nc_u32(vdst: u8, src0: u16, v: u8) -> u32 {
    vop2(0x25, vdst, src0, v)
}

/// DS `op [vdst,] addr [, data0] offset:offset`.
const fn ds(op: u8, vdst: u8, addr: u8, data0: u8, offset: u16) -> [u32; 2] {
    [
        0x36 << 26 | (op as u32) << 18 | offset as u32,
        (vdst as u32) << 24 | (data0 as u32) << 8 | addr as u32,
    ]
}

/// MUBUF `buffer_store_dword vdata, vaddr, s[srsrc:srsrc+3], 0 offen`.
const fn buffer_store_dword_offen(vdata: u8, vaddr: u8, srsrc: u8) -> [u32; 2] {
    [
        0x38 << 26 | 0x1C << 18 | 1 << 12,
        0x80 << 24 | ((srsrc as u32 >> 2) & 0x1F) << 16 | (vdata as u32) << 8 | vaddr as u32,
    ]
}

const DS_WRITE_B32: u8 = 0x0D;
const DS_READ_B32: u8 = 0x36;

/// The byte and halfword opcodes: (name, opcode).
const READS: &[(&str, u8)] = &[
    ("ds_read_i8", 0x39),
    ("ds_read_u8", 0x3A),
    ("ds_read_i16", 0x3B),
    ("ds_read_u16", 0x3C),
    ("ds_read_u8_d16", 0xA2),
    ("ds_read_u8_d16_hi", 0xA3),
    ("ds_read_i8_d16", 0xA4),
    ("ds_read_i8_d16_hi", 0xA5),
    ("ds_read_u16_d16", 0xA6),
    ("ds_read_u16_d16_hi", 0xA7),
];
const WRITES: &[(&str, u8)] = &[
    ("ds_write_b8", 0x1E),
    ("ds_write_b16", 0x1F),
    ("ds_write_b8_d16_hi", 0xA0),
    ("ds_write_b16_d16_hi", 0xA1),
];

/// A compute program of `threads` lanes (waves of 32) with 512 bytes of
/// LDS: the thread id in `v0`, the 4 root constants in `v1..v4`; `body`
/// leaves each lane's result in `v5`, stored to dword `thread id` of the
/// raw UAV at heap index 0.
fn build_cs(body: &[u32], threads: u32) -> HostShader {
    let mut words = vec![s_mov_b32(5, int(0))];
    words.extend_from_slice(&s_load_dwordx4(8, 4));
    for i in 0..4u8 {
        words.push(v_mov_b32(i + 1, u16::from(i)));
    }
    words.extend_from_slice(body);
    words.push(v_lshlrev_b32(6, int(2), 0));
    words.extend_from_slice(&buffer_store_dword_offen(5, 6, 8));
    words.push(S_ENDPGM);

    let native = NativeProgram {
        stage: HwStage::Cs,
        wave64: false,
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
            // 5 user SGPRs, thread id x only, one 512-byte LDS granule.
            rsrc2: 5 << 1 | 1 << 15,
            threads: [threads, 1, 1],
        }),
        code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
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
        v.validate(&mut copy).expect("DXIL validation");
        assert_eq!(copy[4..20], ours[..], "container hash");
    }
    shader
}

/// Each available host (GPU, WARP), for running shaders on.
fn hosts() -> Vec<(&'static str, Host)> {
    let hosts: Vec<_> = [("hardware", false), ("warp", true)]
        .into_iter()
        .filter_map(|(label, warp)| match Host::new(warp) {
            Ok(h) => Some((label, h)),
            Err(_) => {
                eprintln!("no SM 6.6 {label} device; skipped");
                None
            }
        })
        .collect();
    hosts
}

/// Runs `shader` with root constants `k` and returns the first `threads`
/// dwords of its UAV.
fn run(label: &str, host: &Host, shader: &HostShader, k: [u32; 4], threads: u32) -> Vec<u32> {
    let (root, pso) = host
        .compute_pipeline(shader)
        .unwrap_or_else(|e| panic!("{label}: pipeline: {e}"));
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
                views: &[View::RawUav(threads)],
                samplers: &[],
                groups: [1, 1, 1],
            },
        )
        .unwrap_or_else(|e| panic!("{label}: dispatch: {e}"));
    out[0][..threads as usize].to_vec()
}

/// Accesses a halfword (else a byte); `_d16` names a VGPR half, not a width.
fn halfword(name: &str) -> bool {
    ["u16", "i16", "b16"]
        .iter()
        .any(|t| name.split('_').any(|p| p == *t))
}

/// The console's result of a byte or halfword read of `lds` at `at`, into
/// a VGPR holding `old`.
fn model_read(name: &str, lds: &[u8], at: usize, old: u32) -> u32 {
    let wide = halfword(name);
    let x = if wide {
        u32::from(u16::from_le_bytes([lds[at], lds[at + 1]]))
    } else {
        u32::from(lds[at])
    };
    let x = match (name.contains("_i"), wide) {
        (true, true) => x as u16 as i16 as i32 as u32,
        (true, false) => x as u8 as i8 as i32 as u32,
        _ => x,
    };
    if name.ends_with("_d16_hi") {
        old & 0xFFFF | x << 16
    } else if name.ends_with("_d16") {
        old & 0xFFFF_0000 | x & 0xFFFF
    } else {
        x
    }
}

fn model_write(name: &str, lds: &mut [u8], at: usize, data: u32) {
    let d = if name.ends_with("_d16_hi") {
        data >> 16
    } else {
        data
    };
    let n = if halfword(name) { 2 } else { 1 };
    lds[at..at + n].copy_from_slice(&d.to_le_bytes()[..n]);
}

/// Two waves: no host wave size is asked for (WARP has no 32-lane waves),
/// and LDS is shared across waves.
const LANES: u32 = 64;

const DWORD0: u32 = 0x8281_7F01;
const DWORD1: u32 = 0xFEDC_BA98;
const OLD: u32 = 0x1234_5678;

fn initial_lds() -> Vec<u8> {
    [DWORD0, DWORD1]
        .iter()
        .flat_map(|d| d.to_le_bytes())
        .collect()
}

/// Byte addresses (before the instruction's offset of 2) for an access of
/// `name`'s width: every byte of two dwords, or every halfword.
fn addresses(name: &str) -> Vec<u32> {
    let step = if halfword(name) { 2 } else { 1 };
    (0..6).step_by(step).collect()
}

#[test]
fn byte_and_halfword_reads() {
    let hosts = hosts();
    for &(name, op) in READS {
        // Two dwords of LDS from v1/v2, then the read at v3 + 2 into v5,
        // which holds v4 first (the half a d16 read keeps).
        let mut body = vec![v_mov_b32(7, int(0))];
        body.extend_from_slice(&ds(DS_WRITE_B32, 0, 7, 1, 0));
        body.extend_from_slice(&ds(DS_WRITE_B32, 0, 7, 2, 4));
        body.push(S_WAITCNT_0);
        body.push(S_BARRIER);
        body.push(v_mov_b32(5, vgpr(4)));
        body.extend_from_slice(&ds(op, 5, 3, 0, 2));
        body.push(S_WAITCNT_0);
        let shader = build_cs(&body, LANES);
        let lds = initial_lds();
        for (label, host) in &hosts {
            for at in addresses(name) {
                let got = run(label, host, &shader, [DWORD0, DWORD1, at, OLD], LANES);
                let want = model_read(name, &lds, at as usize + 2, OLD);
                for (lane, g) in got.into_iter().enumerate() {
                    assert_eq!(
                        g, want,
                        "{label}: {name} at {at}+2, lane {lane}: {g:#010x}, want {want:#010x}"
                    );
                }
            }
        }
    }
}

#[test]
fn byte_and_halfword_writes() {
    let hosts = hosts();
    for &(name, op) in WRITES {
        // Two dwords of LDS from v1/v2, the write of v4 at v3 + 2 (the same
        // in every lane), then the dword holding it read back.
        let mut body = vec![v_mov_b32(7, int(0))];
        body.extend_from_slice(&ds(DS_WRITE_B32, 0, 7, 1, 0));
        body.extend_from_slice(&ds(DS_WRITE_B32, 0, 7, 2, 4));
        body.push(S_WAITCNT_0);
        body.push(S_BARRIER);
        body.extend_from_slice(&ds(op, 0, 3, 4, 2));
        body.push(S_WAITCNT_0);
        body.push(S_BARRIER);
        body.push(v_add_nc_u32(9, int(2), 3));
        body.push(v_and_b32(9, int(-4), 9));
        body.extend_from_slice(&ds(DS_READ_B32, 5, 9, 0, 0));
        body.push(S_WAITCNT_0);
        let shader = build_cs(&body, LANES);
        for (label, host) in &hosts {
            for at in addresses(name) {
                let at = at as usize + 2;
                let mut lds = initial_lds();
                model_write(name, &mut lds, at, OLD);
                let d = at & !3;
                let want = u32::from_le_bytes(lds[d..d + 4].try_into().unwrap());
                let k = [DWORD0, DWORD1, at as u32 - 2, OLD];
                for (lane, g) in run(label, host, &shader, k, LANES).into_iter().enumerate() {
                    assert_eq!(
                        g, want,
                        "{label}: {name} at {at}, lane {lane}: {g:#010x}, want {want:#010x}"
                    );
                }
            }
        }
    }
}

/// Every lane writes its own bytes of dwords the other lanes write too:
/// none of them is lost.
#[test]
fn lanes_share_dwords() {
    let hosts = hosts();
    for &(name, op) in WRITES {
        let wide = halfword(name);
        // v10 = thread id + v1 (in the high half for _d16_hi), written at
        // thread id × width; then each lane reads back a dword of them.
        let mut body = vec![v_add_nc_u32(10, vgpr(0), 1)];
        if name.ends_with("_d16_hi") {
            body.push(v_lshlrev_b32(10, int(16), 10));
        }
        body.push(if wide {
            v_lshlrev_b32(11, int(1), 0)
        } else {
            v_mov_b32(11, vgpr(0))
        });
        body.extend_from_slice(&ds(op, 0, 11, 10, 0));
        body.push(S_WAITCNT_0);
        body.push(S_BARRIER);
        let dwords = LANES * if wide { 2 } else { 1 } / 4;
        body.push(v_and_b32(9, int(dwords as i8 - 1), 0));
        body.push(v_lshlrev_b32(9, int(2), 9));
        body.extend_from_slice(&ds(DS_READ_B32, 5, 9, 0, 0));
        body.push(S_WAITCNT_0);
        let shader = build_cs(&body, LANES);
        let base = 0x0000_A0F0;
        let mut lds = vec![0u8; LANES as usize * 2];
        for lane in 0..LANES {
            let data = lane + base;
            let data = if name.ends_with("_d16_hi") {
                data << 16
            } else {
                data
            };
            let at = (lane * if wide { 2 } else { 1 }) as usize;
            model_write(name, &mut lds, at, data);
        }
        for (label, host) in &hosts {
            let got = run(label, host, &shader, [base, 0, 0, 0], LANES);
            for (lane, &g) in got.iter().enumerate() {
                let d = (lane % dwords as usize) * 4;
                let want = u32::from_le_bytes(lds[d..d + 4].try_into().unwrap());
                assert_eq!(
                    g, want,
                    "{label}: {name}, lane {lane}: {g:#010x}, want {want:#010x}"
                );
            }
        }
    }
}

/// The opcodes above decode to their names.
#[test]
fn opcodes() {
    let named = [("ds_write_b32", DS_WRITE_B32), ("ds_read_b32", DS_READ_B32)];
    for &(name, op) in READS.iter().chain(WRITES).chain(&named) {
        let code: Vec<u8> = ds(op, 5, 3, 4, 2)
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect();
        let code = [&code[..], &S_ENDPGM.to_le_bytes()].concat();
        let inst = &crate::gfx10::decode(&code).expect("decode")[0];
        assert_eq!(inst.name(), Some(name));
    }
}
