use super::*;

#[test]
fn lds_subtotal_branches_must_be_uniform_and_stay_in_the_loop() {
    // v7 = local X / 64. Accumulate one LDS word in v8 for each preceding wave.
    let words = [
        0x2c0e0086, 0x7e100280, 0xbe80047e, 0x7daa0e80, 0xbf880013, 0xbe82047e, 0xbe86047e, 0xbe920380, 0xbe910380,
        0xbf820002, 0x87fe0610, 0x7e220508, 0x8f108212, 0x81128112, 0x7e100210, 0xd8d80000, 0x08000008, 0xbf8cc07f,
        0x4a101011, 0xbe90047e, 0x7da40e12, 0xbf800000, 0x8a867e06, 0xbf85fff2, 0xbefe0400, 0xbf810000,
    ];
    let recognize = |words: &[u32]| {
        let isa: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        subtotal_loop(&disassemble(&isa))
    };
    assert!(recognize(&words).is_some());
    let mut different_lane = words;
    different_lane[0] = 0x2c0e0085; // local X / 32 varies within wave64
    assert!(recognize(&different_lane).is_none());
    let mut wrong_edge = words;
    wrong_edge[23] += 1;
    assert!(recognize(&wrong_edge).is_none());
    let mut alias = words;
    alias[5] = words[2]; // clobbers the outer saved EXEC pair
    assert!(recognize(&alias).is_none());
}
