use super::*;

// v_mov_b32 v2, 1.0; v_mov_b32 v3, 0; pack (v2,v3) and (v3,v2);
// exp mrt0 v0,v1 compr done vm; s_endpgm.
const RED: [u32; 7] = [
    0x7e04_02f2,
    0x7e06_0280,
    0x5e00_0702,
    0x5e02_0503,
    0xf800_1c0f,
    0x0000_0100,
    0xbf81_0000,
];

fn bytes(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn channels(isa: &[u8]) -> Option<[u32; 4]> {
    match exports(isa, &[4])? {
        PixelOutput::Export(colors) => colors.into_iter().next(),
        PixelOutput::Discard => None,
    }
}

#[test]
fn packs_the_low_and_high_channels_in_export_order() {
    assert_eq!(
        channels(&bytes(&RED)),
        Some([1.0f32.to_bits(), 0, 0, 1.0f32.to_bits()])
    );
    assert!(exact_half(65504.0f32.to_bits()));
    assert!(!exact_half(65536.0f32.to_bits()));
    assert!(!exact_half(f32::NAN.to_bits()));
    assert!(!exact_half(1.0001f32.to_bits()));
}

#[test]
fn depth_only_requires_a_valid_null_export_and_no_mask_changes() {
    assert_eq!(
        exports(&bytes(&[0xf800_1890, 0, 0xbf81_0000]), &[]),
        Some(PixelOutput::Export(vec![]))
    );
    for word in [0xf800_1090, 0xf800_0890, 0xf800_1891, 0xf800_1800] {
        assert!(exports(&bytes(&[word, 0, 0xbf81_0000]), &[]).is_none());
    }
    assert_eq!(
        exports(&bytes(&[0xbefe_0480, 0xf800_1890, 0, 0xbf81_0000]), &[]),
        Some(PixelOutput::Discard)
    );
    assert!(exports(&bytes(&[0xbf81_0000]), &[]).is_none());
}

#[test]
fn follows_a_proven_lane_mask_to_a_discard() {
    // Save EXEC, subtract it from itself, branch past an unreachable
    // MRT, and deliver the empty mask to the rasterizer.
    let path = [
        0xbe84_047e,
        0x8a86_7e04,
        0xbf84_0003,
        0xf800_1c0f,
        0,
        0xbf81_0000,
        0xbefe_0406,
        0xf800_1890,
        0,
        0xbf81_0000,
    ];
    assert_eq!(exports(&bytes(&path), &[4]), Some(PixelOutput::Discard));
    for (i, word) in [
        (0, 0xbe84_0406),
        (1, 0x8786_7e04),
        (2, 0xbf84_ffff),
        (2, 0xbf84_0001),
        (6, 0xbefe_0404),
    ] {
        let mut invalid = path;
        invalid[i] = word;
        assert!(exports(&bytes(&invalid), &[4]).is_none());
    }
}

#[test]
fn validates_mrt_order_final_flags_and_declared_width() {
    let mut shader = RED[..4].to_vec();
    shader.extend([0xf800_0403, 0, 0xf800_1c1f, 0x0000_0001, 0xbf81_0000]);
    let Some(PixelOutput::Export(colors)) = exports(&bytes(&shader), &[2, 4]) else {
        panic!("missing MRT outputs")
    };
    let one = 1.0f32.to_bits();
    assert_eq!(colors, [[one, 0, 0, 0], [0, one, one, 0]]);
    for widths in [&[4, 4][..], &[1], &[], &[2, 4, 4]] {
        assert!(exports(&bytes(&shader), widths).is_none());
    }
    for (i, word) in [(4, 0xf800_1c03), (6, 0xf800_041f), (6, 0xf800_1c0f)] {
        let mut bad = shader.clone();
        bad[i] = word;
        assert!(exports(&bytes(&bad), &[2, 4]).is_none());
    }
}

#[test]
fn rejects_live_inputs_control_flow_and_other_export_modes() {
    for (i, word) in [
        (0, 0x7e04_0202),
        (1, 0xbf82_0000),
        (2, 0x5e00_0704),
        (3, 0xbe_fe_03_80),
        (4, 0xf800_180f),
        (4, 0xf800_1c1f),
        (4, 0xf800_1c03),
        (4, 0xf800_0c0f),
        (4, 0xf800_140f),
        (6, 0xbf80_0000),
    ] {
        let mut bad = RED;
        bad[i] = word;
        assert_eq!(channels(&bytes(&bad)), None, "word {i} = {word:#x}");
    }
    let mut extra = bytes(&RED);
    extra.extend(0xbf81_0000u32.to_le_bytes());
    assert_eq!(channels(&extra), None);
    assert_eq!(channels(&bytes(&RED)[..25]), None);
}
