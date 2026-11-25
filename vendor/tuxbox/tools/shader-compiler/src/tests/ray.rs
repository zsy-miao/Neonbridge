use super::immediate;

#[test]
fn preserves_immediate_width_at_masked_operands() {
    let mut code = 0x7e22028d_u32.to_le_bytes().to_vec();
    code.extend(0x00002001_u32.to_le_bytes());
    assert_eq!(immediate(&code, 0, 1), Some(13));
    code[0] = 0xff;
    assert_eq!(immediate(&code, 0, 1), None);
    assert_eq!(immediate(&code, 0, 2), Some(0x2001));
    assert_eq!(immediate(&code[..4], 0, 2), None);
}
