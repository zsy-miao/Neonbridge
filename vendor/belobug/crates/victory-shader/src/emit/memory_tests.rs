//! Hand-encoded GFX10 compute programs, with a raw UAV for readback.
use super::{Container, HostShader, emit};
use crate::gfx10::{Encoding, Fields, NULL, Operand};
use crate::native::{
    ComputeIo, NativeProgram, ResourceType, RootParam, RootType, StageIo, UsedResource,
};
use crate::{HwStage, ShaderError, gfx10, io, ir, resource, structure};

fn ds(op: u32, offset: u32, dst: u32, addr: u32, data0: u32, data1: u32) -> [u32; 2] {
    [
        0xD800_0000 | op << 18 | offset,
        dst << 24 | data1 << 16 | data0 << 8 | addr,
    ]
}

// s[8:11] is a descriptor loaded through the table in s4.
fn buf(op: u32, offset: u32, data: u32, flags: u32) -> [u32; 2] {
    [
        0xE000_0000 | (op & 127) << 18 | (op >> 7) << 25 | flags | offset,
        0x8002_0000 | data << 8,
    ]
}

fn mov(body: &mut Vec<u32>, reg: u32, value: u32) {
    body.extend([0x7E00_02FF | reg << 17, value]); // v_mov_b32 with a literal
}

fn shader(body: &[u32], threads: u32) -> Result<HostShader, ShaderError> {
    shader_with(body, threads, None, 0)
}

fn shader_with(
    body: &[u32],
    threads: u32,
    root_view: Option<ResourceType>,
    stride: u32,
) -> Result<HostShader, ShaderError> {
    let mut words = vec![
        0xBE85_0380u32, // s_mov_b32 s5, 0
        0xF408_0202,
        0xFA00_0000, // s_load_dwordx4 s[8:11], s[4:5], null
    ];
    if stride != 0 {
        words.extend([0xF408_0302, 0xFA00_0020]); // second descriptor in s[12:15]
    }
    let code: Vec<_> = words
        .iter()
        .chain(body)
        .chain(&[0xBF81_0000])
        .flat_map(|w| w.to_le_bytes())
        .collect();
    let mut native = NativeProgram {
        stage: HwStage::Cs,
        wave64: false,
        vgprs: 64,
        layout: vec![RootParam {
            root: RootType::Table,
            table_offset_dwords: 0,
            sgprs: 1,
            base_register: 0,
            user_data: 4,
            slot: 0,
        }],
        used: vec![UsedResource {
            kind: ResourceType::Uav,
            root: RootType::Table,
            table_offset_dwords: 0,
            lower_bound: 0,
            stride: 0,
            dwords: 4,
            user_data: 4,
            slot: 0,
        }],
        io: StageIo::Cs(ComputeIo {
            rsrc2: (5 << 1) | (2 << 15),
            threads: [threads, 1, 1],
        }),
        code,
    };
    if let Some(kind) = root_view {
        native.layout.push(RootParam {
            root: RootType::Descriptor,
            table_offset_dwords: 0,
            sgprs: 2,
            base_register: 0,
            user_data: 0,
            slot: 1,
        });
        native.used.push(UsedResource {
            kind,
            root: RootType::Descriptor,
            table_offset_dwords: 0,
            lower_bound: 0,
            stride: 0,
            dwords: 2,
            user_data: 0,
            slot: 1,
        });
    }
    if stride != 0 {
        native.used.push(UsedResource {
            kind: ResourceType::Uav,
            root: RootType::Table,
            table_offset_dwords: 8,
            lower_bound: 1,
            stride,
            dwords: 4,
            user_data: 4,
            slot: 0,
        });
    }
    let p = ir::build(&native.code, native.wave64, native.vgprs)?;
    let res = resource::track(&p, &native.layout, native.stage)?;
    let io = io::raise(&p, &native)?;
    let st = structure::structure(&p)?;
    let empty = super::container::build(&[]);
    emit(&Container::parse(&empty)?, &native, &p, &st, &io, &res, crate::emit::Target::ANY)
}

#[cfg(windows)]
fn check(body: &[u32], expected: &[u32], wide_lds: bool, wide_heap: bool) {
    let shader = shader(body, 1).expect("translate synthetic memory program");
    check_shader(shader, expected, wide_lds, wide_heap, None, None);
}

