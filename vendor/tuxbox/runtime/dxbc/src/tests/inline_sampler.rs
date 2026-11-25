#[test]
fn comparison_sampler_recovers_opaque_white_border_and_less_equal() {
    // State seen in ProjectScarlett issue #39. No game shader bytes needed.
    let words = [0x30b6, 0xc00000, 0x18500000, 0x80000000];
    assert_eq!(
        super::descriptor(words, 7, 32765, 5, true),
        Some([0x95, 4, 4, 3, 0, 1, 4, 2, 0, 12.0_f32.to_bits(), 7, 32765, 5])
    );
    assert!(super::descriptor(words, 7, 32765, 5, false).is_none());
    for (word, mask) in [(0, 0x8000), (1, 1), (2, 0x10000), (3, 1), (3, 0x40000000)] {
        let mut bad = words;
        bad[word] ^= mask;
        assert!(super::descriptor(bad, 7, 32765, 5, true).is_none());
    }
}

#[test]
fn comparison_sampler_state_is_recovered_at_the_image_site() {
    let mut words = [
        0xb008_30b6u32,
        0xbe89_03ff,
        0x00c0_0000,
        0xbe8a_03ff,
        0x1850_0000,
        0xbe8b_03ff,
        0x8000_0000,
        0xf0a0_0f08,
        0x0040_0a0a, // image_sample_c
        0xbf81_0000,
    ];
    let classify = |code: &[u32]| {
        let isa: Vec<_> = code.iter().flat_map(|x| x.to_le_bytes()).collect();
        super::sites(&isa, 5)
    };
    let sample = classify(&words).unwrap();
    assert_eq!(sample.descriptors.len(), 1);
    assert_eq!(
        sample.descriptors[0],
        [0x95, 4, 4, 3, 0, 1, 4, 2, 0, 12.0_f32.to_bits(), 0, 32765, 5]
    );
    assert_eq!(sample.sites.get(&28), Some(&0));
    words[7] = 0xf080_0f08; // ordinary image_sample must not receive a comparison sampler
    assert!(classify(&words).is_none());
}

#[test]
fn identical_literal_has_distinct_normal_and_comparison_slots() {
    let words = [
        0xb008_0092u32,
        0xbe89_03ff,
        0x00c0_0000,
        0xbe8a_03ff,
        0x1450_0000,
        0xbe8b_0380,
        0xf080_0f08,
        0x0040_0a0a, // image_sample
        0xf0a0_0f08,
        0x0040_0a0a, // image_sample_c
        0xbf81_0000,
    ];
    let isa: Vec<_> = words.iter().flat_map(|x| x.to_le_bytes()).collect();
    let sample = super::sites(&isa, 5).unwrap();
    assert_eq!(sample.descriptors.len(), 2);
    assert_eq!(sample.descriptors[0][0], 0x14);
    assert_eq!(sample.descriptors[1][0], 0x94);
    assert_eq!(sample.sites.get(&24), Some(&0));
    assert_eq!(sample.sites.get(&32), Some(&1));
}

#[test]
fn all_six_logged_literal_states_match_the_same_static_sampler_decoder() {
    // Six distinct states observed in the ProjectScarlett #39 log.
    for (words, compare, filter, aniso) in [
        ([0x30b6, 0xc00000, 0x18500000, 0x80000000], true, 0x95, 1),
        ([0x92, 0xc00000, 0x14500000, 0], false, 0x14, 1),
        ([0, 0xc00000, 0x14000000, 0], false, 0, 1),
        ([0x92, 0xc00000, 0x14000000, 0], false, 0, 1),
        ([0x400, 0xc00000, 0x18f00000, 0], false, 0x55, 4),
        ([0x92, 0xc00000, 0x18500000, 0], false, 0x15, 1),
    ] {
        let sampler = super::descriptor(words, 3, 7, 5, compare).unwrap();
        assert_eq!((sampler[0], sampler[5]), (filter, aniso));
        assert!(super::matches_static(words, &sampler, compare));
        // All state fields (including LODs, compare function, and
        // anisotropy) must match. Register/space are binding identity.
        for index in 0..10 {
            let mut wrong = sampler;
            wrong[index] ^= 1;
            assert!(!super::matches_static(words, &wrong, compare), "field {index}");
        }
        assert!(!super::matches_static(words, &sampler, !compare));
    }
    for border in 0..=2 {
        let words = [0x1b6, 0xc00000, 0x14500000, border << 30];
        let sampler = super::descriptor(words, 0, 0, 0, false).unwrap();
        assert_eq!(sampler[7], border);
        assert!(super::matches_static(words, &sampler, false));
    }
}

