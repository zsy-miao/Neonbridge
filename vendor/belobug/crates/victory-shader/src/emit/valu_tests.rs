//! End-to-end tests of synthetic GFX10 VALU shaders lowered to DXIL, validated
//! by `dxil.dll` when present, and executed on the host GPU and WARP.
//!
//! Covers RDNA2 hardware semantics that differ from naive DXIL lowering:
//! `v_med3_f32`/`v_med3_f16` with NaN operands, `v_fract_f32`/`v_fract_f16`
//! near `1.0`, and turn-range reduction for `v_sin_*`/`v_cos_*`.

use super::host::{Dispatch, Host, Root, View};
use super::{HostShader, container, lower, validator};
use crate::native::{
    ComputeIo, HwStage, NativeProgram, ResourceType, RootParam, RootType, StageIo, UsedResource,
};
use crate::{io, ir, resource, structure};

const S_ENDPGM: u32 = 0xBF81_0000;

/// SOP1 `s_mov_b32 sdst, src0`.
const fn s_mov_b32(sdst: u8, src0: u8) -> u32 {
    0xBE80_0000 | (sdst as u32) << 16 | 0x03 << 8 | src0 as u32
}

/// SOP2 `op sdst, src0, src1`.
const fn sop2(op: u8, sdst: u8, src0: u8, src1: u8) -> u32 {
    0x8000_0000
        | (op as u32 & 0x7F) << 23
        | (sdst as u32 & 0x7F) << 16
        | (src1 as u32) << 8
        | src0 as u32
}

/// SMEM `s_load_dwordx4 s[dst:dst+3], s[base:base+1], 0` (soffset = null).
const fn s_load_dwordx4(dst: u8, base: u8) -> [u32; 2] {
    [
        0x3D << 26 | 0x02 << 18 | (dst as u32) << 6 | (base as u32 >> 1),
        125 << 25,
    ]
}

/// VOP1 `op vdst, src0` (`src0`: 0..105 = SGPR, 256..511 = VGPR).
const fn vop1(op: u16, vdst: u8, src0: u16) -> u32 {
    0x7E00_0000 | (vdst as u32) << 17 | (op as u32) << 9 | (src0 as u32 & 0x1FF)
}

/// VOP3 `op vdst, src0, src1, src2` (no modifiers).
const fn vop3(op: u16, vdst: u8, src0: u16, src1: u16, src2: u16) -> [u32; 2] {
    [
        0x35 << 26 | (op as u32 & 0x3FF) << 16 | vdst as u32,
        (src0 as u32 & 0x1FF) | (src1 as u32 & 0x1FF) << 9 | (src2 as u32 & 0x1FF) << 18,
    ]
}

/// MUBUF `buffer_store_dword vdata, off, s[srsrc:srsrc+3], 0 offset:byte_off`.
const fn buffer_store_dword(vdata: u8, srsrc: u8, byte_off: u16) -> [u32; 2] {
    [
        0x38 << 26 | 0x1C << 18 | (byte_off as u32 & 0xFFF),
        0x80 << 24 | ((srsrc as u32 >> 2) & 0x1F) << 16 | (vdata as u32) << 8,
    ]
}

const fn sgpr(n: u8) -> u16 {
    n as u16
}

const fn vgpr(n: u8) -> u16 {
    256 + n as u16
}

/// Builds and lowers a 1-thread synthetic RDNA2 compute shader that receives
/// 4 dwords of root constants in `s0..s3`, runs `body` (leaving its result in
/// `v0`), and stores `v0` to dword 0 of the raw UAV at heap index 0.
fn build_valu_cs(body: &[u32]) -> HostShader {
    lower_test_shader(&valu_program(body), &[])
}

fn valu_program(body: &[u32]) -> NativeProgram {
    let mut words = Vec::new();
    // s[4:5] = descriptor table address (heap index 0 -> byte offset 0).
    words.push(s_mov_b32(5, 0x80)); // inline constant 0
    words.extend_from_slice(&s_load_dwordx4(8, 4));
    // Copy root constants s0..s3 into v1..v4 so VALU instructions operate on
    // VGPRs just as they do in real shaders.
    for i in 0..4u8 {
        words.push(vop1(0x001, i + 1, sgpr(i))); // v_mov_b32
    }
    words.extend_from_slice(body);
    words.extend_from_slice(&buffer_store_dword(0, 8, 0));
    words.push(S_ENDPGM);

    let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    NativeProgram {
        stage: HwStage::Cs,
        wave64: false,
        vgprs: 8,
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
            rsrc2: 5 << 1, // 5 user SGPRs: s0..s4
            threads: [1, 1, 1],
        }),
        code,
    }
}

fn lower_test_shader(native: &NativeProgram, parts: &[(crate::FourCC, &[u8])]) -> HostShader {
    let bytes = crate::dxbc::build(parts);
    let container = crate::Container::parse(&bytes).unwrap();

    let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
    let res = resource::track(&p, &native.layout, native.stage).unwrap();
    let io = io::raise(&p, native).unwrap();
    let st = structure::structure(&p).unwrap();
    let shader = super::emit(&container, native, &p, &st, &io, &res, super::Target::ANY).expect("emit");

    if let Ok(mut v) = validator::Validator::load(validator::Validator::default_path().as_deref()) {
        let mut copy = shader.dxil.clone();
        let ours = copy[4..20].to_vec();
        v.validate(&mut copy).expect("DXIL validation failed");
        assert_eq!(copy[4..20], ours[..], "container hash mismatch");
    }

    shader
}

/// Runs `shader` on every available SM 6.6 host (hardware GPU and WARP) for
/// each 4-dword input set and invokes `check(backend, case_index, inputs, result_dword)`.
fn run_cases(
    shader: &HostShader,
    cases: &[[u32; 4]],
    check: impl FnMut(&str, usize, [u32; 4], u32),
) {
    run_with_views(shader, cases, &[View::RawUav(1)], check);
}

