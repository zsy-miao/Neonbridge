use super::*;
use crate::rootsig::RootParam;

#[test]
fn bounded_psv_range_requires_every_physical_binding() {
    let mut program = vec![0; 0x100];
    program[..8].copy_from_slice(b"FSSM\x01\x01\0\0");
    for (field, count, rows) in [
        (0xe8, 0xf8, [[3, 0, 1, 7, 0x009u32], [3, 8, 1, 8, 0x009]]),
        (0xf0, 0xfc, [[0x300, 0, 7, 64, 0x00904], [0x300, 8, 8, 64, 0x00904]]),
    ] {
        let offset = (program.len() - 16) as u64;
        program[field..field + 8].copy_from_slice(&offset.to_le_bytes());
        program[count..count + 4].copy_from_slice(&2u32.to_le_bytes());
        program.extend(rows.into_iter().flatten().flat_map(u32::to_le_bytes));
    }
    let size = (program.len() - 16) as u32;
    program[8..12].copy_from_slice(&size.to_le_bytes());
    let root = RootSig { version: 2, flags: 0,
        params: vec![RootParam { visibility: 7,
            kind: RootParamKind::Table(vec![RootRange { range_type: 0, num: 2,
                base_register: 7, space: 1, offset_in_table: 0 }]) }],
        num_static_samplers: 0, static_samplers: Vec::new() };
    let layout = MeshLayout::read(&program, &[[5, 1, 7, 8]], &root).unwrap();
    assert_eq!(layout.tables[&(17, 0)].register, 7);
    assert_eq!(layout.tables[&(17, 32)].register, 8);
    assert!(layout.replacements.is_empty());
    assert!(MeshLayout::read(&program, &[[5, 1, 7, 9]], &root).is_none());
    assert!(MeshLayout::read(&program, &[[5, 1, 7, u32::MAX]], &root).is_none());
    assert!(MeshLayout::read(&program, &[[5, 1, 7, 8], [5, 1, 8, 8]], &root).is_none());
    program[0x114..0x118].copy_from_slice(&4u32.to_le_bytes());
    assert!(MeshLayout::read(&program, &[[5, 1, 7, 8]], &root).is_none());
}

#[test]
fn remaps_mesh_slots_only_if_both_fssm_tables_agree() {
    let mut program = vec![0; 0x100];
    program[..8].copy_from_slice(b"FSSM\x01\x01\0\0");
    for (field, count, rows) in [
        (0xe8, 0xf8, [[3, 16, 1, 7, 0x109u32], [0, 0, 2, 500, 0x20a]]),
        (
            0xf0,
            0xfc,
            [[0x300, 16, 7, 64, 0x10904], [2, 0, 500, 0, 0x20a04]],
        ),
    ] {
        let offset = (program.len() - 16) as u64;
        program[field..field + 8].copy_from_slice(&offset.to_le_bytes());
        program[count..count + 4].copy_from_slice(&2u32.to_le_bytes());
        program.extend(rows.into_iter().flatten().flat_map(u32::to_le_bytes));
    }
    let size = (program.len() - 16) as u32;
    program[8..12].copy_from_slice(&size.to_le_bytes());
    let table = RootParam {
        visibility: 0,
        kind: RootParamKind::Table(vec![RootRange {
            range_type: 0,
            num: 1,
            base_register: 4,
            space: 1,
            offset_in_table: 0,
        }]),
    };
    let root = RootSig {
        version: 2,
        flags: 0,
        params: vec![
            table.clone(),
            table,
            RootParam {
                visibility: 0,
                kind: RootParamKind::Constants {
                    register: 500,
                    space: 0,
                    num32: 2,
                },
            },
        ],
        num_static_samplers: 0,
        static_samplers: Vec::new(),
    };
    let resources = [[5, 1, 7, 7], [2, 0, 500, 500]];
    let layout = MeshLayout::read(&program, &resources, &root).unwrap();
    let RootParamKind::Table(ranges) = &layout.replacements[&1] else {
        panic!("expected a table")
    };
    assert_eq!((ranges[0].offset_in_table, ranges[0].base_register), (2, 7));
    assert_eq!(layout.tables[&(17, 64)].stride_words, Some(16));
    assert!(layout.takes_sgpr(19));
    assert!(!layout.takes_sgpr(20));
    program[0x104..0x108].copy_from_slice(&8u32.to_le_bytes());
    assert!(MeshLayout::read(&program, &resources, &root).is_none());
}
