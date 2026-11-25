use super::super::translate_valu_probe;

#[test]
fn wave32_comparisons_and_carry_leave_the_next_scalar_intact() {
    // v_cmp_gt_u32 s3,v0,v1 witnesses wave32. The following comparison
    // and add-with-carry use even destinations in that same program.
    let words = [0xd4c40003, 0x02020300, 0xd4c40022, 0x02020300, 0xd70f0200, 0x02020300];
    let narrow = translate_valu_probe(&words, 2).unwrap();
    for assignment in ["s4 = 0u;", "s35 = 0u;", "s3 = 0u;"] {
        assert!(!narrow.lines().any(|line| line.trim() == assignment));
    }
    // Even-only masks retain the existing high word when width is unknown.
    let wide = translate_valu_probe(&words[2..], 2).unwrap();
    assert!(wide.lines().any(|line| line.trim() == "s35 = 0u;"));
    assert!(wide.lines().any(|line| line.trim() == "s3 = 0u;"));
}