#[cfg(windows)]
fn check_shader(
    mut shader: HostShader,
    expected: &[u32],
    wide_lds: bool,
    wide_heap: bool,
    root_view: Option<super::host::Root>,
    extra_view: Option<super::host::View<'_>>,
) {
    use super::host::{Dispatch, Host, Root, View};
    use windows::Win32::Graphics::Direct3D12::*;

    let path = super::validator::Validator::default_path();
    if let Ok(mut validator) = super::validator::Validator::load(path.as_deref()) {
        validator
            .validate(&mut shader.dxil)
            .expect("validate memory DXIL");
    } else {
        eprintln!("no dxil.dll; skipped validation");
    }
    let Ok(host) = Host::new(false) else {
        eprintln!("no SM 6.6 device; skipped dispatch");
        return;
    };
    let mut options9 = D3D12_FEATURE_DATA_D3D12_OPTIONS9::default();
    let mut options11 = D3D12_FEATURE_DATA_D3D12_OPTIONS11::default();
    // SAFETY: the out pointers and sizes match the queried feature structs.
    unsafe {
        if wide_lds
            && (host
                .device
                .CheckFeatureSupport(
                    D3D12_FEATURE_D3D12_OPTIONS9,
                    (&raw mut options9).cast(),
                    size_of_val(&options9) as u32,
                )
                .is_err()
                || !options9.AtomicInt64OnGroupSharedSupported.as_bool())
        {
            eprintln!("no 64-bit groupshared atomics; skipped dispatch");
            return;
        }
        if wide_heap
            && (host
                .device
                .CheckFeatureSupport(
                    D3D12_FEATURE_D3D12_OPTIONS11,
                    (&raw mut options11).cast(),
                    size_of_val(&options11) as u32,
                )
                .is_err()
                || !options11
                    .AtomicInt64OnDescriptorHeapResourceSupported
                    .as_bool())
        {
            eprintln!("no 64-bit heap atomics; skipped dispatch");
            return;
        }
    }
    let (root, pso) = host.compute_pipeline(&shader).expect("create memory PSO");
    let mut roots = vec![(
        0,
        Root::Constants(vec![0; shader.user_data_dwords() as usize]),
    )];
    roots.extend(root_view.map(|view| (1, view)));
    let mut views = vec![View::RawUav(512 + expected.len() as u32)];
    views.extend(extra_view);
    let out = host
        .dispatch(
            &root,
            &pso,
            &Dispatch {
                roots,
                views: &views,
                samplers: &[],
                groups: [1, 1, 1],
            },
        )
        .expect("dispatch memory shader");
    assert_eq!(&out[0][512..], expected);
}

#[cfg(windows)]
fn output(body: &mut Vec<u32>, expected: &mut Vec<u32>, reg: u32, value: u32) {
    body.extend(buf(0x1C, 2048 + 4 * expected.len() as u32, reg, 0));
    expected.push(value);
}

#[cfg(windows)]
#[test]
fn lds_integer_atomic_updates() {
    let mut body = Vec::new();
    let mut expect = Vec::new();
    mov(&mut body, 0, 0);
    body.extend(ds(0x14, 0, 0, 0, 0, 0)); // ds_nop
    for (op, rtn) in [
        (2, false),
        (3, false),
        (4, false),
        (12, false),
        (2, true),
        (3, true),
        (4, true),
        (12, true),
        (0x34, true),
    ] {
        for (old, data, data2) in [
            (0u32, 0u32, 5),
            (0, 5, 3),
            (3, 4, 8),
            (4, 4, 0),
            (5, 4, 0x8000),
            (u32::MAX, u32::MAX, 17),
            (u32::MAX, 0, 1),
        ] {
            mov(&mut body, 3, old);
            mov(&mut body, 4, data);
            mov(&mut body, 8, data2);
            body.extend(ds(0x0D, 4, 0, 0, 3, 0));
            let opcode = if rtn && op != 0x34 { op + 0x20 } else { op };
            body.extend(ds(opcode, 4, 20, 0, 4, 8));
            if rtn {
                output(&mut body, &mut expect, 20, old);
            }
            let new = match op {
                2 => data.wrapping_sub(old),
                3 => {
                    if old >= data {
                        0
                    } else {
                        old + 1
                    }
                }
                4 => {
                    if old == 0 || old > data {
                        data
                    } else {
                        old - 1
                    }
                }
                12 => (old & !data) | data2,
                0x34 => {
                    if old >= data {
                        old - data
                    } else {
                        old.wrapping_add(data2)
                    }
                }
                _ => unreachable!(),
            };
            body.extend(ds(0x36, 4, 20, 0, 0, 0));
            output(&mut body, &mut expect, 20, new);
        }
    }
    check(&body, &expect, false, false);
}

