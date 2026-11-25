use super::*;

fn recognizes(words: &[u32], group: [u32; 3]) -> bool {
    let isa: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    prepare(group, &mut crate::rdna2::disassemble(&isa))
}

#[test]
fn partial_group_reads_only_proven_launched_lanes() {
    // In a 7x3 group, registers and inline operands can select lanes 0..20.
    const SET: u32 = 0xbe98_0394; // s_mov_b32 s24,20
    const READ: [u32; 2] = [0xd760_001c, 0x0000_3109]; // s28,v9,s24
    const END: u32 = 0xbf81_0000;
    let check = |words: &[u32]| {
        let isa: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let mut ops = crate::rdna2::disassemble(&isa);
        let before = ops.iter().map(|op| op.mnemonic.clone()).collect::<Vec<_>>();
        let accepted = constant_reads([7, 3, 1], &mut ops, true);
        if accepted {
            assert!(ops.iter().any(|op| op.mnemonic == "tb_indexed_readlane"));
            assert!(!ops.iter().any(|op| op.mnemonic == "v_readlane_b32"));
        } else {
            assert!(ops.iter().map(|op| &op.mnemonic).eq(&before));
        }
        accepted
    };
    assert!(check(&[SET, READ[0], READ[1], END]));
    assert!(!check(&[SET + 1, READ[0], READ[1], END]));
    assert!(check(&[READ[0], 0x0001_2909, END])); // inline lane 20
    assert!(!check(&[READ[0], 0x0001_2b09, END])); // inline lane 21
    // Equal incoming values at a forward branch join prove the selector.
    assert!(check(&[SET, 0xbf84_0001, SET, READ[0], READ[1], END]));
    assert!(!check(&[SET, 0xbf84_0001, SET + 1, READ[0], READ[1], END]));
    assert!(check(&[SET, 0xbf84_0001, SET - 1, READ[0], READ[1], END]));
    // Keep the runtime selector when both CSELECT operands are in range.
    assert!(check(&[0x8518_8a94, 0xbe99_0318, READ[0], 0x0000_3309, END]));
    assert!(!check(&[0x8518_9594, READ[0], READ[1], END])); // 20 or 21
    assert!(!check(&[0x8518_0594, READ[0], READ[1], END])); // 20 or unknown
    // CMOV keeps the previous destination when SCC is clear.
    const CMOV_20: u32 = 0xbe98_0594; // s_cmov_b32 s24,20
    assert!(check(&[SET - 10, CMOV_20, READ[0], READ[1], END]));
    assert!(!check(&[CMOV_20, READ[0], READ[1], END]));
    assert!(!check(&[SET + 1, CMOV_20, READ[0], READ[1], END]));
    assert!(!check(&[SET, CMOV_20 + 1, READ[0], READ[1], END]));
    assert!(!check(&[SET, 0xbe98_0505, READ[0], READ[1], END])); // source s5 unknown
    assert!(check(&[0x8718_8f05, READ[0], READ[1], END])); // s5 & 15
    assert!(!check(&[0x8718_9f05, READ[0], READ[1], END])); // s5 & 31
    assert!(!check(&[0x8718_8f05, 0x8018_a018, READ[0], READ[1], END])); // add 32
    // The first read overwrites the selector with the selected VGPR value.
    assert!(!check(&[SET, 0xd760_0018, READ[1], READ[0], READ[1], END]));
}

#[test]
fn partial_group_selector_tracks_each_word_of_a_scalar_pair() {
    const OLD: u32 = 0xbe99_038a; // s_mov_b32 s25,10
    const NEW: u32 = 0xbe9b_0394; // s_mov_b32 s27,20
    const MOV: u32 = 0xbe98_041a; // s_mov_b64 s[24:25],s[26:27]
    const CMOV: u32 = 0xbe98_061a; // conditional copy of the same pair
    const SELECT: u32 = 0x8598_181a; // select s[26:27] or s[24:25]
    let reads_high = |setup: &[u32]| {
        let words = [setup, &[0xd760_001c, 0x0000_3309, 0xbf81_0000]].concat();
        let bytes: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        constant_reads([7, 3, 1], &mut crate::rdna2::disassemble(&bytes), true)
    };
    assert!(reads_high(&[NEW, MOV]));
    assert!(!reads_high(&[OLD, MOV])); // unknown source overwrites known high word
    for op in [CMOV, SELECT] {
        assert!(reads_high(&[OLD, NEW, op])); // low words are unknown
        assert!(!reads_high(&[NEW, op]));
        assert!(!reads_high(&[OLD, op]));
        assert!(!reads_high(&[OLD, NEW + 1, op]));
        assert!(!reads_high(&[OLD + 11, NEW, op]));
    }
    assert!(reads_high(&[0xbe98_0487])); // high word of inline 7 is zero
    assert!(!reads_high(&[OLD, 0xbe98_04c1])); // high word of -1 is out of range
}

#[test]
fn empty_wave_fallback_cannot_be_entered_from_another_edge() {
    let pruned = |words: &[u32]| {
        let isa: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let mut ops = crate::rdna2::disassemble(&isa);
        skip_empty_wave_fallback(&mut ops)
            && !ops.iter().any(|op| op.mnemonic == "v_readlane_b32")
    };
    let words = [
        0xbf800000, 0xbeea037e, 0xbf870002, 0xd760000b, 0x00000d01, 0xbf800000, 0xbf810000,
    ];
    assert!(pruned(&words));
    for (index, word) in [
        (0, 0xbefe0380),
        (1, 0xbeea0380),
        (2, 0xbf860002),
        (5, 0xbf82fffd),
        (5, 0xbf82fffb),
        (5, 0xbe802000),
    ] {
        let mut changed = words;
        changed[index] = word;
        assert!(!pruned(&changed));
    }
}

#[test]
fn lane_election_requires_an_intact_full_wave_predicate() {
    let words = [
        0xd4850004, 0x00010081, // v_cmp_ne_i32 s[4:5], 1, 0
        0xbf800000, // s_nop
        0xbed01404, // s_ff1_i32_b64 s80, s[4:5]
        0xd766000c, 0x00020cc1, // v_mbcnt_hi_u32_b32
        0xd7600000, 0x0000a104, // v_readlane_b32 s0, v4, s80
        0xbf810000,
    ];
    assert!(recognizes(&words, [16, 16, 1]));
    assert!(!recognizes(&words, [32, 1, 1]));
    for change in [0xbe850380, 0xbf820001, 0xbefe047e] {
        let mut changed = words;
        changed[2] = change; // clobber mask high, branch, write EXEC
        assert!(!recognizes(&changed, [16, 16, 1]));
    }
    let mut equal = words;
    equal[1] = 0x00010080; // empty mask
    assert!(!recognizes(&equal, [16, 16, 1]));
}
