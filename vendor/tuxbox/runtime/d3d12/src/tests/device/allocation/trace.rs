use super::parse_sizes;

#[test]
fn parses_exact_decimal_hex_and_invalid_sizes() {
    assert_eq!(parse_sizes("8192, 0x4000, invalid, 8192"), Some(vec![8192, 16384, u64::MAX]));
    for bad in ["", "0x", "-1", "8,", "8192,garbage", "18446744073709551616"] {
        assert_eq!(parse_sizes(bad), None, "{bad}");
    }
    let too_many = (0..17).map(|n| n.to_string()).collect::<Vec<_>>().join(",");
    assert_eq!(parse_sizes(&too_many), None);
}
