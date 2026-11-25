use super::*;

pub(crate) fn container_with(parts: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
    let mut file = Vec::from(*b"DXBC");
    file.extend_from_slice(&[0x1d; 16]);
    file.extend_from_slice(&1u32.to_le_bytes());
    file.extend_from_slice(&[0; 4]);
    file.extend_from_slice(&(parts.len() as u32).to_le_bytes());
    file.resize(32 + parts.len() * 4, 0);
    for (index, (tag, payload)) in parts.iter().enumerate() {
        let position = file.len() as u32;
        file[32 + 4 * index..36 + 4 * index].copy_from_slice(&position.to_le_bytes());
        file.extend_from_slice(tag.as_slice());
        file.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        file.extend_from_slice(payload);
    }
    let size = file.len() as u32;
    file[24..28].copy_from_slice(&size.to_le_bytes());
    file
}

#[test]
fn preserves_directory_order_and_respects_embedded_size() {
    let mut file = container_with(&[(b"AAAA", &[8, 3, 1]), (b"BBBB", &[])]);
    let declared = file.len();
    file[32..40].rotate_left(4);
    file.extend_from_slice(b"trailing data");
    let parsed = Dxbc::parse(&file).unwrap();
    assert_eq!(parsed.digest, [0x1d; 16]);
    assert_eq!(parsed.total_size as usize, declared);
    assert_eq!(&parsed.chunks[0].fourcc, b"BBBB");
    let part = parsed.chunk(b"AAAA").unwrap();
    assert_eq!(&file[part.offset..part.offset + part.size], &[8, 3, 1]);
    assert!(parsed.chunk(b"CCCC").is_none());
}

#[test]
fn rejects_directory_and_part_ranges_outside_declared_bounds() {
    let file = container_with(&[(b"TEST", &[0; 8])]);
    for (position, invalid) in [
        (24, 16),
        (24, 51),
        (24, 53),
        (28, u32::MAX),
        (32, 4),
        (32, 34),
        (32, 49),
        (32, u32::MAX),
        (40, u32::MAX),
    ] {
        let mut corrupt = file.clone();
        corrupt[position..position + 4].copy_from_slice(&invalid.to_le_bytes());
        assert!(
            Dxbc::parse(&corrupt).is_err(),
            "field {position} value {invalid}"
        );
    }
    for n in 0..file.len() {
        assert!(Dxbc::parse(&file[..n]).is_err(), "length {n}");
    }
    assert!(Dxbc::parse(&container_with(&[])).unwrap().chunks.is_empty());
}

#[test]
fn detects_duplicate_or_overlapping_part_headers() {
    let file = container_with(&[(b"LEFT", &[0; 8]), (b"RGHT", &[])]);
    for second in [40u32, 48] {
        let mut corrupt = file.clone();
        corrupt[36..40].copy_from_slice(&second.to_le_bytes());
        assert!(Dxbc::parse(&corrupt).is_err());
    }
}
