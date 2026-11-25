use super::*;

#[test]
fn wave_range_compatibility_checks_interval_overlap() {
    let wave64_refusal = check_wave_range((64, 64), (32, 32)).unwrap();
    assert_eq!(wave64_refusal.tag, "host-lacks-wave64");
    assert!(wave64_refusal.detail.contains("host lacks wave64"));
    assert_eq!(check_wave_range((64, 64), (32, 64)), None);
    assert_eq!(check_wave_range((64, 64), (64, 64)), None);
    assert_eq!(check_wave_range((32, 32), (32, 32)), None);
    assert_eq!(check_wave_range((32, 32), (32, 64)), None);
    assert_eq!(check_wave_range((32, 64), (32, 32)), None);
    let wave32_refusal = check_wave_range((32, 32), (64, 64)).unwrap();
    assert_eq!(wave32_refusal.tag, "host-lacks-wave32");
    let range_refusal = check_wave_range((64, 128), (32, 32)).unwrap();
    assert_eq!(range_refusal.tag, "host-wave-range-32-32-excludes-64-128");
    assert_eq!(check_wave_range((64, 64), (0, 0)), None);
    assert_eq!(check_wave_range((64, 64), (64, 32)), None);
}