fn run_with_views(
    shader: &HostShader,
    cases: &[[u32; 4]],
    views: &[View<'_>],
    mut check: impl FnMut(&str, usize, [u32; 4], u32),
) {
    // One test at a time: with these tests on the hardware adapter and WARP
    // from several test threads at once, now and then none of their
    // dispatches ever completes and the test binary hangs.
    static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let mut ran = 0;
    for (label, warp) in [("hardware", false), ("warp", true)] {
        let Ok(host) = Host::new(warp) else {
            eprintln!("no SM 6.6 {label} device; skipped");
            continue;
        };
        let (root, pso) = host
            .compute_pipeline(shader)
            .unwrap_or_else(|e| panic!("{label}: compute_pipeline failed: {e}"));
        for (i, &inputs) in cases.iter().enumerate() {
            let mut ud = vec![0u32; shader.user_data_dwords().max(1) as usize];
            for u in &shader.user_data {
                ud[u.index as usize] = match (u.root, u.slot) {
                    (RootType::Constants, 0) => inputs[u.dword as usize],
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
                        views,
                        samplers: &[],
                        groups: [1, 1, 1],
                    },
                )
                .unwrap_or_else(|e| panic!("{label}: dispatch failed: {e}"));
            check(label, i, inputs, out[0][0]);
        }
        ran += 1;
    }
    assert!(ran > 0, "neither hardware nor WARP SM 6.6 device available");
}

#[test]
fn data_only_structured_srv_reads_its_bound_record_count() {
    // The second heap slot contains an SRV. Its only use is a read of dword 2.
    let mut load = s_load_dwordx4(12, 4);
    load[1] |= 32;
    let mut body = load.to_vec();
    body.push(vop1(0x001, 0, sgpr(14)));
    let mut native = valu_program(&body);
    native.used.push(UsedResource {
        kind: ResourceType::Srv,
        table_offset_dwords: 8,
        lower_bound: 109,
        stride: 16,
        ..native.used[0]
    });
    use container::{Binding, Psv};
    let binding = Binding {
        kind: 5,
        space: 1,
        register: 109,
        dxil_kind: 12,
    };
    let declarations = |bindings: &[Binding]| {
        container::psv(&Psv {
            stage: 5,
            flag: false,
            inputs: &[],
            outputs: &[],
            prims: &[],
            mesh: None,
            rectangles: false,
            bindings,
            threads: [1, 1, 1],
            wave_size: None,
        })
    };
    let psv = declarations(&[binding]);
    let alias = Binding {
        kind: 3,
        space: 2,
        dxil_kind: 10,
        ..binding
    };
    assert!(!super::declares_structured_buffer(
        &declarations(&[binding, alias]),
        109,
        false
    ));
    assert!(!super::declares_structured_buffer(&psv, 108, false));
    let shader = lower_test_shader(&native, &[(crate::FourCC::PSV0, &psv)]);
    for n in [3, 11] {
        let words = vec![0; n * 4];
        let views = [
            View::RawUav(1),
            View::Buffer {
                data: &words,
                // Strides of 16 bytes and their multiples are raw views.
                stride: super::host::Stride::Raw,
            },
        ];
        run_with_views(&shader, &[[0; 4]], &views, |device, _, _, out| {
            assert_eq!(out, n as u32, "{device} structured buffer record count");
        });
    }
}

fn is_f16_nan(bits: u16) -> bool {
    (bits & 0x7FFF) > 0x7C00
}

fn f16_to_f32(bits: u16) -> f32 {
    let sign = u32::from(bits >> 15) << 31;
    let exp = (bits >> 10) & 0x1F;
    let mant = bits & 0x3FF;
    let f32_bits = match exp {
        0 => {
            if mant == 0 {
                sign
            } else {
                let mut m = u32::from(mant);
                let mut e = 127 - 14;
                while (m & 0x400) == 0 {
                    m <<= 1;
                    e -= 1;
                }
                sign | (e << 23) | ((m & 0x3FF) << 13)
            }
        }
        0x1F => sign | (0xFF << 23) | (u32::from(mant) << 13),
        _ => sign | ((u32::from(exp) + 112) << 23) | (u32::from(mant) << 13),
    };
    f32::from_bits(f32_bits)
}

#[test]
fn packed_add_selects_halves_of_inline_constants() {
    // Add the inline constant 1/(2*pi) to v1 (zero) to read back its two halves.
    // The same GFX10 source field (248) has a different bit pattern for a
    // packed floating-point and a packed integer instruction.
    let cases = [
        // v_pk_add_f16 v0, 0x3118, v1
        ([0xcc0f4000, 0x1a0202f8], 0x0000_3118),
        // v_pk_add_f16 v0, 0x3118, v1 op_sel_hi:[0,1] (use low twice)
        ([0xcc0f4000, 0x120202f8], 0x3118_3118),
        // v_pk_add_u16 v0, 0x3e22f983, v1
        ([0xcc0a4000, 0x1a0202f8], 0x3e22_f983),
    ];
    for (words, expected) in cases {
        let shader = build_valu_cs(&words);
        run_cases(&shader, &[[0; 4]], |backend, _, _, got| {
            assert_eq!(got, expected, "{backend}: {words:08x?}");
        });
    }
}

#[test]
fn v_med3_f32_matches_rdna2_with_nan() {
    // v_med3_f32 v0, v1, v2, v3 (VOP3 opcode 0x157)
    let shader = build_valu_cs(&vop3(0x157, 0, vgpr(1), vgpr(2), vgpr(3)));
    let nan = f32::NAN.to_bits();
    let f = f32::to_bits;

    let cases: &[([u32; 4], Option<f32>)] = &[
        // Normal medians (all orderings).
        ([f(1.0), f(2.0), f(3.0), 0], Some(2.0)),
        ([f(3.0), f(1.0), f(2.0), 0], Some(2.0)),
        ([f(-5.0), f(0.0), f(5.0), 0], Some(0.0)),
        ([f(1.0), f(1.0), f(2.0), 0], Some(1.0)),
        // One NaN: RDNA2 returns min3 of the non-NaN operands.
        ([f(1.0), f(2.0), nan, 0], Some(1.0)),
        ([nan, f(1.0), f(0.0), 0], Some(0.0)),
        ([nan, f(0.0), f(1.0), 0], Some(0.0)),
        ([f(2.0), nan, f(1.0), 0], Some(1.0)),
        ([f(-3.0), nan, f(4.0), 0], Some(-3.0)),
        // Two NaNs: returns the sole non-NaN operand.
        ([nan, f(5.0), nan, 0], Some(5.0)),
        ([nan, nan, f(-2.0), 0], Some(-2.0)),
        ([f(7.0), nan, nan, 0], Some(7.0)),
        // Three NaNs: returns NaN.
        ([nan, nan, nan, 0], None),
    ];

    let inputs: Vec<[u32; 4]> = cases.iter().map(|c| c.0).collect();
    run_cases(&shader, &inputs, |backend, i, inp, got| {
        let expect = cases[i].1;
        let got_f = f32::from_bits(got);
        match expect {
            Some(want) => assert_eq!(
                got_f,
                want,
                "{backend}: v_med3_f32({:?}, {:?}, {:?}) = {got_f} ({got:#010x}), want {want}",
                f32::from_bits(inp[0]),
                f32::from_bits(inp[1]),
                f32::from_bits(inp[2]),
            ),
            None => assert!(
                got_f.is_nan(),
                "{backend}: v_med3_f32(NaN, NaN, NaN) = {got_f} ({got:#010x}), want NaN",
            ),
        }
    });
}

/// RDNA2 orders -0 below +0 in its maximums and minimums; DXIL's FMax and
/// FMin compare and select, which can give either zero.
#[test]
fn max_and_min_order_negative_zero_below_positive_zero() {
    let (pz, nz) = (0_u32, 0x8000_0000_u32);
    // Each case has a -0 and a +0 among its first two operands.
    let cases = [
        [nz, pz, nz, 0],
        [pz, nz, nz, 0],
        [nz, pz, pz, 0],
        [pz, nz, pz, 0],
    ];
    let ops = [
        ("v_max_f32", 0x110, pz),
        ("v_min_f32", 0x10F, nz),
        ("v_max3_f32", 0x154, pz),
        ("v_min3_f32", 0x151, nz),
    ];
    for (name, op, want) in ops {
        let shader = build_valu_cs(&vop3(op, 0, vgpr(1), vgpr(2), vgpr(3)));
        run_cases(&shader, &cases, |backend, _, inp, got| {
            assert_eq!(got, want, "{backend}: {name} {inp:x?}");
        });
    }
}

#[test]
fn v_med3_f16_matches_rdna2_with_nan() {
    // v_med3_f16 v0, v1, v2, v3 (VOP3 opcode 0x357)
    let shader = build_valu_cs(&vop3(0x357, 0, vgpr(1), vgpr(2), vgpr(3)));
    let h0 = 0x0000u32; // 0.0
    let h1 = 0x3C00u32; // 1.0
    let h2 = 0x4000u32; // 2.0
    let h3 = 0x4200u32; // 3.0
    let h5 = 0x4500u32; // 5.0
    let hn2 = 0xC000u32; // -2.0
    let nan = 0x7E00u32;

    let cases: &[([u32; 4], Option<u16>)] = &[
        ([h1, h2, h3, 0], Some(h2 as u16)),
        ([hn2, h0, h3, 0], Some(h0 as u16)),
        ([h1, h2, nan, 0], Some(h1 as u16)),
        ([nan, h1, h0, 0], Some(h0 as u16)),
        ([nan, h0, h1, 0], Some(h0 as u16)),
        ([h2, nan, h1, 0], Some(h1 as u16)),
        ([nan, h5, nan, 0], Some(h5 as u16)),
        ([nan, nan, nan, 0], None),
    ];

    let inputs: Vec<[u32; 4]> = cases.iter().map(|c| c.0).collect();
    run_cases(&shader, &inputs, |backend, i, inp, got| {
        let expect = cases[i].1;
        let got_h = got as u16;
        match expect {
            Some(want) => assert_eq!(
                got_h, want,
                "{backend}: v_med3_f16({:#06x}, {:#06x}, {:#06x}) = {got_h:#06x}, want {want:#06x}",
                inp[0], inp[1], inp[2],
            ),
            None => assert!(
                is_f16_nan(got_h),
                "{backend}: v_med3_f16(NaN, NaN, NaN) = {got_h:#06x}, want NaN",
            ),
        }
    });
}

/// `v_cmp_lg_*` is "less or greater": ordered and not equal, so false when
/// either operand is a NaN (RDNA2 ISA; the same on gfx1036 hardware).
#[test]
fn v_cmp_lg_is_false_for_nan() {
    // v_cmp_lg_{f32,f16}_e64 s20, v1, v2 (VOPC 0x005 / 0x0cd in VOP3);
    // v_cndmask_b32_e64 v0, 0, 1, s20 (VOP3 0x101)
    let nan = f32::NAN.to_bits();
    let (inf, ninf) = (f32::INFINITY.to_bits(), f32::NEG_INFINITY.to_bits());
    let f = f32::to_bits;
    let f32_cases: &[([u32; 4], u32)] = &[
        ([f(1.0), f(2.0), 0, 0], 1),
        ([f(2.0), f(1.0), 0, 0], 1),
        ([f(1.0), f(1.0), 0, 0], 0),
        ([f(0.0), f(-0.0), 0, 0], 0),
        ([inf, ninf, 0, 0], 1),
        ([inf, inf, 0, 0], 0),
        ([nan, f(1.0), 0, 0], 0),
        ([f(1.0), nan, 0, 0], 0),
        ([nan, nan, 0, 0], 0),
        ([0x7F80_0001, f(0.0), 0, 0], 0), // a signalling NaN
    ];
    let f16_cases: &[([u32; 4], u32)] = &[
        ([0x3C00, 0x4000, 0, 0], 1),
        ([0x4000, 0x3C00, 0, 0], 1),
        ([0x3C00, 0x3C00, 0, 0], 0),
        ([0x0000, 0x8000, 0, 0], 0),
        ([0x7C00, 0xFC00, 0, 0], 1),
        ([0x7E00, 0x3C00, 0, 0], 0),
        ([0x3C00, 0x7E00, 0, 0], 0),
        ([0x7E00, 0x7E00, 0, 0], 0),
        ([0x7C01, 0x0000, 0, 0], 0),           // a signalling NaN
        ([0xABCD_3C00, 0x1234_7E00, 0, 0], 0), // only the low halves are read
    ];
    for (op, what, cases) in [
        (0x005u16, "v_cmp_lg_f32", f32_cases),
        (0x0cd, "v_cmp_lg_f16", f16_cases),
    ] {
        let mut body = vop3(op, 20, vgpr(1), vgpr(2), 0x80).to_vec();
        body.extend_from_slice(&vop3(0x101, 0, 0x80, 0x81, sgpr(20)));
        let shader = build_valu_cs(&body);
        let inputs: Vec<[u32; 4]> = cases.iter().map(|c| c.0).collect();
        run_cases(&shader, &inputs, |backend, i, inp, got| {
            let want = cases[i].1;
            assert_eq!(
                got, want,
                "{backend}: {what}({:#010x}, {:#010x}) = {got}, want {want}",
                inp[0], inp[1],
            );
        });
    }
}

#[test]
fn v_fma_legacy_f32_follows_the_d3d9_zero_rule() {
    // v_fma_legacy_f32 v0, v1, v2, v3 (VOP3 opcode 0x140 on RDNA2)
    let shader = build_valu_cs(&vop3(0x140, 0, vgpr(1), vgpr(2), vgpr(3)));
    let f = f32::to_bits;
    let cases: &[([u32; 4], u32)] = &[
        ([f(2.0), f(3.0), f(1.0), 0], f(7.0)),
        // A zero factor gives a zero product, whatever the other one is.
        ([0, f(f32::INFINITY), f(1.5), 0], f(1.5)),
        ([f(f32::NAN), 0, f(-2.0), 0], f(-2.0)),
    ];
    let inputs: Vec<[u32; 4]> = cases.iter().map(|c| c.0).collect();
    run_cases(&shader, &inputs, |backend, i, _, got| {
        let want = cases[i].1;
        assert_eq!(got, want, "{backend}: case {i}: {got:#x}, want {want:#x}");
    });
}

#[test]
fn v_fract_f32_never_returns_one() {
    // v_fract_f32 v0, v1 (VOP1 opcode 0x020)
    let shader = build_valu_cs(&[vop1(0x020, 0, vgpr(1))]);
    let f = f32::to_bits;
    let below_one = 0x3F7F_FFFFu32;

    let cases: &[(u32, Option<u32>)] = &[
        // Tiny negatives: must clamp strictly below 1.0 (not 0x3f800000).
        (f(-1e-10), Some(below_one)),
        (f(-1e-8), Some(below_one)),
        (f(-2.0f32.powi(-25)), Some(below_one)),
        (f(-2.0f32.powi(-24)), Some(below_one)),
        // Normal values.
        (f(0.0), Some(f(0.0))),
        (f(-0.0), Some(f(0.0))),
        (f(0.25), Some(f(0.25))),
        (f(3.75), Some(f(0.75))),
        (f(-0.25), Some(f(0.75))),
        (f(-3.25), Some(f(0.75))),
        (f(1.0), Some(f(0.0))),
        (f(-1.0), Some(f(0.0))),
        // Non-finite inputs yield NaN.
        (f(f32::NAN), None),
        (f(f32::INFINITY), None),
        (f(f32::NEG_INFINITY), None),
    ];

    let inputs: Vec<[u32; 4]> = cases.iter().map(|&(x, _)| [x, 0, 0, 0]).collect();
    run_cases(&shader, &inputs, |backend, i, inp, got| {
        let expect = cases[i].1;
        match expect {
            Some(want) => assert_eq!(
                got,
                want,
                "{backend}: v_fract_f32({:?} / {:#010x}) = {got:#010x} ({:?}), want {want:#010x}",
                f32::from_bits(inp[0]),
                inp[0],
                f32::from_bits(got),
            ),
            None => assert!(
                f32::from_bits(got).is_nan(),
                "{backend}: v_fract_f32({:?}) = {got:#010x}, want NaN",
                f32::from_bits(inp[0]),
            ),
        }
    });
}

#[test]
fn v_fract_f16_never_returns_one() {
    // v_fract_f16 v0, v1 (VOP1 opcode 0x05f)
    let shader = build_valu_cs(&[vop1(0x05f, 0, vgpr(1))]);
    let below_one = 0x3BFFu16; // 1.0 - 2^-11

    assert_eq!(f16_to_f32(0x9400), -2.0f32.powi(-10));
    assert_eq!(f16_to_f32(0x9000), -2.0f32.powi(-11));
    assert_eq!(f16_to_f32(0x8C00), -2.0f32.powi(-12));

    let cases: &[(u16, Option<u16>)] = &[
        // Negatives in (-2^-11, 0): must clamp to 0x3BFF, never 0x3C00 (1.0).
        (0x8C00, Some(below_one)), // -2^-12
        (0x8400, Some(below_one)), // -2^-14
        (0x8001, Some(below_one)), // -2^-24 (smallest negative f16 subnormal)
        (0x9000, Some(below_one)), // -2^-11 (exact 1 - 2^-11 = 0x3BFF)
        (0x9400, Some(0x3BFE)),    // -2^-10 (exact 1 - 2^-10 = 0x3BFE)
        // Normal values.
        (0x0000, Some(0x0000)), // 0.0 -> 0.0
        (0x3400, Some(0x3400)), // 0.25 -> 0.25
        (0xB400, Some(0x3A00)), // -0.25 -> 0.75
        (0x4300, Some(0x3800)), // 3.5 -> 0.5
        (0x3C00, Some(0x0000)), // 1.0 -> 0.0
        // Non-finite inputs yield NaN.
        (0x7E00, None), // NaN
        (0x7C00, None), // +inf
        (0xFC00, None), // -inf
    ];

    let inputs: Vec<[u32; 4]> = cases
        .iter()
        .map(|&(x, _)| [u32::from(x), 0, 0, 0])
        .collect();
    run_cases(&shader, &inputs, |backend, i, inp, got| {
        let expect = cases[i].1;
        let got_h = got as u16;
        match expect {
            Some(want) => assert_eq!(
                got_h, want,
                "{backend}: v_fract_f16({:#06x}) = {got_h:#06x}, want {want:#06x}",
                inp[0],
            ),
            None => assert!(
                is_f16_nan(got_h),
                "{backend}: v_fract_f16({:#06x}) = {got_h:#06x}, want NaN",
                inp[0],
            ),
        }
    });
}

/// RDNA2's flt16_to_snorm16 / flt16_to_unorm16: clamp, scale by 32767 or
/// 65535, round the exact product to nearest even; NaN gives 0.
fn cvt_norm16_reference(bits: u16, signed: bool) -> u16 {
    let x = f16_to_f32(bits);
    if x.is_nan() {
        return 0;
    }
    let (lo, scale) = if signed {
        (-1.0, 32767.0)
    } else {
        (0.0, 65535.0)
    };
    // f64 holds the product of an f16 and a 16-bit scale exactly.
    let p = f64::from(x.clamp(lo, 1.0)) * scale;
    p.round_ties_even() as i32 as u16
}

#[test]
fn v_cvt_norm_i16_and_u16_f16_round_the_exact_product() {
    // Every 251st f16 pattern, plus the values where an f32 product would
    // round before the rounding to an integer, ties, and the edges.
    let mut bits: Vec<u16> = (0..=u16::MAX).step_by(251).collect();
    bits.extend([
        0x0000, 0x8000, 0x0001, 0x8001, 0x03FF, 0x0400, 0x3800, 0xB800, 0x3801, 0xB801, 0x37FF,
        0x3BFF, 0xBBFF, 0x3C00, 0xBC00, 0x3C01, 0x7BFF, 0x7C00, 0xFC00, 0x7E00, 0xFE00, 0x7C01,
        0x3A9B, 0xBA9B, 0x2E66, 0x1400,
    ]);
    for (op, signed, what) in [
        (0x063u16, true, "v_cvt_norm_i16_f16"),
        (0x064, false, "v_cvt_norm_u16_f16"),
    ] {
        // v_cvt_norm_*16_f16 v0, v1 (VOP1); v0's high half starts at 0.
        let shader = build_valu_cs(&[vop1(0x001, 0, 128), vop1(op, 0, vgpr(1))]);
        let inputs: Vec<[u32; 4]> = bits.iter().map(|&b| [u32::from(b), 0, 0, 0]).collect();
        run_cases(&shader, &inputs, |backend, i, _, got| {
            let want = cvt_norm16_reference(bits[i], signed);
            assert_eq!(
                got,
                u32::from(want),
                "{backend}: {what}({:#06x}) = {got:#010x}, want {want:#06x}",
                bits[i],
            );
        });
    }
}

#[test]
fn v_sin_and_v_cos_f32_reduce_large_turns() {
    let sin_shader = build_valu_cs(&[vop1(0x035, 0, vgpr(1))]);
    let cos_shader = build_valu_cs(&[vop1(0x036, 0, vgpr(1))]);
    let f = f32::to_bits;
    let half_sqrt2 = std::f32::consts::FRAC_1_SQRT_2;

    let sin_cases: &[(f32, Option<f32>)] = &[
        (0.0, Some(0.0)),
        (0.25, Some(1.0)),
        (255.0, Some(0.0)),
        (255.25, Some(1.0)),
        (255.5, Some(0.0)),
        (-255.25, Some(-1.0)),
        (10_000.125, Some(half_sqrt2)),
        (f32::NAN, None),
        (f32::INFINITY, None),
        (f32::NEG_INFINITY, None),
    ];
    let sin_inputs: Vec<[u32; 4]> = sin_cases.iter().map(|&(x, _)| [f(x), 0, 0, 0]).collect();
    run_cases(&sin_shader, &sin_inputs, |backend, i, inp, got| {
        let expect = sin_cases[i].1;
        let got_f = f32::from_bits(got);
        match expect {
            Some(want) => assert!(
                (got_f - want).abs() < 1e-5,
                "{backend}: v_sin_f32({}) = {got_f}, want {want} (err = {})",
                f32::from_bits(inp[0]),
                (got_f - want).abs(),
            ),
            None => assert!(
                got_f.is_nan(),
                "{backend}: v_sin_f32({}) = {got_f}, want NaN",
                f32::from_bits(inp[0]),
            ),
        }
    });

    let cos_cases: &[(f32, Option<f32>)] = &[
        (0.0, Some(1.0)),
        (0.25, Some(0.0)),
        (255.0, Some(1.0)),
        (255.25, Some(0.0)),
        (255.5, Some(-1.0)),
        (-255.5, Some(-1.0)),
        (10_000.125, Some(half_sqrt2)),
        (f32::NAN, None),
        (f32::INFINITY, None),
        (f32::NEG_INFINITY, None),
    ];
    let cos_inputs: Vec<[u32; 4]> = cos_cases.iter().map(|&(x, _)| [f(x), 0, 0, 0]).collect();
    run_cases(&cos_shader, &cos_inputs, |backend, i, inp, got| {
        let expect = cos_cases[i].1;
        let got_f = f32::from_bits(got);
        match expect {
            Some(want) => assert!(
                (got_f - want).abs() < 1e-5,
                "{backend}: v_cos_f32({}) = {got_f}, want {want} (err = {})",
                f32::from_bits(inp[0]),
                (got_f - want).abs(),
            ),
            None => assert!(
                got_f.is_nan(),
                "{backend}: v_cos_f32({}) = {got_f}, want NaN",
                f32::from_bits(inp[0]),
            ),
        }
    });

    // Main's `trig_turns` preserves sin(-0.0) = -0.0 and produces exact +0.0 at
    // half-turns (sin) and quarter-turns (cos).
    let exact_sin: &[(u32, u32)] = &[
        (0x8000_0000, 0x8000_0000), // sin(-0.0) = -0.0
        (f(255.0), 0),              // sin(255.0) = +0.0
        (f(255.5), 0),              // sin(255.5) = +0.0
    ];
    let exact_sin_in: Vec<[u32; 4]> = exact_sin.iter().map(|&(x, _)| [x, 0, 0, 0]).collect();
    run_cases(&sin_shader, &exact_sin_in, |backend, i, inp, got| {
        let want = exact_sin[i].1;
        assert_eq!(
            got, want,
            "{backend}: v_sin_f32({:#010x}) = {got:#010x}, want {want:#010x}",
            inp[0],
        );
    });
    let exact_cos: &[(u32, u32)] = &[
        (f(0.25), 0),   // cos(0.25) = +0.0
        (f(255.25), 0), // cos(255.25) = +0.0
    ];
    let exact_cos_in: Vec<[u32; 4]> = exact_cos.iter().map(|&(x, _)| [x, 0, 0, 0]).collect();
    run_cases(&cos_shader, &exact_cos_in, |backend, i, inp, got| {
        let want = exact_cos[i].1;
        assert_eq!(
            got, want,
            "{backend}: v_cos_f32({:#010x}) = {got:#010x}, want {want:#010x}",
            inp[0],
        );
    });
}

#[test]
fn v_cvt_u16_and_i16_f16_saturate() {
    // v_mov_b32 v0, 0; v_cvt_u16_f16 / v_cvt_i16_f16 v0, v1 (VOP1 0x052 / 0x053).
    let cases: &[(u16, u16, u16)] = &[
        // (f16 input, u16 result, i16 result)
        (0x0000, 0, 0),          // 0
        (0x8000, 0, 0),          // -0
        (0x3E00, 1, 1),          // 1.5 -> 1 (toward zero)
        (0xBE00, 0, 0xFFFF),     // -1.5 -> 0 / -1
        (0x57D0, 125, 125),      // 125.0
        (0x7800, 32768, 32767),  // 32768 saturates i16
        (0x77FF, 32752, 32752),  // 32752, the largest f16 below 32768
        (0xF800, 0, 0x8000),     // -32768 fits i16
        (0xF801, 0, 0x8000),     // -32800 saturates i16
        (0x7BFF, 65504, 32767),  // max finite f16
        (0xFBFF, 0, 0x8000),     // -65504
        (0x7C00, 0xFFFF, 32767), // +inf
        (0xFC00, 0, 0x8000),     // -inf
        (0x7E00, 0, 0),          // NaN
        (0xFE00, 0, 0),          // -NaN
        (0x0001, 0, 0),          // smallest subnormal
    ];
    for (op, what, pick) in [
        (0x052u16, "v_cvt_u16_f16", 0usize),
        (0x053, "v_cvt_i16_f16", 1),
    ] {
        let shader = build_valu_cs(&[vop1(0x001, 0, 128), vop1(op, 0, vgpr(1))]);
        let inputs: Vec<[u32; 4]> = cases.iter().map(|c| [u32::from(c.0), 0, 0, 0]).collect();
        run_cases(&shader, &inputs, |backend, i, _, got| {
            let want = if pick == 0 { cases[i].1 } else { cases[i].2 };
            assert_eq!(
                got,
                u32::from(want),
                "{backend}: {what}({:#06x}) = {got:#010x}, want {want:#06x}",
                cases[i].0,
            );
        });
    }
}

#[test]
fn v_sin_and_v_cos_f16_reduce_large_turns() {
    let sin_shader = build_valu_cs(&[vop1(0x060, 0, vgpr(1))]);
    let cos_shader = build_valu_cs(&[vop1(0x061, 0, vgpr(1))]);

    // Sanity-check the f16 bit encodings of large turns.
    assert_eq!(f16_to_f32(0x5BF8), 255.0);
    assert_eq!(f16_to_f32(0x5BFA), 255.25);
    assert_eq!(f16_to_f32(0x5BFC), 255.5);
    assert_eq!(f16_to_f32(0x7BFF), 65504.0);

    let sin_cases: &[(u16, Option<f32>)] = &[
        (0x5BF8, Some(0.0)),  // 255.0
        (0x5BFA, Some(1.0)),  // 255.25
        (0x5BFC, Some(0.0)),  // 255.5
        (0xDBFA, Some(-1.0)), // -255.25
        (0x7BFF, Some(0.0)),  // 65504.0 (largest finite f16, exact integer)
        (0x7E00, None),       // NaN
        (0x7C00, None),       // +inf
        (0xFC00, None),       // -inf
    ];
    let sin_inputs: Vec<[u32; 4]> = sin_cases
        .iter()
        .map(|&(x, _)| [u32::from(x), 0, 0, 0])
        .collect();
    run_cases(&sin_shader, &sin_inputs, |backend, i, inp, got| {
        let expect = sin_cases[i].1;
        let got_h = got as u16;
        match expect {
            Some(want) => {
                let got_f = f16_to_f32(got_h);
                assert!(
                    (got_f - want).abs() < 1e-3,
                    "{backend}: v_sin_f16({:#06x} = {}) = {got_f} ({got_h:#06x}), want {want}",
                    inp[0],
                    f16_to_f32(inp[0] as u16),
                );
            }
            None => assert!(
                is_f16_nan(got_h),
                "{backend}: v_sin_f16({:#06x}) = {got_h:#06x}, want NaN",
                inp[0],
            ),
        }
    });

    let cos_cases: &[(u16, Option<f32>)] = &[
        (0x5BF8, Some(1.0)),  // 255.0
        (0x5BFA, Some(0.0)),  // 255.25
        (0x5BFC, Some(-1.0)), // 255.5
        (0x7BFF, Some(1.0)),  // 65504.0 (largest finite f16, exact integer)
        (0x7E00, None),       // NaN
        (0x7C00, None),       // +inf
        (0xFC00, None),       // -inf
    ];
    let cos_inputs: Vec<[u32; 4]> = cos_cases
        .iter()
        .map(|&(x, _)| [u32::from(x), 0, 0, 0])
        .collect();
    run_cases(&cos_shader, &cos_inputs, |backend, i, inp, got| {
        let expect = cos_cases[i].1;
        let got_h = got as u16;
        match expect {
            Some(want) => {
                let got_f = f16_to_f32(got_h);
                assert!(
                    (got_f - want).abs() < 1e-3,
                    "{backend}: v_cos_f16({:#06x} = {}) = {got_f} ({got_h:#06x}), want {want}",
                    inp[0],
                    f16_to_f32(inp[0] as u16),
                );
            }
            None => assert!(
                is_f16_nan(got_h),
                "{backend}: v_cos_f16({:#06x}) = {got_h:#06x}, want NaN",
                inp[0],
            ),
        }
    });
}

#[test]
fn bfe_signed_and_unsigned_match_rdna2() {
    // v_bfe_i32 v0, v1, v2, v3 (VOP3 opcode 0x149)
    let v_signed = build_valu_cs(&vop3(0x149, 0, vgpr(1), vgpr(2), vgpr(3)));
    // v_bfe_u32 v0, v1, v2, v3 (VOP3 opcode 0x148)
    let v_unsigned = build_valu_cs(&vop3(0x148, 0, vgpr(1), vgpr(2), vgpr(3)));
    // s_bfe_i32 s6, s0, s1 (SOP2 opcode 0x28: s1 = off[4:0] | width[22:16]); v_mov_b32 v0, s6
    let s_signed = build_valu_cs(&[sop2(0x28, 6, 0, 1), vop1(0x001, 0, sgpr(6))]);

    let signed_cases: &[([u32; 4], u32)] = &[
        // Field crosses bit 31: extends from bit 31.
        ([0xd2f9_0159, 28, 16, 0], 0xffff_fffd),
        ([0x8000_0000, 31, 4, 0], 0xffff_ffff),
        ([0x7000_0000, 28, 8, 0], 0x0000_0007),
        // Normal fields inside 32 bits.
        ([0x0000_0080, 4, 4, 0], 0xffff_fff8),
        ([0x0000_0070, 4, 4, 0], 0x0000_0007),
        // Edge widths: 0 yields 0; v_bfe masks width to 5 bits (32 -> 0),
        // while 31 extracts all remaining bits from offset 4.
        ([0xffff_ffff, 4, 0, 0], 0),
        ([0x8000_0000, 4, 31, 0], 0xf800_0000),
    ];
    let v_in: Vec<[u32; 4]> = signed_cases.iter().map(|c| c.0).collect();
    run_cases(&v_signed, &v_in, |backend, i, inp, got| {
        let want = signed_cases[i].1;
        assert_eq!(
            got, want,
            "{backend}: v_bfe_i32({:#010x}, {}, {}) = {got:#010x}, want {want:#010x}",
            inp[0], inp[1], inp[2],
        );
    });
    // v_bfe_i32 width is 5-bit (src2[4:0]), so width=32 wraps to 0.
    run_cases(
        &v_signed,
        &[[0x8000_0000, 4, 32, 0]],
        |backend, _, _, got| {
            assert_eq!(got, 0, "{backend}: v_bfe_i32 width 32 should mask to 0");
        },
    );

    // s_bfe_i32 width is 7-bit (src1[22:16]), so width=32 keeps all 32 bits.
    let mut s_cases = signed_cases.to_vec();
    s_cases.push(([0x8000_0000, 4, 32, 0], 0xf800_0000));
    let s_in: Vec<[u32; 4]> = s_cases
        .iter()
        .map(|&(inp, _)| [inp[0], (inp[1] & 31) | ((inp[2] & 0x7F) << 16), 0, 0])
        .collect();
    run_cases(&s_signed, &s_in, |backend, i, inp, got| {
        let want = s_cases[i].1;
        assert_eq!(
            got, want,
            "{backend}: s_bfe_i32({:#010x}, {:#010x}) = {got:#010x}, want {want:#010x}",
            inp[0], inp[1],
        );
    });

    let unsigned_cases: &[([u32; 4], u32)] = &[
        ([0xd2f9_0159, 28, 16, 0], 0x0000_000d),
        ([0x8000_0000, 31, 4, 0], 0x0000_0001),
        ([0xffff_ffff, 4, 0, 0], 0),
        ([0x8000_0000, 4, 31, 0], 0x0800_0000),
    ];
    let u_in: Vec<[u32; 4]> = unsigned_cases.iter().map(|c| c.0).collect();
    run_cases(&v_unsigned, &u_in, |backend, i, inp, got| {
        let want = unsigned_cases[i].1;
        assert_eq!(
            got, want,
            "{backend}: v_bfe_u32({:#010x}, {}, {}) = {got:#010x}, want {want:#010x}",
            inp[0], inp[1], inp[2],
        );
    });
}

#[test]
fn v_cmp_class_f16_matches_rdna2() {
    // v_cmp_class_f16_e64 s6, v1, v2 (VOP3 opcode 0x08f); v_cndmask_b32_e64 v0, 0, 1, s6
    let cmp_lo = build_valu_cs(&[
        vop3(0x08f, 6, vgpr(1), vgpr(2), 0)[0],
        vop3(0x08f, 6, vgpr(1), vgpr(2), 0)[1],
        vop3(0x101, 0, 0x80, 0x81, sgpr(6))[0],
        vop3(0x101, 0, 0x80, 0x81, sgpr(6))[1],
    ]);
    // Same with op_sel:[1,0] (high half of v1) + neg on src0 (bit 29 of w1).
    let [mut w0, mut w1] = vop3(0x08f, 6, vgpr(1), vgpr(2), 0);
    w0 |= 1 << 11; // opsel[0] = 1 (high half)
    w1 |= 1 << 29; // neg[0] = 1
    let cmp_hi_neg = build_valu_cs(&[
        w0,
        w1,
        vop3(0x101, 0, 0x80, 0x81, sgpr(6))[0],
        vop3(0x101, 0, 0x80, 0x81, sgpr(6))[1],
    ]);

    // All 10 IEEE-754 binary16 classes: bit k corresponds to class k.
    let classes: &[(u32, u32)] = &[
        (0x7C01, 0), // sNaN (+sNaN, quiet bit 0x0200 clear)
        (0xFDFF, 0), // -sNaN (sign ignored for NaNs)
        (0x7E00, 1), // +qNaN
        (0xFE01, 1), // -qNaN
        (0xFC00, 2), // -inf
        (0xBC00, 3), // -normal (-1.0)
        (0x8001, 4), // -denormal
        (0x8000, 5), // -0.0
        (0x0000, 6), // +0.0
        (0x0001, 7), // +denormal
        (0x3C00, 8), // +normal (+1.0)
        (0x7C00, 9), // +inf
    ];
    let mut cases = Vec::new();
    for &(bits, cls) in classes {
        // Exact class bit -> true; complement of class bit across all 10 bits -> false.
        cases.push(([bits, 1u32 << cls, 0, 0], 1u32));
        cases.push(([bits, 0x3FF & !(1u32 << cls), 0, 0], 0u32));
    }
    let inputs: Vec<[u32; 4]> = cases.iter().map(|c| c.0).collect();
    run_cases(&cmp_lo, &inputs, |backend, i, inp, got| {
        let want = cases[i].1;
        assert_eq!(
            got, want,
            "{backend}: v_cmp_class_f16({:#06x}, {:#05x}) = {got}, want {want}",
            inp[0], inp[1],
        );
    });

    // High half with neg: high half 0x3C00 (+1.0) negated -> -normal (class 3);
    // high half 0x7C01 (+sNaN) negated -> still sNaN (class 0, not quieted).
    let hi_cases: &[([u32; 4], u32)] = &[
        ([0x3C00_0000, 1 << 3, 0, 0], 1),
        ([0x3C00_0000, 1 << 8, 0, 0], 0),
        ([0x7C01_FFFF, 1 << 0, 0, 0], 1),
        ([0x7C01_FFFF, 1 << 1, 0, 0], 0),
    ];
    let hi_in: Vec<[u32; 4]> = hi_cases.iter().map(|c| c.0).collect();
    run_cases(&cmp_hi_neg, &hi_in, |backend, i, inp, got| {
        let want = hi_cases[i].1;
        assert_eq!(
            got, want,
            "{backend}: v_cmp_class_f16(-hi({:#010x}), {:#05x}) = {got}, want {want}",
            inp[0], inp[1],
        );
    });
}

#[test]
fn v_cmp_f32_nan_matches_rdna2() {
    let nan = f32::NAN.to_bits();
    let one = 1.0f32.to_bits();
    let two = 2.0f32.to_bits();
    let sel = vop3(0x101, 0, 0x80, 0x81, sgpr(6));

    // Self-comparisons (x, x): ordered predicates must be false on NaN and true
    // on finite values; unordered predicates must be true on NaN and false on
    // finite values (Intel Xe folds plain `fcmp` self-compares to constants).
    for (name, opc, want_nan, want_one) in [
        ("v_cmp_eq_f32", 0x002u16, 0u32, 1u32),
        ("v_cmp_le_f32", 0x003u16, 0u32, 1u32),
        ("v_cmp_ge_f32", 0x006u16, 0u32, 1u32),
        ("v_cmp_o_f32", 0x007u16, 0u32, 1u32),
        ("v_cmp_u_f32", 0x008u16, 1u32, 0u32),
        ("v_cmp_nge_f32", 0x009u16, 1u32, 0u32),
        ("v_cmp_nle_f32", 0x00cu16, 1u32, 0u32),
        ("v_cmp_neq_f32", 0x00du16, 1u32, 0u32),
    ] {
        let cmp = vop3(opc, 6, vgpr(1), vgpr(1), 0);
        let sh = build_valu_cs(&[cmp[0], cmp[1], sel[0], sel[1]]);
        run_cases(&sh, &[[nan, 0, 0, 0], [one, 0, 0, 0]], |backend, i, _, got| {
            let want = if i == 0 { want_nan } else { want_one };
            assert_eq!(
                got, want,
                "{backend}: {name}(same reg, case {i}) = {got}, want {want}",
            );
        });
    }

    // Distinct-register ordered/unordered comparisons: Intel Xe also folds
    // `fcmp ord`/`fcmp uno` on distinct registers when either operand is NaN.
    for (name, opc, want_nan, want_finite) in [
        ("v_cmp_o_f32", 0x007u16, 0u32, 1u32),
        ("v_cmp_u_f32", 0x008u16, 1u32, 0u32),
    ] {
        let cmp = vop3(opc, 6, vgpr(1), vgpr(2), 0);
        let sh = build_valu_cs(&[cmp[0], cmp[1], sel[0], sel[1]]);
        let inputs = [
            [nan, one, 0, 0],
            [one, nan, 0, 0],
            [nan, nan, 0, 0],
            [one, two, 0, 0],
        ];
        run_cases(&sh, &inputs, |backend, i, _, got| {
            let want = if i == 3 { want_finite } else { want_nan };
            assert_eq!(
                got, want,
                "{backend}: {name}(diff reg, case {i}) = {got}, want {want}",
            );
        });
    }
}

/// VOP3P `op vdst, src0, src1, src2` with the default `op_sel_hi` (all
/// halves high) and no other modifiers.
const fn vop3p(op: u16, vdst: u8, src0: u16, src1: u16, src2: u16) -> [u32; 2] {
    [
        0x33 << 26 | (op as u32 & 0x7F) << 16 | 1 << 14 | vdst as u32,
        (src0 as u32 & 0x1FF) | (src1 as u32 & 0x1FF) << 9 | (src2 as u32 & 0x1FF) << 18 | 3 << 27,
    ]
}

/// Inputs that reach the edges of 8-, 16- and 24-bit fields.
const EDGES: [u32; 10] = [
    0,
    1,
    0x7F,
    0x80,
    0xFF,
    0x7FFF,
    0x8000,
    0x00FF_FFFF,
    0x8000_0000,
    0xFFFF_FFFF,
];

/// Pairs and triples of [`EDGES`] and a few mixed words.
fn edge_cases() -> Vec<[u32; 4]> {
    let mut out = Vec::new();
    for &a in &EDGES {
        for &b in &EDGES {
            out.push([a, b, 0x1234_5678, 0]);
        }
    }
    out.extend([
        [0x0123_4567, 0x89AB_CDEF, 0xDEAD_BEEF, 0],
        [0x8081_7F7E, 0xFF01_80FE, 0x7FFF_FFFF, 0],
        [0xFEDC_BA98, 0x7654_3210, 0x8000_0000, 0],
    ]);
    out
}

#[test]
fn v_mul_hi_24_bit_products() {
    // VOP2 v_mul_hi_u32_u24 (0x0c) and v_mul_hi_i32_i24 (0x0a) as VOP3.
    for (op, signed) in [(0x10c, false), (0x10a, true)] {
        let shader = build_valu_cs(&vop3(op, 0, vgpr(1), vgpr(2), 0));
        let cases = edge_cases();
        run_cases(&shader, &cases, |backend, _, inp, got| {
            let ext = |x: u32| {
                if signed {
                    i64::from((x << 8).cast_signed() >> 8)
                } else {
                    i64::from(x & 0x00FF_FFFF)
                }
            };
            let want = ((ext(inp[0]) * ext(inp[1])) >> 32) as u32;
            assert_eq!(
                got, want,
                "{backend}: op {op:#x}({:#x}, {:#x})",
                inp[0], inp[1]
            );
        });
    }
}

#[test]
fn v_cvt_pk_16_saturates_each_half() {
    // VOP3 v_cvt_pk_u16_u32 (0x36a) and v_cvt_pk_i16_i32 (0x36b).
    for (op, signed) in [(0x36a, false), (0x36b, true)] {
        let shader = build_valu_cs(&vop3(op, 0, vgpr(1), vgpr(2), 0));
        let cases = edge_cases();
        run_cases(&shader, &cases, |backend, _, inp, got| {
            let sat = |x: u32| {
                if signed {
                    (x.cast_signed().clamp(-32768, 32767) as u32) & 0xFFFF
                } else {
                    x.min(0xFFFF)
                }
            };
            let want = sat(inp[0]) | sat(inp[1]) << 16;
            assert_eq!(
                got, want,
                "{backend}: op {op:#x}({:#x}, {:#x})",
                inp[0], inp[1]
            );
        });
    }
}

#[test]
fn v_dot_products_of_packed_fields() {
    // VOP3P dot products: (opcode, field bits, signed).
    for (op, bits, signed) in [
        (0x14, 16, true),
        (0x15, 16, false),
        (0x16, 8, true),
        (0x17, 8, false),
        (0x19, 4, false),
    ] {
        let shader = build_valu_cs(&vop3p(op, 0, vgpr(1), vgpr(2), vgpr(3)));
        let cases = edge_cases();
        run_cases(&shader, &cases, |backend, _, inp, got| {
            let field = |x: u32, i: u32| {
                let f = (x >> (bits * i)) & ((1 << bits) - 1);
                if signed && f >> (bits - 1) != 0 {
                    i64::from(f) - (1 << bits)
                } else {
                    i64::from(f)
                }
            };
            let mut sum = if signed {
                i64::from(inp[2].cast_signed())
            } else {
                i64::from(inp[2])
            };
            for i in 0..32 / bits {
                sum += field(inp[0], i) * field(inp[1], i);
            }
            let want = sum as u32;
            assert_eq!(
                got, want,
                "{backend}: dot op {op:#x}({:#x}, {:#x}, {:#x})",
                inp[0], inp[1], inp[2]
            );
        });
    }
}

#[test]
fn v_ldexp_f32_scales_by_powers_of_two() {
    // VOP3 v_ldexp_f32 (0x362): v1 * 2^v2.
    let shader = build_valu_cs(&vop3(0x362, 0, vgpr(1), vgpr(2), 0));
    let f = f32::to_bits;
    let e = |x: i32| x.cast_unsigned();
    let cases: &[([u32; 4], f32)] = &[
        ([f(1.0), e(0), 0, 0], 1.0),
        ([f(1.5), e(3), 0, 0], 12.0),
        ([f(-3.0), e(-2), 0, 0], -0.75),
        ([f(1.0), e(127), 0, 0], 2f32.powi(127)),
        ([f(1.0), e(-126), 0, 0], f32::MIN_POSITIVE),
        ([f(1.0), e(128), 0, 0], f32::INFINITY),
        ([f(-1.0), e(1000), 0, 0], f32::NEG_INFINITY),
        ([f(1.0), e(i32::MIN), 0, 0], 0.0),
        // Exponents past one factor's range still scale exactly.
        ([f(2f32.powi(-100)), e(200), 0, 0], 2f32.powi(100)),
        ([f(2f32.powi(100)), e(-200), 0, 0], 2f32.powi(-100)),
        (
            [f(f32::MAX), e(-253), 0, 0],
            f32::MAX / 2f32.powi(127) / 2f32.powi(126),
        ),
        ([f(0.0), e(50), 0, 0], 0.0),
        ([f(f32::INFINITY), e(-50), 0, 0], f32::INFINITY),
    ];
    let inputs: Vec<[u32; 4]> = cases.iter().map(|c| c.0).collect();
    run_cases(&shader, &inputs, |backend, i, inp, got| {
        let want = cases[i].1;
        assert_eq!(
            f32::from_bits(got),
            want,
            "{backend}: ldexp({}, {})",
            f32::from_bits(inp[0]),
            inp[1].cast_signed()
        );
    });
}

/// SCC after the SOP2 `op s12, s0, s1`, read back into `v0` through
/// `s_cselect_b32 s13, 1, 0`.
fn scc_after(op: u8) -> HostShader {
    build_valu_cs(&[
        sop2(op, 12, 0, 1),
        sop2(0x0A, 13, 129, 128), // s_cselect_b32 s13, 1, 0
        vop1(0x001, 0, sgpr(13)), // v_mov_b32 v0, s13
    ])
}

/// Checks the SCC that `op` leaves for each `(s0, s1, scc)`.
fn check_scc(name: &str, op: u8, cases: &[(u32, u32, u32)]) {
    let inputs: Vec<[u32; 4]> = cases.iter().map(|&(a, b, _)| [a, b, 0, 0]).collect();
    run_cases(&scc_after(op), &inputs, |backend, i, inp, got| {
        let want = cases[i].2;
        assert_eq!(
            got, want,
            "{backend}: SCC after {name}({:#010x}, {:#010x}) = {got}, want {want}",
            inp[0], inp[1],
        );
    });
}

#[test]
fn s_min_and_s_max_set_scc_when_src0_is_chosen() {
    let m1 = (-1i32) as u32;
    check_scc("s_min_i32", 0x06, &[(m1, 2, 1), (2, m1, 0), (4, 4, 0)]);
    check_scc("s_min_u32", 0x07, &[(3, 5, 1), (5, 3, 0), (4, 4, 0)]);
    check_scc("s_max_i32", 0x08, &[(2, m1, 1), (m1, 2, 0), (4, 4, 0)]);
    check_scc("s_max_u32", 0x09, &[(5, 3, 1), (3, 5, 0), (4, 4, 0)]);
}

#[test]
fn s_lshl_add_sets_scc_on_unsigned_overflow() {
    // (s0 << n) + s1 past 32 bits, through the shift or through the add.
    check_scc(
        "s_lshl1_add_u32",
        0x2E,
        &[
            (1, 2, 0),
            (1, 0xFFFF_FFFD, 0),
            (1, 0xFFFF_FFFE, 1),
            (0x8000_0000, 0, 1),
        ],
    );
    check_scc(
        "s_lshl4_add_u32",
        0x31,
        &[
            (0x0FFF_FFFF, 0xF, 0),
            (0x0FFF_FFFF, 0x10, 1),
            (0x1000_0000, 0, 1),
        ],
    );
}

/// MTBUF `op vdata, off, s[srsrc:srsrc+3], 0 format:format` (at byte 0).
const fn mtbuf(op: u8, vdata: u8, srsrc: u8, format: u8) -> [u32; 2] {
    [
        0x3A << 26 | (format as u32 & 0x7F) << 19 | (op as u32 & 7) << 16,
        0x80 << 24
            | (op as u32 >> 3 & 1) << 21
            | (srsrc as u32 >> 2 & 0x1F) << 16
            | (vdata as u32) << 8,
    ]
}

/// Stores `v1` to the buffer, then reads it back into `v0` with the tbuffer
/// load `op` in buffer format `format`.
fn tbuffer_load_of_v1(op: u8, format: u8) -> HostShader {
    let mut body = buffer_store_dword(1, 8, 0).to_vec();
    body.extend(mtbuf(op, 0, 8, format));
    build_valu_cs(&body)
}

#[test]
fn tbuffer_d16_loads_pack_two_channels_per_dword() {
    // tbuffer_load_format_d16_x and tbuffer_load_format_d16_xy.
    let (x, xy) = (8, 9);
    // (opcode, format, dword stored, dword loaded)
    let cases: &[(u8, u8, u32, u32)] = &[
        // 16_16_FLOAT: the f16 pair (1.0, 2.0) as it was stored.
        (xy, 29, 0x4000_3C00, 0x4000_3C00),
        // 16_16_UINT: the 16-bit integers as they were stored.
        (xy, 27, 0xBEEF_1234, 0xBEEF_1234),
        // 16_16_UNORM: 0.0 and 1.0, as f16.
        (xy, 23, 0xFFFF_0000, 0x3C00_0000),
        // 16_FLOAT, one channel: the high half is 0.
        (x, 13, 0x4000_3C00, 0x0000_3C00),
    ];
    for &(op, format, stored, want) in cases {
        let shader = tbuffer_load_of_v1(op, format);
        run_cases(&shader, &[[stored, 0, 0, 0]], |backend, _, _, got| {
            assert_eq!(
                got, want,
                "{backend}: d16 opcode {op} format {format} of {stored:#010x} = {got:#010x}, want {want:#010x}",
            );
        });
    }
}

/// A wave64 compute program of 64 lanes (thread id in v0) with `body` between
/// the descriptor load and the store of v0 at byte 4 * lane.
fn build_lanes_cs(body: &[u32]) -> Result<HostShader, String> {
    let mut words = Vec::new();
    words.push(s_mov_b32(5, 0x80));
    words.extend_from_slice(&s_load_dwordx4(8, 4));
    words.extend_from_slice(body);
    // v_lshlrev_b32 v9, 2, v10 (v10 = saved thread id): byte offset per lane
    words.push(0x3412_1482);
    // buffer_store_dword v0, v9, s[8:11], 0 offen
    words.extend_from_slice(&[
        0x38 << 26 | 0x1C << 18 | 1 << 12,
        0x80 << 24 | (8 >> 2) << 16 | 9,
    ]);
    words.push(S_ENDPGM);
    let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let native = NativeProgram {
        stage: HwStage::Cs,
        wave64: true,
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
            rsrc2: 5 << 1,
            threads: [64, 1, 1],
        }),
        code,
    };
    let p = ir::build(&native.code, native.wave64, native.vgprs).map_err(|e| format!("{e:?}"))?;
    let res = resource::track(&p, &native.layout, native.stage).map_err(|e| format!("{e:?}"))?;
    let io = io::raise(&p, &native).map_err(|e| format!("{e:?}"))?;
    let st = structure::structure(&p).map_err(|e| format!("{e:?}"))?;
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
    .map_err(|e| format!("{e:?}"))?;
    Ok(HostShader {
        stage: HwStage::Cs,
        dxil: lowered.dxil,
        user_data: lowered.user_data,
        samplers: lowered.samplers,
        static_samplers: Vec::new(),
        root_cbvs: lowered.root_cbvs,
        root_srvs: lowered.root_srvs,
        root_uavs: lowered.root_uavs,
        stride_table: lowered.stride_table,
    })
}

