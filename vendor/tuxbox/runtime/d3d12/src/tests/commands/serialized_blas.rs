use super::*;

fn store(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn quad() -> Vec<u8> {
    let mut data = vec![0; 256];
    for (offset, word) in [
        (32, 256),
        (40, 192),
        (64, 0x80000004),
        (68, 2),
        (72, 0xff1),
        (76, 1),
        (92, 1f32.to_bits()),
        (96, 1f32.to_bits()),
        (112, 20),
        (116, 64),
        (120, 64),
        (124, 0x7f7f7f),
        (176, 128),
        (180, 1),
        (184, 0),
        (188, 0x0902),
        (192, 8),
        (196, u32::MAX),
        (200, u32::MAX),
        (204, u32::MAX),
    ] {
        store(&mut data, offset, word);
    }
    for offset in [140, 156, 164, 168] {
        store(&mut data, offset, 1f32.to_bits());
    }
    data
}

#[test]
fn decodes_paired_triangle_winding_and_primitive_ids() {
    let ts = decode(&quad()).unwrap();
    assert_eq!(ts.len(), 2);
    assert_eq!(
        ts[0],
        Triangle {
            geometry: 0,
            primitive: 0,
            vertices: [[1., 0., 0.], [1., 1., 0.], [0., 1., 0.]]
        }
    );
    assert_eq!(
        ts[1],
        Triangle {
            geometry: 0,
            primitive: 1,
            vertices: [[1., 0., 0.], [0., 1., 0.], [0., 0., 0.]]
        }
    );
}

#[test]
fn accepts_shared_leaf_and_preserves_missing_geometry_and_primitive_indices() {
    let mut bytes = quad();
    store(&mut bytes, 196, 8); // same leaf in two node child boxes
    store(&mut bytes, 176, 0x280); // geometry 2
    store(&mut bytes, 180, 4);
    store(&mut bytes, 184, 1);
    let dense = decode_for_build(&bytes).unwrap();
    let ids: Vec<_> = dense.iter().map(|t| (t.geometry, t.primitive)).collect();
    assert_eq!(
        ids,
        [(0, 0), (1, 0), (2, 0), (2, 1), (2, 2), (2, 3), (2, 4)]
    );
    for (i, triangle) in dense.iter().enumerate() {
        assert_eq!(triangle.vertices[0][0].is_nan(), ![3, 6].contains(&i));
    }
    store(&mut bytes, 180, 0x10000000);
    assert!(decode_for_build(&bytes).is_err());
}

#[test]
fn rejects_invalid_fields_cycles_and_unknown_nodes() {
    let good = quad();
    for (offset, bad) in [
        (0, 9),
        (32, 255),
        (40, 511),
        (48, 1),
        (64, 0x80000005),
        (68, u32::MAX),
        (76, u32::MAX),
        (112, u32::MAX),
        (116, u32::MAX),
        (128, f32::NAN.to_bits()),
        (176, 0x87),
        (176, 0),
        (180, 0),
        (180, u32::MAX),
        (184, u32::MAX),
        (188, 0x905),
        (188, 0x10009),
        (192, 20),
        (192, 9),
        (192, 40),
    ] {
        let mut bytes = good.clone();
        store(&mut bytes, offset, bad);
        assert!(
            decode(&bytes).is_err(),
            "accepted field offset {offset} = {bad:#x}"
        );
    }
    for end in 0..good.len() {
        assert!(decode(&good[..end]).is_err());
    }
}
