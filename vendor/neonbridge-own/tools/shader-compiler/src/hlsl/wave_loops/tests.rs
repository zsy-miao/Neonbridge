use super::super::{translate, ShaderExtract, Stage};
use crate::meta::{RootParam, RootParamKind, RootRange, RootSig};

mod fixture {
    include!("../../../examples/support/wave_guard_fixture.rs");
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    Uniform,
    ReadOnlyBuffer,
    WritableBuffer,
    BypassDescriptor,
    ClobberDescriptor,
    BypassCounterInit,
    EmptyExec,
    ClobberSavedExec,
    NoBitmapUpdate,
    TruncatedBitmapUpdate,
    TrailingLoop,
    ResolveSpill,
    AliasedTable,
    HeapBuffer,
    UnresolvedOffset,
    PerInvocation,
}

fn branch(op: u32, from: usize, to: usize) -> u32 {
    0xbf80_0000 | (op << 16) | (to as isize - from as isize - 1) as i16 as u16 as u32
}

/// An OR reduction followed by a bitmap scan. The selected bit indexes
/// a four-dword CBV load, whose first word controls a branch in the scan.
fn shader(width: u32, shape: Shape) -> ShaderExtract {
    let (mut shader, mut reduction, end) = fixture::shader(width);
    reduction.truncate(end);
    for word in &mut reduction {
        if *word >> 24 == 0x4a {
            *word = (*word & 0x00ff_ffff) | 0x3800_0000;
        }
    }
    let smov = |dst: u32, src: u32| 0xbe80_0300 | (dst << 16) | src;
    let vmove = |dst: u32, src: u32| 0x7e00_0200 | (dst << 17) | src;
    let sop2 =
        |op: u32, dst: u32, a: u32, b: u32| 0x8000_0000 | (op << 23) | (dst << 16) | (b << 8) | a;

    let mut code = vec![smov(7, 145), 0]; // EXECZ skips the complete outer loop.
    let resolve = shape != Shape::PerInvocation;
    if resolve {
        code.extend([
            0xd4c2_001f,
            0x0202_0480, // v_cmp_eq_u32 s31, 0, v2
            0x7d84_0480, // v_cmp_eq_u32 vcc_lo, 0, v2
        ]);
        if shape == Shape::ResolveSpill {
            code.extend([
                0xd761_0017,
                0x0201_001f, // v_writelane_b32 v23, s31, 0
                0xd760_001e,
                0x0201_0117, // v_readlane_b32 s30, v23, 0
                smov(106, 30),
            ]);
        }
        code.extend([0xbf86_0001, 0x7e00_0000]); // skip a nop if VCC is zero
    }
    let bypass_init = if shape == Shape::BypassCounterInit {
        let at = code.len();
        code.extend([0xbf06_807e, 0]); // SCC is lane-dependent.
        Some(at + 1)
    } else {
        None
    };
    code.push(smov(19, 128));
    if shape == Shape::BypassDescriptor {
        code.extend([0xbf06_8013, 0xbf85_0002]);
    }
    match shape {
        Shape::AliasedTable => code.extend([smov(32, 4), 0xf408_0510, 0xfa00_0000]),
        Shape::HeapBuffer => code.extend([smov(32, 128), 0xf408_0505, 0x4000_0000]),
        _ => code.extend([0xf408_0502, 0xfa00_0000]),
    }
    code.push(smov(11, 128));
    if shape == Shape::ClobberDescriptor {
        code.push(smov(23, 128));
    }
    let outer = code.len();
    if let Some(at) = bypass_init {
        code[at] = branch(5, at, outer);
    }
    code.extend(reduction);
    if shape == Shape::EmptyExec {
        code.push(smov(126, 128));
    }
    if shape == Shape::ClobberSavedExec {
        if width == 32 {
            code.extend([0xbe88_3dc1, smov(8, 128), smov(126, 8)]);
        } else {
            code.extend([0xbe88_25c1, smov(9, 128), 0xbefe_0408]);
        }
    }
    code.extend([vmove(5, 7), smov(12, 128)]);
    let inner = code.len();
    code.extend([
        0x7e1c_0505,           // v_readfirstlane_b32 s14, v5
        0xbe90_130e,           // s_ff1_i32_b32 s16, s14
        sop2(14, 16, 16, 159), // and s16, s16, 31
        sop2(30, 15, 129, 16), // shift one by s16
    ]);
    code.push(sop2(30, 18, 16, 132));
    let offset = if shape == Shape::UnresolvedOffset {
        126 << 25
    } else {
        18 << 25
    };
    code.extend([
        0xf428_060a,
        offset, // s_buffer_load_dwordx4 s[24:27], s[20:23], s18
        0x939c_ff18,
        0x0001_0000, // extract the low bit
        0xbf06_811c,
        0xbf85_0001, // skip marker increment based on the CBV value
        0x8113_8113,
        sop2(18, 17, 15, 14), // clear the selected bit
    ]);
    if shape == Shape::TruncatedBitmapUpdate {
        // v_mov_b32_sdwa v5, s17 dst_sel:DWORD dst_unused:UNUSED_PAD src0_sel:WORD_0
        code.extend([0x7e0a_02f9, 0x0084_0611]);
    } else {
        code.push(vmove(
            5,
            if shape == Shape::NoBitmapUpdate {
                14
            } else {
                17
            },
        ));
    }
    code.push(sop2(2, 12, 12, 129));
    code.push(0xbf06_0e0f); // s_cmp_eq_u32 s15, s14
    code.push(branch(4, code.len(), inner));
    code.extend([0x810b_810b, 0xbf06_820b]); // increment the outer count, then compare to two.
    code.push(branch(4, code.len(), outer));
    code[1] = branch(8, 1, code.len());
    if shape == Shape::TrailingLoop {
        code.extend([smov(32, 128), sop2(2, 32, 32, 129), 0xbf06_82a0]);
        code.push(branch(4, code.len(), code.len() - 2));
    }
    code.push(0xbf81_0000);

    if resolve {
        shader.entry = Some("CS_RaytracingResolve_Synthetic".into());
        // The two table pointers, constants, and heap pointers reserve the
        // first twelve SGPRs. Group IDs follow in s[12:14].
        let rows = [
            [3, 0, 1, 0, 4u32],
            [3, 0, 1, 0, 0x105],
            [0, 0, 4, 1, 0x206],
            [5, 0, 1, 0, 10],
            [6, 0, 1, 0, 11],
        ];
        shader.fssm = vec![0; 0x100];
        shader.fssm[..8].copy_from_slice(b"FSSM\x01\x01\0\0");
        shader.fssm[0xe8..0xec].copy_from_slice(&0xf0u32.to_le_bytes());
        shader.fssm[0xf8..0xfc].copy_from_slice(&(rows.len() as u32).to_le_bytes());
        shader
            .fssm
            .extend(rows.into_iter().flatten().flat_map(u32::to_le_bytes));
        let payload = shader.fssm.len() as u32 - 16;
        shader.fssm[8..12].copy_from_slice(&payload.to_le_bytes());
    }
    if shape == Shape::HeapBuffer {
        shader.required_features = Some(1 << 25);
    }
    shader.has_root_sig = true;
    shader.root_sig = Some(RootSig {
        version: 1,
        flags: if shape == Shape::HeapBuffer { 0x400 } else { 0 },
        num_static_samplers: 0,
        static_samplers: Vec::new(),
        params: vec![RootParam {
            visibility: 0,
            kind: RootParamKind::Table(vec![RootRange {
                range_type: match shape {
                    Shape::WritableBuffer => 1,
                    Shape::ReadOnlyBuffer => 0,
                    _ => 2,
                },
                num: 1,
                base_register: 0,
                space: 0,
                offset_in_table: 0,
            }]),
        }],
    });
    shader.isa = code.into_iter().flat_map(u32::to_le_bytes).collect();
    shader.isa_data = shader.isa.clone();
    shader
}