#[cfg(windows)]
fn mov64(body: &mut Vec<u32>, reg: u32, value: u64) {
    mov(body, reg, value as u32);
    mov(body, reg + 1, (value >> 32) as u32);
}

#[cfg(windows)]
fn output64(body: &mut Vec<u32>, expected: &mut Vec<u32>, reg: u32, value: u64) {
    output(body, expected, reg, value as u32);
    output(body, expected, reg + 1, (value >> 32) as u32);
}

#[cfg(windows)]
#[test]
fn lds_64_bit_atomics_share_the_dword_allocation() {
    let mut body = Vec::new();
    let mut expect = Vec::new();
    mov(&mut body, 0, 0);
    for op in (0x40..=0x4C).chain([0x50, 0x6D]) {
        for rtn in [false, true] {
            if op == 0x6D && !rtn {
                continue;
            }
            let opcode = if rtn && op != 0x6D { op + 0x20 } else { op };
            for (old, data, data2) in [
                (0, 0, 3),
                (0, u64::MAX, 1),
                (0xFFFF_FFFF, 1, 0),
                (0x8000_0000_0000_0001, 0x8000_0000_0000_0001, 11),
                (u64::MAX, 1, 5),
            ] {
                mov64(&mut body, 2, old);
                mov64(&mut body, 4, data);
                mov64(&mut body, 8, data2);
                body.extend(ds(0x4D, 8, 0, 0, 2, 0));
                body.extend(ds(opcode, 8, 20, 0, 4, 8));
                if rtn {
                    output64(&mut body, &mut expect, 20, old);
                }
                let new = match op {
                    0x40 => old.wrapping_add(data),
                    0x41 => old.wrapping_sub(data),
                    0x42 => data.wrapping_sub(old),
                    0x43 => {
                        if old >= data {
                            0
                        } else {
                            old + 1
                        }
                    }
                    0x44 => {
                        if old == 0 || old > data {
                            data
                        } else {
                            old - 1
                        }
                    }
                    0x45 => (old as i64).min(data as i64) as u64,
                    0x46 => (old as i64).max(data as i64) as u64,
                    0x47 => old.min(data),
                    0x48 => old.max(data),
                    0x49 => old & data,
                    0x4A => old | data,
                    0x4B => old ^ data,
                    0x4C => (old & !data) | data2,
                    0x50 => {
                        if old == data {
                            data2
                        } else {
                            old
                        }
                    }
                    0x6D => data,
                    _ => unreachable!(),
                };
                body.extend(ds(0x76, 8, 20, 0, 0, 0));
                output64(&mut body, &mut expect, 20, new);
            }
        }
    }
    // A dword atomic aliases the high half of a qword.
    mov64(&mut body, 2, 0x8100_0001_AABB_CCDD);
    body.extend(ds(0x4D, 8, 0, 0, 2, 0));
    mov(&mut body, 4, 3);
    body.extend(ds(0x21, 12, 20, 0, 4, 0)); // ds_sub_rtn_u32
    output(&mut body, &mut expect, 20, 0x8100_0001);
    body.extend(ds(0x76, 8, 20, 0, 0, 0));
    output64(&mut body, &mut expect, 20, 0x80FF_FFFE_AABB_CCDD);
    check(&body, &expect, true, false);
}

