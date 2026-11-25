use super::*;

#[test]
fn leaf_payload_offsets_and_return_token_have_distinct_bounds() {
    let mut words = vec![
        3611688968, 73991433, // v8 = v9 * 4 + v26 (incoming payload)
        3761246244, 67116296, // v29 = payload[9]
        3765440512, 67110408, // payload[0] = v6
        3611688968, 73991431, // v8 = v7 * 4 + v26 (return token)
        3761246208, 67110920,   // v8 = scratch[v8]
        3196067870, // return to scheduler
    ];
    let run = |words: &[u32], count| {
        let bytes: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        Stack::analyze(&bytes, &(0..bytes.len()), count)
    };
    assert_eq!(run(&words, 10).unwrap().return_read, 32);
    assert!(run(&words, 9).is_err());
    words[8] |= 4; // return token must be at offset zero
    assert!(run(&words, 10).is_err());
    words[8] &= !4;
    words.insert(0, 0xbf82_0002); // skip the payload address calculation
    assert!(run(&words, 10).is_err());
}

#[test]
fn renamed_payload_address_must_reach_each_store() {
    let mut words = vec![
        0xd746002d, 0x04690509, // v45 = v9 * 4 + v26
        0x7e32032d, // v25 = v45
        0xe0701000, 0x04000619, // payload[v25] = v6
        0xd7460008, 0x04690507, // v8 = v7 * 4 + v26
        0xe0301000, 0x04000808, // v8 = scratch[v8]
        0xbe80201e, // return
    ];
    let analyze = |words: &[u32]| {
        let bytes = words
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect::<Vec<_>>();
        Stack::analyze(&bytes, &(0..bytes.len()), 16)
    };
    assert!(analyze(&words).is_ok());
    words.insert(2, 0xbf85_0001); // one predecessor skips the copy into v25
    assert!(analyze(&words).is_err());
    // Restoring the same alias in complementary EXEC arms covers every
    // lane that continues under their saved mask.
    words.splice(
        2..4,
        [
            0xbeb7_3c35, // s_and_saveexec_b32 s55, s53
            0xbf88_0001, // skip THEN for an empty mask
            0x7e32_032d, // v25 = v45
            0x8a7e_7e37, // EXEC = s55 & ~EXEC
            0xbf88_0001, // skip ELSE for an empty mask
            0x7e32_032d, // v25 = v45
            0xbefe_0337, // EXEC = s55
        ],
    );
    assert!(analyze(&words).is_ok());
    words[7] = 0x7e32_032c; // ELSE now uses a different VGPR
    assert!(analyze(&words).is_err());
}
