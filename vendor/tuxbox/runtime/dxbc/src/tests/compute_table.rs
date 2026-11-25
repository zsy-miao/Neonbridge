use super::*;
use crate::rootsig::{RootParam, RootParamKind, RootRange, RootSig};

fn put_u32(buf: &mut [u8], at: usize, value: u32) {
    buf[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_u64(buf: &mut [u8], at: usize, value: u64) {
    buf[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

fn program(user: &[[u32; 5]], used: &[[u32; 5]]) -> Vec<u8> {
    let user_at = 0xf0u64;
    let used_at = user_at + (user.len() * 20) as u64;
    let len = 16 + used_at as usize + used.len() * 20;
    let mut bytes = vec![0u8; len];
    bytes[..8].copy_from_slice(b"FSSM\x01\x01\0\0");
    put_u32(&mut bytes, 8, (len - 16) as u32);
    put_u64(&mut bytes, 0xe8, user_at);
    put_u64(&mut bytes, 0xf0, used_at);
    put_u32(&mut bytes, 0xf8, user.len() as u32);
    put_u32(&mut bytes, 0xfc, used.len() as u32);
    for (index, row) in user.iter().chain(used).enumerate() {
        let at = if index < user.len() { user_at } else { used_at };
        let row_index = if index < user.len() { index } else { index - user.len() };
        for (word, value) in row.iter().enumerate() {
            put_u32(&mut bytes, 16 + at as usize + row_index * 20 + word * 4, *value);
        }
    }
    bytes
}

fn root(ranges: Vec<RootRange>) -> RootSig {
    RootSig {
        version: 2,
        flags: 0,
        params: vec![
            RootParam { visibility: 0, kind: RootParamKind::Table(Vec::new()) },
            RootParam { visibility: 0, kind: RootParamKind::Table(ranges) },
        ],
        num_static_samplers: 0,
        static_samplers: Vec::new(),
    }
}

#[test]
fn corroborated_cbv_uses_the_free_register_only() {
    let image = program(&[[3, 0xc8, 1, 28, 0x105]], &[[0x302, 0xc8, 28, 0, 0x10504]]);
    let resources = [[2u32, 1, 28, 28]];
    assert_eq!(corroborated_descriptor(&image, &resources, 5, 5, 0x320), Some((2, 28, 1)));
    let open = root(vec![RootRange {
        range_type: 0, num: 16, base_register: 0, space: 0, offset_in_table: 0,
    }]);
    assert_eq!(uncovered_descriptor(&image, &resources, 5, 5, 0x320, &open), Some((2, 28, 1)));
    let taken = root(vec![RootRange {
        range_type: 2, num: 1, base_register: 28, space: 1, offset_in_table: 16,
    }]);
    assert_eq!(uncovered_descriptor(&image, &resources, 5, 5, 0x320, &taken), Some((2, 25, 32_701)));
}

#[test]
fn rejects_ambiguous_unaligned_and_non_descriptor_slots() {
    let duplicated = program(
        &[[3, 0xc8, 1, 28, 0x105], [3, 0xc8, 1, 28, 0x105]],
        &[[0x302, 0xc8, 28, 0, 0x10504]],
    );
    assert_eq!(corroborated_descriptor(&duplicated, &[[2, 1, 28, 28]], 5, 5, 0x320), None);
    let sampler = program(&[[3, 0, 1, 3, 0x105]], &[[0x401, 0, 3, 0, 0x10500]]);
    assert_eq!(corroborated_descriptor(&sampler, &[[1, 0, 3, 3]], 5, 5, 0), None);
    let image = program(&[[3, 0xc8, 1, 28, 0x105]], &[[0x302, 0xc8, 28, 0, 0x10504]]);
    assert_eq!(corroborated_descriptor(&image, &[[2, 1, 28, 28]], 5, 5, 0x324), None);
}

#[test]
fn raster_descriptor_in_s9_table_is_recognized() {
    let (kind, psv) = descriptor_class(0x300, 0x908, 0, 0, 5, &[]).unwrap();
    assert_eq!((kind, psv), (0, 3));
    let (kind, psv) = descriptor_class(0x302, 0x904, 0, 1, 8, &[]).unwrap();
    assert_eq!((kind, psv), (2, 2));
}

fn tables_then(kinds: &[RootParamKind]) -> RootSig {
    RootSig {
        version: 2,
        flags: 0,
        params: kinds.iter().cloned().map(|kind| RootParam { visibility: 0, kind }).collect(),
        num_static_samplers: 0,
        static_samplers: Vec::new(),
    }
}

#[test]
fn leading_tables_counts_only_the_first_run_of_tables() {
    let table = RootParamKind::Table(Vec::new());
    let constants = RootParamKind::Constants { register: 500, space: 0, num32: 1 };
    let srv = RootParamKind::Descriptor { kind: 3, register: 0, space: 3 };
    assert_eq!(leading_tables(&tables_then(&[table.clone(), table.clone(), table.clone(), srv, constants.clone()])), 3);
    assert_eq!(leading_tables(&tables_then(&[table.clone(), constants.clone(), table.clone()])), 1);
    assert_eq!(leading_tables(&tables_then(&[constants, table])), 0);
}

#[test]
fn multi_table_raster_records_name_their_table() {
    let table = RootParamKind::Table(Vec::new());
    let constants = RootParamKind::Constants { register: 500, space: 0, num32: 1 };
    let two = tables_then(&[table.clone(), table.clone(), constants]);
    // Table N is read from s8 + N; the location keeps the descriptor width.
    assert_eq!(multi_table_descriptor(0x0808, &two), Some((0, 0x808)));
    assert_eq!(multi_table_descriptor(0x1_0908, &two), Some((1, 0x808)));
    assert_eq!(multi_table_descriptor(0x1_0904, &two), Some((1, 0x804)));
    // A table index that disagrees with the SGPR, or names a parameter that is
    // not a leading table, fails closed.
    assert_eq!(multi_table_descriptor(0x1_0808, &two), None);
    assert_eq!(multi_table_descriptor(0x0908, &two), None);
    assert_eq!(multi_table_descriptor(0x2_0a08, &two), None);
    let three = tables_then(&[table.clone(), table.clone(), table]);
    assert_eq!(multi_table_descriptor(0x2_0a04, &three), Some((2, 0x804)));
}

#[test]
fn raster_stage_bindings_merge_only_when_they_agree() {
    let b = |table, kind, register, space, slot| Binding { table, kind, register, space, slot };
    let vs = vec![b(1, 0, 4, 1, 2), b(0, 2, 0, 1, 0)];
    // Shared identical binding, plus a new one: merged and sorted by (table, slot).
    assert_eq!(merge_raster_bindings(vs.clone(), vec![b(0, 2, 0, 1, 0), b(0, 0, 3, 0, 5)]),
        Some(vec![b(0, 2, 0, 1, 0), b(0, 0, 3, 0, 5), b(1, 0, 4, 1, 2)]));
    // One slot, two registers.
    assert_eq!(merge_raster_bindings(vs.clone(), vec![b(1, 0, 9, 1, 2)]), None);
    // One register, two slots.
    assert_eq!(merge_raster_bindings(vs.clone(), vec![b(0, 0, 4, 1, 7)]), None);
    // Same register number in another space or kind is a different binding.
    assert_eq!(merge_raster_bindings(vs, vec![b(0, 0, 4, 2, 7), b(0, 1, 4, 1, 8)]).map(|v| v.len()), Some(4));
}

/// A one-program raster stage: FSSM user and used rows, and PSV resource rows
/// ([type, space, lower, upper]; one per used row).
fn raster_shader(stage: u32, user: &[[u32; 5]], used: &[[u32; 5]], psv_rows: &[[u32; 4]]) -> Vec<u8> {
    let xdat = crate::xdat::tests::container(&[(13, crate::xdat::tests::xbch(&program(user, used)))]);
    let mut dxil = vec![0u8; 24];
    put_u32(&mut dxil, 0, stage << 16);
    dxil[8..12].copy_from_slice(b"DXIL");
    dxil.extend(xdat);
    let mut psv = vec![0u8; 36];
    put_u32(&mut psv, 0, 32);
    psv[28] = stage as u8;
    psv.extend((psv_rows.len() as u32).to_le_bytes());
    psv.extend(16u32.to_le_bytes());
    psv.extend(psv_rows.iter().flatten().flat_map(|w| w.to_le_bytes()));
    crate::container::tests::container_with(&[(b"DXIL", &dxil), (b"PSV0", &psv)])
}

/// [table, table, table, root SRV t4 space3, constants b500].
fn three_tables_srv_constants() -> RootSig {
    let table = RootParamKind::Table(Vec::new());
    tables_then(&[table.clone(), table.clone(), table,
        RootParamKind::Descriptor { kind: 3, register: 4, space: 3 },
        RootParamKind::Constants { register: 500, space: 0, num32: 1 }])
}

#[test]
fn multi_table_root_srv_carries_the_shader_register() {
    let root = three_tables_srv_constants();
    // Parameter 3's address arrives in s11-s12; the shader names it t5 space7.
    let used = [[0x100, 0, 5, 1, 0x3_0b04]];
    let srv = raster_shader(1, &[[1, 0, 2, 5, 0x30b]], &used, &[[4, 7, 5, 5]]);
    assert_eq!(root_bindings(&srv, 1, &root),
        Some(vec![Binding { table: 3, kind: ROOT_SRV, register: 5, space: 7, slot: 0 }]));
    // Without the user-data row, or when parameter 3 is not a root SRV, it fails closed.
    assert_eq!(root_bindings(&raster_shader(1, &[], &used, &[[4, 7, 5, 5]]), 1, &root), None);
    let mut constants = root.clone();
    constants.params[3].kind = RootParamKind::Constants { register: 5, space: 7, num32: 2 };
    assert_eq!(root_bindings(&srv, 1, &constants), None);
    // Root-less callers never see a root SRV binding.
    assert_eq!(bindings(&srv, 1), None);
}

#[test]
fn multi_table_raster_buffer_views_follow_psv() {
    let root = three_tables_srv_constants();
    // A raw UAV in table 0 (s8) and a typed SRV buffer in table 1 (s9).
    let user = [[3, 8, 1, 2, 0x008], [3, 16, 1, 3, 0x109]];
    let used = [[0x303, 8, 2, 1, 0x0804], [0x300, 16, 3, 4, 0x1_0904]];
    let shader = raster_shader(0, &user, &used, &[[7, 1, 2, 2], [3, 2, 3, 3]]);
    assert_eq!(root_bindings(&shader, 0, &root), Some(vec![
        Binding { table: 0, kind: 1, register: 2, space: 1, slot: 1 },
        Binding { table: 1, kind: 0, register: 3, space: 2, slot: 2 },
    ]));
    // PSV must confirm the view type: a raw UAV named typed is refused.
    assert_eq!(root_bindings(&raster_shader(0, &user, &used, &[[6, 1, 2, 2], [3, 2, 3, 3]]), 0, &root), None);
    // Under a single-table root the same typed SRV buffer keeps its old result.
    let single = tables_then(&[RootParamKind::Table(Vec::new()),
        RootParamKind::Constants { register: 500, space: 0, num32: 1 }]);
    let typed = raster_shader(0, &[[3, 16, 1, 3, 0x008]], &[[0x300, 16, 3, 4, 0x0804]], &[[3, 2, 3, 3]]);
    assert_eq!(root_bindings(&typed, 0, &single), None);
    assert_eq!(bindings(&typed, 0), None);
    let uav = raster_shader(0, &[[3, 8, 1, 2, 0x008]], &[[0x303, 8, 2, 1, 0x0804]], &[[7, 1, 2, 2]]);
    assert_eq!(root_bindings(&uav, 0, &single), None);
    assert_eq!(bindings(&uav, 0), None);
}

#[test]
fn same_register_at_two_slots_fails_closed() {
    let image = program(
        &[[3, 0xc8, 1, 28, 0x105], [3, 0xd0, 1, 28, 0x105]],
        &[[0x302, 0xc8, 28, 0, 0x10504], [0x302, 0xd0, 28, 0, 0x10504]],
    );
    let resources = [[2u32, 1, 28, 28]];
    assert_eq!(corroborated_descriptor(&image, &resources, 5, 5, 0x320), None);
    assert_eq!(corroborated_descriptor(&image, &resources, 5, 5, 0x340), None);
}