#[cfg(windows)]
#[test]
fn lds_paired_exchanges() {
    for wide in [false, true] {
        let mut body = Vec::new();
        let mut expect = Vec::new();
        mov(&mut body, 0, 0);
        for strided in [false, true] {
            let off = if wide { 8 } else { 4 } * if strided { 64 } else { 1 };
            let store = if wide { 0x4D } else { 0x0D };
            let load = if wide { 0x76 } else { 0x36 };
            let op = match (wide, strided) {
                (false, false) => 0x2E,
                (false, true) => 0x2F,
                (true, false) => 0x6E,
                (true, true) => 0x6F,
            };
            let old = [0xABAB_ABAB_0123_4567, 0xCDCD_CDCD_89AB_CDEF];
            let new = [0x8901_0000_0000_000B, 0x4321_0000_0000_000D];
            mov64(&mut body, 2, old[0]);
            mov64(&mut body, 4, old[1]);
            mov64(&mut body, 6, new[0]);
            mov64(&mut body, 8, new[1]);
            body.extend(ds(store, 0, 0, 0, 2, 0));
            body.extend(ds(store, off, 0, 0, 4, 0));
            body.extend(ds(op, 1 << 8, 20, 0, 6, 8));
            if wide {
                output64(&mut body, &mut expect, 20, old[0]);
                output64(&mut body, &mut expect, 22, old[1]);
            } else {
                output(&mut body, &mut expect, 20, old[0] as u32);
                output(&mut body, &mut expect, 21, old[1] as u32);
            }
            for (address, value) in [(0, new[0]), (off, new[1])] {
                body.extend(ds(load, address, 20, 0, 0, 0));
                if wide {
                    output64(&mut body, &mut expect, 20, value);
                } else {
                    output(&mut body, &mut expect, 20, value as u32);
                }
            }
        }
        check(&body, &expect, wide, false);
    }
}

#[cfg(windows)]
#[test]
fn buffer_bytes_and_halves() {
    let mut body = Vec::new();
    let mut expect = Vec::new();
    let mut bytes = [0x47u8, 0x81, 0x19, 0x84, 0xA3, 0x51, 0x08, 0xCC];
    for (k, &chunk) in bytes.as_chunks::<4>().0.iter().enumerate() {
        mov(&mut body, 4, u32::from_le_bytes(chunk));
        body.extend(buf(0x1C, 4 * k as u32, 4, 0));
    }
    let data = 0xB69C_80EDu32;
    mov(&mut body, 4, data);
    for (op, at, width, high) in [
        (0x18, 1, 1, false),
        (0x1A, 3, 2, false),
        (0x19, 5, 1, true),
        (0x1B, 5, 2, true),
    ] {
        body.extend(buf(op, at as u32, 4, 0));
        let source = &data.to_le_bytes()[if high { 2 } else { 0 }..][..width];
        bytes[at..at + width].copy_from_slice(source);
    }
    for (op, width, signed, half) in [
        (0x08, 1, false, None),
        (0x09, 1, true, None),
        (0x0A, 2, false, None),
        (0x0B, 2, true, None),
        (0x20, 1, false, Some(0)),
        (0x21, 1, false, Some(16)),
        (0x22, 1, true, Some(0)),
        (0x23, 1, true, Some(16)),
        (0x24, 2, false, Some(0)),
        (0x25, 2, false, Some(16)),
    ] {
        for at in 1..=5 {
            let marker = 0x1357_2468;
            mov(&mut body, 20, marker);
            // Use VADDR + the immediate offset, including a split short.
            mov(&mut body, 0, at as u32 - 1);
            body.extend(buf(op, 1, 20, 1 << 12));
            let value = match (width, signed) {
                (1, true) => bytes[at] as i8 as i32 as u32,
                (1, false) => u32::from(bytes[at]),
                (2, true) => i16::from_le_bytes([bytes[at], bytes[at + 1]]) as i32 as u32,
                _ => u32::from(u16::from_le_bytes([bytes[at], bytes[at + 1]])),
            };
            let value = half.map_or(value, |shift| {
                (marker & !(0xFFFF << shift)) | ((value & 0xFFFF) << shift)
            });
            output(&mut body, &mut expect, 20, value);
        }
    }
    mov(&mut body, 0, u32::MAX);
    body.extend(buf(0x1A, 0, 4, 1 << 12)); // a split store must not wrap to byte zero
    body.extend(buf(0x0A, 0, 20, 1 << 12));
    output(&mut body, &mut expect, 20, 0);
    for (k, &chunk) in bytes.as_chunks::<4>().0.iter().enumerate() {
        body.extend(buf(0x0C, 4 * k as u32, 20, 0));
        output(&mut body, &mut expect, 20, u32::from_le_bytes(chunk));
    }
    check(&body, &expect, false, false);
}