/// A divergent loop's exit mask (lanes that left in earlier iterations, so
/// not running where it is last written) read as a number after the loop.
#[test]
fn loop_exit_mask_counted_after_the_loop() {
    let body = [
        0x7E14_0300, // v_mov_b32 v10, v0 (thread id kept)
        0x4A02_0081, // v_add_nc_u32 v1, 1, v0  (lane i loops i + 1 times)
        0xBE82_0480, // s_mov_b64 s[2:3], 0
        0x4E02_0281, // loop: v_subrev_nc_u32 v1, 1, v1 (v1 - 1)
        0x7D84_0280, // v_cmp_eq_u32 vcc, 0, v1
        0x8882_026A, // s_or_b64 s[2:3], vcc, s[2:3]
        0x8AFE_027E, // s_andn2_b64 exec, exec, s[2:3]
        0xBF89_FFFB, // s_cbranch_execnz loop
        0x88FE_027E, // s_or_b64 exec, exec, s[2:3]
        0xBE84_1002, // s_bcnt1_i32_b64 s4, s[2:3]
        0x7E00_0204, // v_mov_b32 v0, s4
    ];
    let shader = build_lanes_cs(&body).unwrap_or_else(|e| panic!("translation: {e}"));
    let Ok(host) = Host::new(false) else {
        eprintln!("no SM 6.6 hardware device; skipped");
        return;
    };
    let (root, pso) = host.compute_pipeline(&shader).expect("pipeline");
    let ud = vec![0u32; shader.user_data_dwords().max(1) as usize];
    let out = host
        .dispatch(
            &root,
            &pso,
            &Dispatch {
                roots: vec![(0, Root::Constants(ud))],
                views: &[View::RawUav(64)],
                samplers: &[],
                groups: [1, 1, 1],
            },
        )
        .expect("dispatch");
    let got: Vec<u32> = out[0][..64].to_vec();
    assert!(
        got.iter().all(|&x| x == 64),
        "lanes counted as exited: {got:?}"
    );
}

