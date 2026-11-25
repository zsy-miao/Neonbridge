use super::*;
use miniz_oxide::deflate::compress_to_vec_zlib;

pub(crate) fn xbch(program: &[u8]) -> Vec<u8> {
    let compressed = compress_to_vec_zlib(program, 6);
    let mut section = Vec::from(*b"XBCH");
    section.extend(3u32.to_le_bytes());
    section.extend((compressed.len() as u64).to_le_bytes());
    section.extend((program.len() as u64).to_le_bytes());
    section.extend(compressed);
    section
}

pub(crate) fn container(parts: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut data = Vec::from(*b"XDAT");
    data.extend((parts.len() as u32).to_le_bytes());
    data.extend(36u32.to_le_bytes());
    data.resize(12 + 12 * parts.len(), 0);
    for (i, (kind, payload)) in parts.iter().enumerate() {
        let entry = 12 + 12 * i;
        let offset = (data.len() + 24) as u32;
        data[entry..entry + 4].copy_from_slice(&kind.to_le_bytes());
        data[entry + 4..entry + 8].copy_from_slice(&offset.to_le_bytes());
        data[entry + 8..entry + 12].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        data.extend(payload);
        data.resize(data.len().next_multiple_of(4), 0);
    }
    data
}

#[test]
fn returns_native_programs_in_directory_order() {
    let mut data = container(&[
        (14, xbch(b"FSSM unrelated")),
        (13, xbch(b"FSSM A")),
        (13, xbch(b"FSSM B")),
        (7, Vec::from(*b"NOBC")),
    ]);
    data[24..48].rotate_left(12);
    assert_eq!(programs(&data).unwrap(), [b"FSSM B", b"FSSM A"]);
}

#[test]
fn requires_a_bounded_directory_and_disjoint_sections() {
    let good = container(&[(13, xbch(b"FSSM")), (7, vec![0; 28])]);
    for (field, value) in [
        (4, u32::MAX),
        (8, 20),
        (8, u32::MAX),
        (16, 36),
        (20, u32::MAX),
        (28, 64),
        (32, 29),
    ] {
        let mut data = good.clone();
        data[field..field + 4].copy_from_slice(&value.to_le_bytes());
        assert!(programs(&data).is_err(), "field {field}, value {value}");
    }
    assert!(programs(&container(&[(14, xbch(b"FSSM"))])).is_err());
}

#[test]
fn verifies_size_fields_checksum_and_signature() {
    let good = xbch(b"FSSM example program");
    for (field, value) in [(8, 1u64), (16, 4), (16, 30), (16, u64::MAX)] {
        let mut bad = good.clone();
        bad[field..field + 8].copy_from_slice(&value.to_le_bytes());
        assert!(programs(&container(&[(13, bad)])).is_err());
    }
    assert!(inflate_program(&good, 8).is_err());
    let mut corrupt = good.clone();
    *corrupt.last_mut().unwrap() ^= 0xff;
    assert!(inflate_program(&corrupt, 100).is_err());
    let mut trailing = good;
    trailing.push(0);
    let stored = (trailing.len() - 24) as u64;
    trailing[8..16].copy_from_slice(&stored.to_le_bytes());
    assert!(inflate_program(&trailing, 100).is_err());
    assert!(inflate_program(&xbch(b"other"), 100).is_err());
}
