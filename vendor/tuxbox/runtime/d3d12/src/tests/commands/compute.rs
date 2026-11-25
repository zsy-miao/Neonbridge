use super::*;

#[test]
fn slot80_thread_dimensions_convert_to_rounded_groups_and_fail_closed() {
    // Without USE_THREAD_DIMENSIONS (0x20), dimensions are already group counts,
    // even when CS_W32_EN (0x8000) or no compute group size is present.
    assert_eq!(
        resolve_flagged_groups([45, 1, 1], 0, None, false).unwrap(),
        FlaggedGroups { groups: [45, 1, 1], used_threads: false, partial: false }
    );
    assert_eq!(
        resolve_flagged_groups([3, 2, 1], DISPATCH_CS_W32_EN, None, false).unwrap(),
        FlaggedGroups { groups: [3, 2, 1], used_threads: false, partial: false }
    );

    // With USE_THREAD_DIMENSIONS (0x20 or 0x8020), thread counts divide by [gx, gy, gz] rounded up.
    assert_eq!(
        resolve_flagged_groups([45, 1, 1], DISPATCH_USE_THREAD_DIMENSIONS, Some([64, 1, 1]), true).unwrap(),
        FlaggedGroups { groups: [1, 1, 1], used_threads: true, partial: true }
    );
    assert_eq!(
        resolve_flagged_groups([1, 1, 1], DISPATCH_USE_THREAD_DIMENSIONS | DISPATCH_CS_W32_EN, Some([8, 4, 2]), true).unwrap(),
        FlaggedGroups { groups: [1, 1, 1], used_threads: true, partial: true }
    );
    assert_eq!(
        resolve_flagged_groups([128, 16, 4], DISPATCH_USE_THREAD_DIMENSIONS, Some([64, 8, 2]), true).unwrap(),
        FlaggedGroups { groups: [2, 2, 2], used_threads: true, partial: false }
    );
    // Zero on any thread axis yields zero groups on that axis and no partial boundary.
    assert_eq!(
        resolve_flagged_groups([0, 5, 1], DISPATCH_USE_THREAD_DIMENSIONS, Some([64, 4, 1]), true).unwrap(),
        FlaggedGroups { groups: [0, 2, 1], used_threads: true, partial: false }
    );
    // Large thread counts stay overflow-safe and scale down to valid desktop group counts.
    assert_eq!(
        resolve_flagged_groups([70_000, 1, 1], DISPATCH_USE_THREAD_DIMENSIONS, Some([8, 1, 1]), true).unwrap(),
        FlaggedGroups { groups: [8750, 1, 1], used_threads: true, partial: false }
    );
    assert_eq!(
        resolve_flagged_groups([u32::MAX, u32::MAX, 1], DISPATCH_USE_THREAD_DIMENSIONS, Some([2, 1, 1]), true).unwrap(),
        FlaggedGroups { groups: [1u32 << 31, u32::MAX, 1], used_threads: true, partial: true }
    );

    // Unknown or zero [numthreads] fails closed when 0x20 is set.
    assert_eq!(
        resolve_flagged_groups([45, 1, 1], DISPATCH_USE_THREAD_DIMENSIONS, None, false),
        Err(E_INVALIDARG)
    );
    assert_eq!(
        resolve_flagged_groups([45, 1, 1], DISPATCH_USE_THREAD_DIMENSIONS | DISPATCH_CS_W32_EN, None, true),
        Err(E_NOTIMPL)
    );
    assert_eq!(
        resolve_flagged_groups([45, 1, 1], DISPATCH_USE_THREAD_DIMENSIONS, Some([0, 1, 1]), true),
        Err(E_NOTIMPL)
    );
}