/// A scalar counter a divergent loop adds to in every iteration, read after
/// the loop: the console runs the loop until its last lane leaves, so every
/// lane reads the count of the wave's iterations.
#[test]
fn scalar_counted_in_a_divergent_loop_read_after_it() {
    let body = [
        0x7E14_0300, // v_mov_b32 v10, v0 (thread id kept)
        0x4A02_0081, // v_add_nc_u32 v1, 1, v0  (lane i loops i + 1 times)
        0xBE82_0480, // s_mov_b64 s[2:3], 0
        0xBE86_0380, // s_mov_b32 s6, 0
        0x8106_8106, // loop: s_add_u32 s6, s6, 1
        0x4E02_0281, // v_subrev_nc_u32 v1, 1, v1 (v1 - 1)
        0x7D84_0280, // v_cmp_eq_u32 vcc, 0, v1
        0x8882_026A, // s_or_b64 s[2:3], vcc, s[2:3]
        0x8AFE_027E, // s_andn2_b64 exec, exec, s[2:3]
        0xBF89_FFFA, // s_cbranch_execnz loop
        0x88FE_027E, // s_or_b64 exec, exec, s[2:3]
        0x7E00_0206, // v_mov_b32 v0, s6
    ];
    let shader = build_lanes_cs(&body).unwrap_or_else(|e| panic!("translation: {e}"));
    let host = Host::new(false).unwrap();
    let (root, pso) = host.compute_pipeline(&shader).expect("pipeline");
    let ud = vec![0u32; shader.user_data_dwords().max(1) as usize];
    let out = host
        .dispatch(
            &root,
            &pso,
            &Dispatch {
                roots: vec![(0, Root::Constants(ud))],
                views: &[View::RawUav(64)],
                samplers: &[],
                groups: [1, 1, 1],
            },
        )
        .expect("dispatch");
    let got: Vec<u32> = out[0][..64].to_vec();
    assert!(got.iter().all(|&x| x == 64), "iterations counted: {got:?}");
}