#[cfg(windows)]
#[test]
fn buffer_integer_atomic_updates() {
    let mut body = Vec::new();
    let mut expect = Vec::new();
    for op in [0x33, 0x34, 0x3C, 0x3D] {
        for glc in [0, 1 << 14] {
            if op == 0x34 && glc == 0 {
                continue;
            }
            for (old, data) in [
                (0u32, 0u32),
                (0, 5),
                (3, 4),
                (4, 4),
                (5, 4),
                (u32::MAX, u32::MAX),
                (u32::MAX, 0),
            ] {
                mov(&mut body, 3, old);
                mov(&mut body, 4, data);
                body.extend(buf(0x1C, 0, 3, 0));
                body.extend(buf(op, 0, 4, glc));
                output(&mut body, &mut expect, 4, if glc == 0 { data } else { old });
                let new = match op {
                    0x33 => old.wrapping_sub(data),
                    0x34 => old.saturating_sub(data),
                    0x3C => {
                        if old >= data {
                            0
                        } else {
                            old + 1
                        }
                    }
                    0x3D => {
                        if old == 0 || old > data {
                            data
                        } else {
                            old - 1
                        }
                    }
                    _ => unreachable!(),
                };
                body.extend(buf(0x0C, 0, 20, 0));
                output(&mut body, &mut expect, 20, new);
            }
        }
    }
    check(&body, &expect, false, false);
}

#[cfg(windows)]
#[test]
fn buffer_64_bit_atomics() {
    let mut body = Vec::new();
    let mut expect = Vec::new();
    for op in (0x50..=0x53).chain(0x55..=0x5D) {
        for glc in [0, 1 << 14] {
            for (old, data, compare) in [
                (0, 0, 1),
                (0xFFFF_FFFF, 1, 0xFFFF_FFFF),
                (0x8000_0000_0000_0001, 2, 1),
                (u64::MAX, 1, u64::MAX),
            ] {
                mov64(&mut body, 2, old);
                mov64(&mut body, 4, data);
                mov64(&mut body, 6, compare);
                body.extend(buf(0x1D, 8, 2, 0)); // store_dwordx2
                body.extend(buf(op, 8, 4, glc));
                output64(&mut body, &mut expect, 4, if glc == 0 { data } else { old });
                let new = match op {
                    0x50 => data,
                    0x51 => {
                        if old == compare {
                            data
                        } else {
                            old
                        }
                    }
                    0x52 => old.wrapping_add(data),
                    0x53 => old.wrapping_sub(data),
                    0x55 => (old as i64).min(data as i64) as u64,
                    0x56 => old.min(data),
                    0x57 => (old as i64).max(data as i64) as u64,
                    0x58 => old.max(data),
                    0x59 => old & data,
                    0x5A => old | data,
                    0x5B => old ^ data,
                    0x5C => {
                        if old >= data {
                            0
                        } else {
                            old + 1
                        }
                    }
                    0x5D => {
                        if old == 0 || old > data {
                            data
                        } else {
                            old - 1
                        }
                    }
                    _ => unreachable!(),
                };
                body.extend(buf(0x0D, 8, 20, 0)); // load_dwordx2
                output64(&mut body, &mut expect, 20, new);
            }
        }
    }
    check(&body, &expect, false, true);
}

#[cfg(windows)]
#[test]
fn buffer_concurrent_byte_stores() {
    // Each lane writes its index to a different byte of one output dword.
    let body = buf(0x18, 2048, 0, 1 << 12);
    let shader = shader(&body, 4).unwrap();
    check_shader(shader, &[0x0302_0100], false, false, None, None);
}

