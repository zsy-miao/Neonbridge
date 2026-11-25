use super::*;

#[test]
fn readfirstlane_selects_lane_zero_when_both_exec_words_are_empty() {
    // v_readfirstlane_b32 s28, v9
    let ops = crate::rdna2::disassemble(&0x7e38_0509u32.to_le_bytes());
    for mesh in [false, true] {
        let code = exchange(0, &ops[0], mesh).unwrap().unwrap();
        assert!(code.contains("first_mask0.y != 0u ? 32u + uint(firstbitlow(first_mask0.y)) : 0u"), "{code}");
    }
}
