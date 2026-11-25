use super::*;

fn write_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn blob(layout: u64, program: &[u32]) -> Vec<u8> {
    let mut image = vec![0; 0x170];
    write_u64(&mut image, 0, layout);
    for slot in 0..12 {
        write_u64(
            &mut image,
            8 + 8 * slot,
            if slot == 7 { 0x80 } else { ABSENT },
        );
    }
    write_u64(&mut image, 0x80, 0x170);
    write_u64(&mut image, 0x88, program.len() as u64 * 4);
    let name_header = 0x80 + if layout == 5 { 0x58 } else { 0x60 };
    write_u64(&mut image, name_header, 0x100);
    image[name_header + 8..name_header + 12].copy_from_slice(&1u32.to_le_bytes());
    image[name_header + 12..name_header + 16].copy_from_slice(&26u32.to_le_bytes());
    image[0x112..0x114].copy_from_slice(&6u16.to_le_bytes());
    image[0x114..0x11a].copy_from_slice(b"Sprite");
    image[0x134..0x13c].copy_from_slice(b"MainFake");
    image[0x160..0x164].copy_from_slice(&17u32.to_le_bytes());
    image.extend(program.iter().flat_map(|word| word.to_le_bytes()));
    let mut file = Vec::from(*b"FSSM");
    file.extend_from_slice(&0x101u32.to_le_bytes());
    file.extend_from_slice(&(image.len() as u64).to_le_bytes());
    file.extend(image);
    file
}

#[test]
fn follows_branch_after_early_exit_and_keeps_separate_table() {
    // The conditional jump reaches offset 12, whose block returns to the
    // endpgm at offset 4. The word at 20 is a table value.
    let words = [
        0xbf85_0002,
        0xbf81_0000,
        0,
        0xbf80_0000,
        0xbf82_fffc,
        0x3f80_0000,
        CODE_END,
        CODE_END,
    ];
    for layout in [5, 6] {
        let p = decode(&blob(layout, &words)).unwrap();
        assert_eq!(p.isa_offset, 0x180);
        assert_eq!(p.isa.len(), 20);
        assert_eq!(p.isa_data.len(), 24);
        assert_eq!(p.preamble, [17, 0, 0, 0]);
        assert_eq!(p.entry.as_deref(), Some("Sprite"));
    }
}

#[test]
fn handles_explicit_export_end_marker_and_rejects_unlinked_return() {
    let ret = 0xbe80_2020; // s_setpc_b64 s[32:33]
    assert_eq!(
        decode(&blob(6, &[ret, 0xbf81_0000, CODE_END]))
            .unwrap()
            .isa
            .len(),
        8
    );
    assert!(decode(&blob(6, &[ret, CODE_END])).is_err());
    // A reachable endpgm may be followed by a table whose first word
    // happens to have the same bits as another endpgm.
    let p = decode(&blob(6, &[0xbf81_0000, 0xbf81_0000, 3, CODE_END])).unwrap();
    assert_eq!((p.isa.len(), p.isa_data.len()), (4, 12));
}

#[test]
fn rejects_a_truncated_transfer_even_when_another_path_terminates() {
    // s_cbranch_scc1 branches past s_endpgm to a setpc with a literal.
    let mut words = vec![0xbf85_0001, 0xbf81_0000, 0xbe80_20ff];
    assert_eq!(
        decode(&blob(6, &words))
            .map(|program| program.isa.len())
            .unwrap_err(),
        "truncated shader instruction"
    );
    words.push(0x1234_5678);
    assert_eq!(decode(&blob(6, &words)).unwrap().isa.len(), 16);
}

#[test]
fn rejects_invalid_spans_and_targets() {
    assert!(decode(&blob(6, &[0xbf85_0001, 0xbe80_03ff, 0, 0xbf81_0000])).is_err());
    assert!(decode(&blob(6, &[0xbf85_7fff, 0xbf81_0000])).is_err());
    assert!(decode(&blob(5, &[0xbf85_0001, 0xbf81_0000, 0xbf80_0000])).is_err());
    let file = blob(6, &[0xbf81_0000]);
    for offset in [8, 16 + 0x80, 16 + 0x88, 16 + 0xe0] {
        let mut bad = file.clone();
        write_u64(&mut bad, offset, u64::MAX);
        assert!(decode(&bad).is_err(), "offset {offset}");
    }
}