#[test]
fn pair_moves_recover_both_words_without_guessing() {
    let mut words = [
        0xb00c_0092u32, // s_movk_i32 s12, 0x92
        0xbe8d_03ff, 0x00c0_0000, // s_mov_b32 s13, max LOD
        0xbe8e_03ff, 0x1450_0000, // s_mov_b32 s14, linear
        0xbe8f_0380, // s_mov_b32 s15, zero
        0xbe88_040c, // s_mov_b64 s[8:9], s[12:13]
        0xbe8a_040e, // s_mov_b64 s[10:11], s[14:15]
        0xf080_0f08, 0x0040_0a0a,
        0xbf81_0000,
    ];
    let classify = |code: &[u32]| {
        let isa: Vec<_> = code.iter().flat_map(|x| x.to_le_bytes()).collect();
        super::sites(&isa, 5)
    };
    let result = classify(&words).unwrap();
    assert_eq!(result.descriptors[0][0], 0x14);
    assert_eq!(result.sites.get(&32), Some(&0));
    words[6] = 0xbe88_040a; // reads uninitialized s[10:11]
    assert!(classify(&words).is_none());
}

#[test]
fn a_loop_backedge_cannot_initialize_a_sampler_at_entry() {
    let words = [
        0xf080_0f08u32, 0x0040_0a0a, // sample before any initialization
        0xb008_0092,
        0xbe89_03ff, 0x00c0_0000,
        0xbe8a_03ff, 0x1450_0000,
        0xbe8b_0380,
        0xbf82_fff7, // branch back to entry
    ];
    let isa: Vec<_> = words.iter().flat_map(|x| x.to_le_bytes()).collect();
    assert!(super::sites(&isa, 5).is_none());
}

#[test]
fn sampler_words_must_agree_on_every_reachable_loop_path() {
    // s[8:11] describe linear clamped sampling. The preheader skips a
    // conflicting definition of s10 immediately before the loop header.
    let mut words = [
        0xb008_0092u32, // s_movk_i32 s8, 0x92
        0xbe89_03ff,
        0x00c0_0000, // s_mov_b32 s9, max LOD 12
        0xbe8a_03ff,
        0x1450_0000, // s_mov_b32 s10, linear filtering
        0xbe8b_0380, // s_mov_b32 s11, 0
        0xbf82_0002, // s_branch sample
        0xbe8a_03ff,
        0x1400_0000, // unreachable point filtering
        0xf080_0f08,
        0x0040_0a0a, // sample: image_sample ... s[8:11]
        0xbf85_fffd, // s_cbranch_scc1 sample
        0xbf81_0000, // s_endpgm
    ];
    let classify = |code: &[u32]| {
        let isa: Vec<_> = code.iter().flat_map(|x| x.to_le_bytes()).collect();
        super::sites(&isa, 5)
    };
    let sample = classify(&words).unwrap();
    assert_eq!(sample.descriptors.len(), 1);
    assert_eq!(sample.descriptors[0][0], 0x14);
    assert_eq!(sample.descriptors[0][12], 5);
    assert_eq!(sample.sites.get(&36), Some(&0));
    words[6] = 0xbf84_0002; // s_cbranch_scc0 also allows the point filter.
    assert!(classify(&words).is_none());
}
