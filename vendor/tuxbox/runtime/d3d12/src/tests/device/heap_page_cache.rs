use super::*;
#[test]
fn span_keys_identify_exact_page_ranges() {
    let base = 0x1_2345_0000;
    let key = span_key(base, 17 * PAGE).unwrap();
    assert_eq!(span_key(base, 17 * PAGE), Some(key));
    // Another length or another start is another import.
    assert_ne!(span_key(base, 16 * PAGE), Some(key));
    assert_ne!(span_key(base + PAGE, 17 * PAGE), Some(key));
    // Neighbouring ranges that overlap must not collide either.
    assert_ne!(span_key(base, 2 * PAGE), span_key(base + PAGE, 2 * PAGE));
    // No span shares the key of a single page, which is its base.
    for pages in 2..64 {
        let key = span_key(base, pages * PAGE).unwrap();
        assert!(!key.is_multiple_of(PAGE));
        assert_eq!(key & !(PAGE - 1), base);
    }
}

#[test]
fn span_keys_reject_ranges_the_key_cannot_describe() {
    let base = 0x1_2345_0000;
    // Single pages have their own caches, keyed by base.
    assert_eq!(span_key(base, PAGE), None);
    assert_eq!(span_key(base, 0), None);
    // Partial pages and unaligned bases are never shared.
    assert_eq!(span_key(base, 2 * PAGE + 1), None);
    assert_eq!(span_key(base + 4096, 2 * PAGE), None);
    // The count must fit below the page bits: 4 GiB and above is not cached.
    assert!(span_key(base, (PAGE - 1) * PAGE).is_some());
    assert_eq!(span_key(base, PAGE * PAGE), None);
}

#[test]
fn spans_and_pages_at_one_base_keep_separate_imports() {
    let mut cache = PageCache::default();
    let base = 0x7000_0000;
    let page = cache.get_or_open(base, |_| true, || Ok::<_, ()>(Arc::new(1))).unwrap();
    let short = cache.get_or_open(span_key(base, 2 * PAGE).unwrap(), |_| true, || Ok::<_, ()>(Arc::new(2))).unwrap();
    let long = cache.get_or_open(span_key(base, 17 * PAGE).unwrap(), |_| true, || Ok::<_, ()>(Arc::new(17))).unwrap();
    assert_eq!((*page, *short, *long), (1, 2, 17));
    // A repeated view of the same range gets the live import back.
    let again = cache.get_or_open(span_key(base, 17 * PAGE).unwrap(), |_| true, || Err(())).unwrap();
    assert!(Arc::ptr_eq(&long, &again));
    // Once every holder is gone the range is imported afresh.
    drop(long); drop(again);
    assert_eq!(*cache.get_or_open(span_key(base, 17 * PAGE).unwrap(), |_| true, || Ok::<_, ()>(Arc::new(18))).unwrap(), 18);
}

#[test]
fn interleaved_live_pages_reuse_exact_heap_and_expired_pages_reopen() {
    let mut cache = PageCache::default();
    let a = cache.get_or_open(0, |_| true, || Ok::<_, ()>(Arc::new(7))).unwrap();
    let b = cache.get_or_open(65536, |_| true, || Ok::<_, ()>(Arc::new(9))).unwrap();
    let same = cache.get_or_open(0, |_| true, || Err(())).unwrap();
    assert!(Arc::ptr_eq(&a, &same));
    assert!(!Arc::ptr_eq(&b, &same));
    drop(a); drop(same);
    assert_eq!(*cache.get_or_open(0, |_| true, || Ok::<_, ()>(Arc::new(11))).unwrap(), 11);
}

#[test]
fn rejected_live_entry_is_replaced_by_a_new_import() {
    let mut cache = PageCache::default();
    let old = cache.get_or_open(0, |_| true, || Ok::<_, ()>(Arc::new(1))).unwrap();
    let new = cache.get_or_open(0, |value| *value != 1, || Ok::<_, ()>(Arc::new(2))).unwrap();
    assert_eq!(*new, 2);
    assert!(!Arc::ptr_eq(&old, &new));
    let again = cache.get_or_open(0, |value| *value != 1, || Err(())).unwrap();
    assert!(Arc::ptr_eq(&new, &again));
}

#[test]
fn full_cache_replaces_same_key_without_duplicate_queue_entries() {
    let mut cache = PageCache::default();
    let live: Vec<_> = (0..LIMIT).map(|n| cache.get_or_open(n as u64, |_| true, || Ok::<_, ()>(Arc::new(n))).unwrap()).collect();
    let replacement = cache.get_or_open(0, |_| false, || Ok::<_, ()>(Arc::new(99999))).unwrap();
    assert!(!Arc::ptr_eq(&live[0], &replacement));
    let hit = cache.get_or_open(0, |v| *v == 99999, || Err(())).unwrap();
    assert!(Arc::ptr_eq(&replacement, &hit));
    assert_eq!(cache.entries.len(), LIMIT);
    assert_eq!(cache.sweep.len(), LIMIT);
}

#[test]
fn sweep_reaches_expired_key_behind_live_entries() {
    let mut cache = PageCache::default();
    let mut live: Vec<_> = (0..LIMIT).map(|n| Some(cache.get_or_open(n as u64, |_| true, || Ok::<_, ()>(Arc::new(n))).unwrap())).collect();
    live[LIMIT - 1] = None;
    let mut found = None;
    for n in 0..LIMIT.div_ceil(SWEEP) {
        let base = 100000 + n as u64;
        let value = cache.get_or_open(base, |_| true, || Ok::<_, ()>(Arc::new(n))).unwrap();
        if cache.entries.contains_key(&base) {
            found = Some((base,value));
            break;
        }
    }
    let (base, value) = found.expect("expired key must be reclaimed within one sweep");
    assert!(Arc::ptr_eq(&value, &cache.get_or_open(base, |_| true, || Err(())).unwrap()));
    assert_eq!(cache.entries.len(), LIMIT);
    assert_eq!(cache.sweep.len(), LIMIT);
}

#[test]
fn failed_imports_are_not_cached_and_live_entries_are_bounded() {
    let mut cache = PageCache::default();
    assert!(cache.get_or_open(0, |_| true, || Err::<Arc<u32>, _>(())).is_err());
    assert!(cache.entries.is_empty());
    let live: Vec<_> = (0..=LIMIT).map(|n| cache.get_or_open(n as u64, |_| true, || Ok::<_, ()>(Arc::new(n as u32))).unwrap()).collect();
    assert_eq!(cache.entries.len(), LIMIT);
    drop(live);
    cache.get_or_open(99999, |_| true, || Ok::<_, ()>(Arc::new(1))).unwrap();
    assert_eq!(cache.entries.len(), LIMIT - SWEEP + 1);
    assert_eq!(cache.sweep.len(), cache.entries.len());
}
