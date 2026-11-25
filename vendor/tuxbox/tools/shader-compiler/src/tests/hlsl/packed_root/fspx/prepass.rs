use super::*;

#[test]
fn maps_descriptor_indices_across_ranges_and_rejects_ambiguous_slots() {
    let range = |base_register, num, offset_in_table| RootRange {
        range_type: 0,
        num,
        base_register,
        space: 0,
        offset_in_table,
    };
    let ranges = [range(6, 1, 0), range(17, 2, u32::MAX)];
    assert_eq!(table_register(&ranges, 0).unwrap(), 6);
    assert_eq!(table_register(&ranges, 1).unwrap(), 17);
    assert_eq!(table_register(&ranges, 2).unwrap(), 18);
    assert!(table_register(&ranges, 3).is_err());
    let ranges = [range(4, 2, 2), range(9, 1, u32::MAX)];
    assert!(table_register(&ranges, 1).is_err());
    assert_eq!(table_register(&ranges, 4).unwrap(), 9);
    assert!(table_register(&[range(0, 2, 0), range(8, 1, 1)], 1).is_err());
    assert!(table_register(&[range(u32::MAX, 2, 0)], 1).is_err());
}
