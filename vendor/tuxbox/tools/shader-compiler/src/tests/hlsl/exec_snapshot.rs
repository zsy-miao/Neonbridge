use super::*;

#[test]
fn four_word_load_ends_the_lifetime_of_an_overlapping_saved_pair() {
    // Save EXEC in s[30:31], elect a lane, read s31, load s[28:31], then read s30.
    let words = [
        0xbe9e_047eu32,
        0xbe98_147e,
        0x7e04_021f,
        0xf408_0702,
        0xfa00_0000,
        0xbf8c_c07f,
        0x7e08_021e,
    ];
    let bytes = words
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect::<Vec<_>>();
    let code = super::super::disassemble(&bytes);
    let plan = analyze(&code, ElectionLowering::Shared64);
    assert!(matches!(
        plan.sites.get(&0),
        Some(Site::Capture(ElectionLowering::Shared64))
    ));
    assert!(matches!(plan.sites.get(&1), Some(Site::First(0))));
    assert_eq!(plan.inputs.get(&(2, 31)), Some(&(0, true)));
    assert!(!plan.inputs.contains_key(&(5, 30)));
}