/// A lane mask holding lanes from before a divergent branch and lanes added
/// inside it, counted after the branch.
#[test]
fn mask_grown_inside_a_branch_counted_after_it() {
    let body = [
        0x7E14_0300, // v_mov_b32 v10, v0
        0x7D88_0090, // v_cmp_gt_u32 vcc, 16, v0      (lanes 0-15)
        0xBE82_046A, // s_mov_b64 s[2:3], vcc
        0x7D86_00A0, // v_cmp_le_u32 vcc, 32, v0      (lanes 32-63)
        0xBE84_246A, // s_and_saveexec_b64 s[4:5], vcc
        0xBF88_0002, // s_cbranch_execz -> join
        0x7D88_00B0, // v_cmp_gt_u32 vcc, 48, v0      (of those: 32-47)
        0x8882_026A, // s_or_b64 s[2:3], vcc, s[2:3]
        0x88FE_047E, // join: s_or_b64 exec, exec, s[4:5]
        0xBE86_1002, // s_bcnt1_i32_b64 s6, s[2:3]
        0x7E00_0206, // v_mov_b32 v0, s6
    ];
    let shader = build_lanes_cs(&body).unwrap_or_else(|e| panic!("translation: {e}"));
    let host = Host::new(false).unwrap();
    let (root, pso) = host.compute_pipeline(&shader).expect("pipeline");
    let ud = vec![0u32; shader.user_data_dwords().max(1) as usize];
    let out = host
        .dispatch(
            &root,
            &pso,
            &Dispatch {
                roots: vec![(0, Root::Constants(ud))],
                views: &[View::RawUav(64)],
                samplers: &[],
                groups: [1, 1, 1],
            },
        )
        .expect("dispatch");
    let got: Vec<u32> = out[0][..64].to_vec();
    assert!(got.iter().all(|&x| x == 32), "lanes counted: {got:?}");
}