#[cfg(windows)]
#[test]
fn structured_buffer_memory_uses_index_and_member_offset() {
    use super::host::View;

    let mut body = Vec::new();
    let mut expect = Vec::new();
    let indexed = |op, off, reg, flags: u32| {
        let mut words = buf(op, off, reg, flags | (1 << 13));
        words[1] += 1 << 16; // s[12:15]
        words
    };
    mov(&mut body, 0, 1);
    mov64(&mut body, 2, 0x1020_3040_ABCD_EF09);
    body.extend(indexed(0x1D, 0, 2, 0));
    mov(&mut body, 4, 0x5678);
    body.extend(indexed(0x1A, 3, 4, 0)); // cross a dword within a record
    body.extend(indexed(0x0A, 3, 20, 0));
    output(&mut body, &mut expect, 20, 0x5678);
    mov(&mut body, 4, 0x9ABC);
    body.extend(indexed(0x1A, 7, 4, 0)); // short past the end of the member
    body.extend(indexed(0x0A, 7, 20, 0));
    output(&mut body, &mut expect, 20, 0);
    mov(&mut body, 4, 0);
    body.extend(indexed(0x3C, 4, 4, 1 << 14));
    output(&mut body, &mut expect, 4, 0x1020_3056);
    body.extend(indexed(0x0D, 0, 20, 0));
    output64(&mut body, &mut expect, 20, 0x0000_0000_78CD_EF09);
    mov(&mut body, 0, 2);
    mov(&mut body, 4, 7);
    body.extend(indexed(0x3C, 0, 4, 1 << 14)); // invalid record never enters the CAS loop
    output(&mut body, &mut expect, 4, 0);
    let shader = shader_with(&body, 1, None, 8).unwrap();
    check_shader(
        shader,
        &expect,
        false,
        false,
        None,
        Some(View::StructuredUav(2, 8)),
    );
}

#[cfg(windows)]
#[test]
fn global_and_scalar_loads_through_root_views() {
    use super::host::Root;

    let mut body = Vec::new();
    let mut expect = Vec::new();
    let data: Vec<_> = (0..64)
        .map(|n| 0x0123_4567u32.wrapping_mul(n + 1))
        .collect();
    let bytes: Vec<_> = data.iter().flat_map(|x| x.to_le_bytes()).collect();
    mov(&mut body, 0, 1);
    for (op, width, signed, half) in [
        (0x08, 1, false, None),
        (0x09, 1, true, None),
        (0x0A, 2, false, None),
        (0x0B, 2, true, None),
        (0x20, 1, false, Some(0)),
        (0x21, 1, false, Some(16)),
        (0x22, 1, true, Some(0)),
        (0x23, 1, true, Some(16)),
        (0x24, 2, false, Some(0)),
        (0x25, 2, false, Some(16)),
    ] {
        for at in 1..=7 {
            let marker = 0xDEAD_BEEF;
            mov(&mut body, 20, marker);
            // global_load_* v20, v0, s[0:1] offset:(at - 1)
            body.extend([0xDC00_8000 | op << 18 | (at as u32 - 1), 20 << 24]);
            let value = match (width, signed) {
                (1, true) => bytes[at] as i8 as i32 as u32,
                (1, false) => u32::from(bytes[at]),
                (2, true) => i16::from_le_bytes([bytes[at], bytes[at + 1]]) as i32 as u32,
                _ => u32::from(u16::from_le_bytes([bytes[at], bytes[at + 1]])),
            };
            let value = half.map_or(value, |shift| {
                (marker & !(0xFFFF << shift)) | ((value & 0xFFFF) << shift)
            });
            output(&mut body, &mut expect, 20, value);
        }
    }
    let scalar_start = body.len();
    let scalar_output = expect.len();
    for op in 0..=4 {
        body.extend([0xF400_0000 | op << 18 | 20 << 6, 0xFA00_000C]);
        for k in 0..(1 << op) {
            body.push(0x7E00_0200 | 40 << 17 | (20 + k)); // v_mov_b32 v40, s[20 + k]
            output(&mut body, &mut expect, 40, data[3 + k as usize]);
        }
    }
    for (kind, body, expect) in [
        (ResourceType::Srv, &body[..], &expect[..]),
        (
            ResourceType::Cbv,
            &body[scalar_start..],
            &expect[scalar_output..],
        ),
    ] {
        // The CBV-only fixture uses the same output offsets as the SRV case.
        let padded;
        let expect = if kind == ResourceType::Cbv {
            padded = [vec![0; scalar_output], expect.to_vec()].concat();
            &padded
        } else {
            expect
        };
        let shader = shader_with(body, 1, Some(kind), 0).unwrap();
        let root = Root::View {
            srv: kind == ResourceType::Srv,
            data: data.clone(),
        };
        check_shader(shader, expect, false, false, Some(root), None);
    }
}