#[test]
fn accepts_full_mask_branches_and_uniform_scalar_reads() {
    for shape in [
        Shape::Uniform,
        Shape::ReadOnlyBuffer,
        Shape::ResolveSpill,
        Shape::AliasedTable,
        Shape::HeapBuffer,
    ] {
        let translated = translate(&shader(32, shape)).expect("bounded reduction");
        assert_eq!(translated.required_wave_size, Some(32));
        assert!(translated.hlsl.contains("if (vcc_lo == 0u)"));
        assert!(translated.hlsl.contains("if (exec_lo == 0u)"));
        assert!(!translated.hlsl.contains("++tb_guard < 65536u"));
    }
}

#[test]
fn refuses_unproved_ctrl_flow_and_buffer_accesses() {
    for shape in [
        Shape::WritableBuffer,
        Shape::BypassDescriptor,
        Shape::ClobberDescriptor,
        Shape::BypassCounterInit,
        Shape::EmptyExec,
        Shape::ClobberSavedExec,
        Shape::NoBitmapUpdate,
        Shape::TruncatedBitmapUpdate,
        Shape::UnresolvedOffset,
        Shape::PerInvocation,
    ] {
        let native = translate(&shader(32, shape))
            .ok()
            .and_then(|t| t.required_wave_size);
        assert_eq!(native, None, "{shape:?}");
    }
}

#[test]
fn keeps_the_watchdog_for_other_loops() {
    let translated = translate(&shader(32, Shape::TrailingLoop)).expect("bounded reduction");
    assert_eq!(translated.required_wave_size, Some(32));
    assert_eq!(translated.hlsl.matches("++tb_guard < 65536u").count(), 1);
}