/// Nested divergent loops with a scalar counter in the inner one: the wave
/// runs 4 outer iterations, and in each the inner loop runs until its
/// longest lane (4 iterations) is done, so every lane reads 16.
#[test]
fn scalar_counted_in_nested_divergent_loops() {
    let body = [
        0x7E14_0300, // v_mov_b32 v10, v0
        0x3602_0083, // v_and_b32 v1, 3, v0          outer count - 1
        0x4A02_0281, // v_add_nc_u32 v1, 1, v1
        0x2C08_0084, // v_lshrrev_b32 v4, 4, v0      inner count - 1
        0x4A08_0881, // v_add_nc_u32 v4, 1, v4
        0xBE86_0380, // s_mov_b32 s6, 0
        0xBE82_0480, // s_mov_b64 s[2:3], 0
        0xBE8C_0480, // outer: s_mov_b64 s[12:13], 0
        0x7E06_0304, // v_mov_b32 v3, v4
        0x8106_8106, // inner: s_add_i32 s6, s6, 1
        0x4E06_0681, // v_subrev_nc_u32 v3, 1, v3
        0x7D84_0680, // v_cmp_eq_u32 vcc, 0, v3
        0x888C_0C6A, // s_or_b64 s[12:13], vcc, s[12:13]
        0x8AFE_0C7E, // s_andn2_b64 exec, exec, s[12:13]
        0xBF89_FFFA, // s_cbranch_execnz inner
        0x88FE_0C7E, // s_or_b64 exec, exec, s[12:13]
        0x4E02_0281, // v_subrev_nc_u32 v1, 1, v1
        0x7D84_0280, // v_cmp_eq_u32 vcc, 0, v1
        0x8882_026A, // s_or_b64 s[2:3], vcc, s[2:3]
        0x8AFE_027E, // s_andn2_b64 exec, exec, s[2:3]
        0xBF89_FFF2, // s_cbranch_execnz outer
        0x88FE_027E, // s_or_b64 exec, exec, s[2:3]
        0x7E00_0206, // v_mov_b32 v0, s6
    ];
    let shader = build_lanes_cs(&body).unwrap_or_else(|e| panic!("translation: {e}"));
    let host = Host::new(false).unwrap();
    let (root, pso) = host.compute_pipeline(&shader).expect("pipeline");
    let ud = vec![0u32; shader.user_data_dwords().max(1) as usize];
    let out = host
        .dispatch(
            &root,
            &pso,
            &Dispatch {
                roots: vec![(0, Root::Constants(ud))],
                views: &[View::RawUav(64)],
                samplers: &[],
                groups: [1, 1, 1],
            },
        )
        .expect("dispatch");
    let got: Vec<u32> = out[0][..64].to_vec();
    assert!(
        got.iter().all(|&x| x == 16),
        "inner iterations counted: {got:?}"
    );
}