#[test]
#[ignore = "prints the current lowering coverage of the memory opcode tables"]
fn memory_opcode_refusals() {
    // One live invocation of each named opcode. Keep every result alive with a
    // raw store because unused read results can be dropped by the emitter.
    for encoding in [
        Encoding::Ds,
        Encoding::Mubuf,
        Encoding::Mtbuf,
        Encoding::Flat,
        Encoding::Global,
        Encoding::Scratch,
        Encoding::Smem,
    ] {
        for op in 0..=255 {
            let Some(name) = gfx10::name(encoding, op) else {
                continue;
            };
            let op = u32::from(op);
            let cases = match encoding {
                Encoding::Ds => {
                    let pair = ["ds_read2", "ds_write2", "ds_wrxchg2"]
                        .iter()
                        .any(|prefix| name.starts_with(prefix));
                    vec![("lds", ds(op, u32::from(pair) << 8, 20, 0, 4, 8))]
                }
                Encoding::Mubuf => vec![
                    (
                        "raw",
                        buf(op, 0, 20, u32::from(name.contains("atomic")) << 14),
                    ),
                    ("offset", buf(op, 4, 20, 0)),
                    ("idx?", buf(op, 0, 20, 1 << 13)),
                    ("indexed", {
                        let mut words = buf(
                            op,
                            0,
                            20,
                            1 << 13 | u32::from(name.contains("atomic")) << 14,
                        );
                        words[1] += 1 << 16;
                        words
                    }),
                ],
                Encoding::Mtbuf => [("raw", 0), ("indexed", 1 << 13)]
                    .map(|(label, flags)| {
                        (
                            label,
                            [
                                0xE800_0000 | (op & 7) << 16 | 22 << 19 | flags,
                                0x8002_1400 | (op >> 3) << 21,
                            ],
                        )
                    })
                    .to_vec(),
                Encoding::Flat | Encoding::Global | Encoding::Scratch => {
                    let seg = match encoding {
                        Encoding::Global => 2,
                        Encoding::Scratch => 1,
                        _ => 0,
                    };
                    let saddr = if encoding == Encoding::Global {
                        0
                    } else {
                        u32::from(NULL)
                    };
                    vec![(
                        "address",
                        [
                            0xDC00_0000 | op << 18 | seg << 14 | 1 << 16,
                            20 << 24 | saddr << 16 | 4 << 8,
                        ],
                    )]
                }
                Encoding::Smem => {
                    let base = if name.starts_with("s_buffer_") { 4 } else { 0 };
                    vec![(
                        "scalar",
                        [0xF400_0000 | op << 18 | 20 << 6 | base, 0xFA00_0000],
                    )]
                }
                _ => unreachable!(),
            };
            for (label, words) in cases {
                let bytes: Vec<_> = words
                    .into_iter()
                    .chain([0xBF81_0000])
                    .flat_map(u32::to_le_bytes)
                    .collect();
                let inst = &gfx10::decode(&bytes).unwrap()[0];
                assert_eq!(inst.name(), Some(name));
                let mut body = vec![0xBEFE_0380]; // s_mov_b32 m0, 0
                for reg in 0..32 {
                    mov(&mut body, reg, 0);
                }
                body.extend(words);
                match inst.fields(false) {
                    Fields::Ds {
                        dst: Some(Operand::Vector { dwords, .. }),
                        ..
                    }
                    | Fields::Flat {
                        dst: Some(Operand::Vector { dwords, .. }),
                        ..
                    }
                    | Fields::Buffer {
                        data: Some(Operand::Vector { dwords, .. }),
                        ..
                    } if name.contains("read")
                        || name.contains("load")
                        || name.contains("atomic")
                        || name.contains("rtn")
                        || name.starts_with("ds_") =>
                    {
                        for k in 0..u32::from(dwords) {
                            body.extend(buf(0x1C, 2048 + 4 * k, 20 + k, 0));
                        }
                    }
                    Fields::Smem {
                        data: Some(Operand::Scalar { dwords, .. }),
                        ..
                    } if !name.contains("store") => {
                        for k in 0..u32::from(dwords) {
                            body.push(0x7E00_0200 | 40 << 17 | (20 + k));
                            body.extend(buf(0x1C, 2048 + 4 * k, 40, 0));
                        }
                    }
                    _ => {}
                }
                let root = matches!(encoding, Encoding::Global | Encoding::Smem)
                    .then_some(ResourceType::Srv);
                let stride = if encoding == Encoding::Mubuf && label == "indexed" {
                    16
                } else {
                    0
                };
                if let Err(err) = shader_with(&body, 1, root, stride) {
                    println!("{name:34} {label:8} {err}");
                }
            }
        }
    }
}
