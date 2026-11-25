use super::*;

// s[2:3] = {s7, 0}; v0 = -1, v1 = 1; s_load_dwordx4 s[0:3],
// s[2:3], 0; wait; store v[0:1] at byte 8; endpgm.
const STORE: [u32; 10] = [
    0xbe820307, 0xbe830380, 0x7e0002c1, 0x7e020281, 0xf4080001, 0xfa000000, 0xbf8cc07f,
    0xe0740008, 0x80000000, 0xbf810000,
];

fn bytes(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

#[test]
fn writes_only_selected_dwords_in_the_record() {
    assert_eq!(
        store_body(&bytes(&STORE), 16).as_deref(),
        Some("    output[0].words[2] = 0xffffffffu;\n    output[0].words[3] = 0x00000001u;\n")
    );
    assert_eq!(store_body(&bytes(&STORE), 12), None);
}

#[test]
fn uses_the_encoded_word_count_for_three_and_four_word_stores() {
    let mut words = STORE.to_vec();
    words.splice(4..4, [0x7e040282, 0x7e060283]); // v2 = 2, v3 = 3
    for (word, width) in [(0xe0780000, 4), (0xe07c0000, 3)] {
        words[9] = word;
        let body = store_body(&bytes(&words), 16).unwrap();
        assert_eq!(body.lines().count(), width);
        assert_eq!(store_body(&bytes(&words), (width as u32 - 1) * 4), None);
    }
}

#[test]
fn validates_table_pointer_registers_and_structured_stride() {
    let mut image = vec![0u8; 0x240];
    fn set(bytes: &mut [u8], at: usize, n: u32) {
        bytes[at..at + 4].copy_from_slice(&n.to_le_bytes());
    }
    for (at, value) in [
        (0, 6),
        (0x60, 0x90),
        (0xb8, 0x120),
        (0xc0, 0x1b0),
        (0xc8, 4),
        (0xcc, 1),
        (0x1b0, 0x303),
        (0x1bc, 12),
        (0x1c0, 0x30704),
    ] {
        set(&mut image, at, value);
    }
    for i in 0..4 {
        let at = 0x120 + i * 20;
        for (j, val) in [3, 0, 1, 0, (i as u32) << 8 | (4 + i as u32)]
            .into_iter()
            .enumerate()
        {
            set(&mut image, at + j * 4, val);
        }
    }
    assert_eq!(single_uav_stride(&image), Some(12));
    for (off, replacement) in [
        (0x64, 1),
        (0xc8, 3),
        (0x16c, 0x306),
        (0x1b4, 4),
        (0x1bc, 3),
        (0x1bc, 2052),
        (0x1c0, 0x20704),
    ] {
        let mut bad = image.clone();
        set(&mut bad, off, replacement);
        assert_eq!(single_uav_stride(&bad), None, "image offset {off:#x}");
    }
}

#[test]
fn rejects_unknown_addresses_unsupported_flags_and_missing_waits() {
    for (i, word) in [
        (0, 0xbe820306),
        (1, 0xbe830381),
        (2, 0x7e000204),
        (4, 0xf4080081),
        (5, 0xfa000020),
        (6, 0xbf800000),
        (7, 0xe0742008),
        (7, 0xe074000a),
        (8, 0x81000000),
        (8, 0x80010000),
        (9, 0xbf820000),
    ] {
        let mut other = STORE;
        other[i] = word;
        assert_eq!(store_body(&bytes(&other), 16), None, "word {i}={word:x}");
    }
}
