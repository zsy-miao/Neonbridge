use super::*;

fn fixture(rows: &[[u32; 5]]) -> Vec<u8> {
    let start = 0x160;
    let mut data = vec![0u8; start];
    data[..8].copy_from_slice(b"FSSM\x01\x01\0\0");
    data[0xf0..0xf8].copy_from_slice(&((start - 16) as u64).to_le_bytes());
    data[0xfc..0x100].copy_from_slice(&(rows.len() as u32).to_le_bytes());
    for row in rows {
        data.extend(row.iter().flat_map(|v| v.to_le_bytes()));
    }
    let size = (data.len() - 16) as u32;
    data[8..12].copy_from_slice(&size.to_le_bytes());
    data
}

#[test]
fn reads_reflected_stride_for_structured_bindings_only() {
    let rows = [
        [0x300, 24, 3, 800, 0x404],
        [0x303, 8, 1, 44, 0x30704],
        [0x300, 0, 0, 1, 0x404],
    ];
    let data = fixture(&rows);
    let bindings = [(5, 3), (8, 1), (4, 0)].map(|(res_type, slot)| PsvResource {
        res_type,
        space: 0,
        lower: slot,
        upper: slot,
    });
    assert_eq!(
        stride_words(&data, &bindings, Stage::Compute, 3, false),
        Some(200)
    );
    assert_eq!(
        stride_words(&data, &bindings, Stage::Compute, 1, true),
        Some(11)
    );
    assert_eq!(
        stride_words(&data, &bindings, Stage::Compute, 0, false),
        None
    );
    assert_eq!(
        stride_words(&data, &bindings, Stage::Compute, 1, false),
        None
    );

    let binding = &bindings[..1];
    for row in [
        [0x300, 8, 3, 800, 0x404],
        [0x300, 24, 3, 799, 0x404],
        [0x300, 24, 3, 800, 0x408],
    ] {
        assert_eq!(
            stride_words(&fixture(&[row]), binding, Stage::Compute, 3, false),
            None
        );
    }
    assert_eq!(
        stride_words(&fixture(&[rows[0]; 2]), binding, Stage::Compute, 3, false),
        None
    );
    assert_eq!(
        stride_words(&data[..data.len() - 1], binding, Stage::Compute, 3, false),
        None
    );
    let mut out_of_range = data;
    out_of_range[0xf0..0xf8].copy_from_slice(&u64::MAX.to_le_bytes());
    assert_eq!(
        stride_words(&out_of_range, binding, Stage::Compute, 3, false),
        None
    );
}

#[test]
fn pixel_srv_stride_includes_unread_fields() {
    // These MainPS inputs only fetch the first word or vector of each record.
    let data = fixture(&[[0x300, 0, 0, 16, 0x804], [0x300, 16, 2, 32, 0x804]]);
    let resources = [0, 2].map(|slot| PsvResource {
        res_type: 5,
        space: 0,
        lower: slot,
        upper: slot,
    });
    assert_eq!(
        stride_words(&data, &resources, Stage::Pixel, 0, false),
        Some(4)
    );
    assert_eq!(
        stride_words(&data, &resources, Stage::Pixel, 2, false),
        Some(8)
    );
    assert_eq!(
        stride_words(&data, &resources, Stage::Compute, 0, false),
        None
    );
    assert_eq!(stride_words(&data, &resources, Stage::Pixel, 0, true), None);
}

#[test]
fn vertex_and_mesh_views_keep_the_complete_record() {
    let resource = [PsvResource { res_type: 5, space: 0, lower: 3, upper: 3 }];
    for (stage, tag, other) in [
        (Stage::Vertex, 0x30b04, Stage::Mesh),
        (Stage::Mesh, 0x91104, Stage::Vertex),
    ] {
        let table = fixture(&[[0x300, 24, 3, 800, tag]]);
        assert_eq!(stride_words(&table, &resource, stage, 3, false), Some(200));
        assert_eq!(stride_words(&table, &resource, other, 3, false), None);
    }
}
