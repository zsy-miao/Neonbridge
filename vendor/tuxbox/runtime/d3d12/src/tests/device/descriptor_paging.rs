use super::*;

fn test_arena(count: u32, limit: u32) -> Arc<Arena> {
    Arc::new(Arena {
        base_address: 0x1000_0000,
        native: None,
        registry: Arc::default(),
        stride: 32,
        count,
        limit,
        inline: false,
        authored: Mutex::default(),
        tables: Mutex::new(Tables::default()),
    })
}

fn slot_of(arena: &Arena, handle: D3D12_GPU_DESCRIPTOR_HANDLE) -> u32 {
    ((handle.ptr - 0x2000_0000) / arena.stride as u64) as u32
}

fn addr_of(arena: &Arena, logical: u32) -> u64 {
    arena.base() as u64 + u64::from(logical) * arena.stride as u64
}

#[test]
fn free_spans_split_coalesce_and_shrink_used() {
    let mut free = FreeSpans::default();
    let mut used = 10;
    free.free(0, 2, &mut used);
    free.free(4, 3, &mut used);
    assert_eq!(used, 10);
    assert_eq!(free.free_slots(), 5);
    assert_eq!(free.largest_span(), 3);

    // Best-fit prefers the smaller fitting span (2) over (3).
    assert_eq!(free.alloc(2), Some(0));
    assert_eq!(free.free_slots(), 3);
    assert_eq!(free.largest_span(), 3);

    // Split remainder cleanly.
    assert_eq!(free.alloc(1), Some(4));
    assert_eq!(free.free_slots(), 2);
    assert_eq!(free.largest_span(), 2);

    // Freeing [2..4) coalesces [0..4), and freeing [7..10) merges left with [5..7)
    // to reach used=10 and shrink used down to 5!
    free.free(0, 2, &mut used);
    free.free(2, 2, &mut used);
    assert_eq!(free.free_slots(), 6);
    assert_eq!(free.largest_span(), 4);
    free.free(7, 3, &mut used);
    assert_eq!(used, 5);
    assert_eq!(free.free_slots(), 4);
    assert_eq!(free.largest_span(), 4);

    free.free(4, 1, &mut used); // merges [0..4) + [4..5) == used -> shrinks to 0!
    assert_eq!(used, 0);
    assert_eq!(free.free_slots(), 0);
    assert_eq!(free.largest_span(), 0);
}

#[test]
fn pin_blocks_eviction_and_out_of_order_multi_queue_fences_retire() {
    let arena = test_arena(100, 8);
    let direct_fence = Arc::new(AtomicU64::new(0));
    let compute_fence = Arc::new(AtomicU64::new(0));

    // Recording 1 on DIRECT uses slots [0..4)
    let mut l1 = DescriptorLeases::default();
    let h1 = arena.gpu_table(addr_of(&arena, 0), 4, &[], &mut l1).unwrap();
    assert_eq!(slot_of(&arena, h1), 0);
    let mut p1 = PendingSubmissions::new();
    p1.pin_from(&l1).unwrap();
    p1.commit(RetireFence::Mock(direct_fence.clone()), 10);
    l1.clear(); // Recording reset while DIRECT GPU is still in flight!

    // Recording 2 on COMPUTE uses slots [4..8)
    let mut l2 = DescriptorLeases::default();
    let h2 = arena.gpu_table(addr_of(&arena, 10), 4, &[], &mut l2).unwrap();
    assert_eq!(slot_of(&arena, h2), 4);
    let mut p2 = PendingSubmissions::new();
    p2.pin_from(&l2).unwrap();
    p2.commit(RetireFence::Mock(compute_fence.clone()), 1);
    l2.clear();

    // Both batches are in-flight: heap full, eviction blocked by pins.
    let mut l3 = DescriptorLeases::default();
    assert_eq!(
        arena.gpu_table(addr_of(&arena, 20), 4, &[], &mut l3).unwrap_err().code(),
        E_OUTOFMEMORY
    );

    // COMPUTE completes BEFORE DIRECT (out of order! DIRECT is still at 0).
    compute_fence.store(1, Ordering::Release);
    let h3 = arena.gpu_table(addr_of(&arena, 20), 4, &[], &mut l3).unwrap();
    assert_eq!(slot_of(&arena, h3), 4);

    // DIRECT's slots [0..4) are still pinned, so asking for 4 more fails.
    assert_eq!(
        arena.gpu_table(addr_of(&arena, 30), 4, &[], &mut l3).unwrap_err().code(),
        E_OUTOFMEMORY
    );

    // Now DIRECT completes too.
    direct_fence.store(10, Ordering::Release);
    let h4 = arena.gpu_table(addr_of(&arena, 30), 4, &[], &mut l3).unwrap();
    assert_eq!(slot_of(&arena, h4), 0);
}

#[test]
fn device_removed_sentinel_never_retires_pins() {
    let arena = test_arena(64, 4);
    let fence = Arc::new(AtomicU64::new(0));
    let mut leases = DescriptorLeases::default();
    arena.gpu_table(addr_of(&arena, 0), 4, &[], &mut leases).unwrap();
    let mut pending = PendingSubmissions::new();
    pending.pin_from(&leases).unwrap();
    pending.commit(RetireFence::Mock(fence.clone()), 1);
    leases.clear();

    // u64::MAX indicates device removed — must NOT be treated as completed.
    fence.store(u64::MAX, Ordering::Release);
    let mut next = DescriptorLeases::default();
    assert_eq!(
        arena.gpu_table(addr_of(&arena, 10), 4, &[], &mut next).unwrap_err().code(),
        E_OUTOFMEMORY
    );
}

#[test]
fn execute_rollback_and_clear_state_pin_retention() {
    let arena = test_arena(64, 4);
    let mut leases = DescriptorLeases::default();
    arena.gpu_table(addr_of(&arena, 0), 4, &[], &mut leases).unwrap();

    // Simulate ExecuteCommandLists failing validation / placement::submit before commit.
    {
        let mut pending = PendingSubmissions::new();
        pending.pin_from(&leases).unwrap();
        let tables = arena.tables.lock().unwrap();
        assert_eq!(tables.ranges.get(&(0, 4)).unwrap().pins, 2);
        // drop(pending) simulates Err unwind before commit!
    }
    {
        let tables = arena.tables.lock().unwrap();
        assert_eq!(tables.ranges.get(&(0, 4)).unwrap().pins, 1);
    }

    // Simulate ClearState NOT clearing leases: table 0 stays pinned even after
    // bindings reset, so binding table 1 (size 4) cannot evict table 0 while list is open.
    let mut clear_state_survivor = leases;
    assert_eq!(
        arena
            .gpu_table(addr_of(&arena, 10), 4, &[], &mut clear_state_survivor)
            .unwrap_err()
            .code(),
        E_OUTOFMEMORY
    );

    // Only Reset() / Drop clears the lease so a new recording can reclaim [0..4).
    clear_state_survivor.clear();
    let mut after_reset = DescriptorLeases::default();
    let h = arena
        .gpu_table(addr_of(&arena, 10), 4, &[], &mut after_reset)
        .unwrap();
    assert_eq!(slot_of(&arena, h), 0);
}

#[test]
fn direct_pinned_index_map_slots_are_never_evicted_while_unpinned_evicts() {
    let arena = test_arena(64, 8);
    let mut leases = DescriptorLeases::default();
    // Table A at [0..2) and unpinned Table B at [10..12).
    arena.gpu_table(addr_of(&arena, 0), 2, &[], &mut leases).unwrap();
    leases.clear(); // unpinned now, A has old tick, B will be newer
    arena.gpu_table(addr_of(&arena, 10), 2, &[], &mut leases).unwrap();
    leases.clear();

    // Mark Table A as direct_pinned (simulating index_map activation adopting A)
    // and mark direct.written / resident behavior.
    {
        let mut tables = arena.tables.lock().unwrap();
        tables.ranges.get_mut(&(0, 2)).unwrap().direct_pinned = true;
    }

    // Request 6 slots: must evict unpinned B [2..4) and use remaining [4..8) -> wait:
    // used=4, limit=8 -> 4 free tail slots; evicting B frees [2..4), shrinking used? No, [0..2) is direct_pinned,
    // freeing [2..4) shrinks used from 4 -> 2, so bump used 2..8 gives 6 contiguous slots!
    let h = arena.gpu_table(addr_of(&arena, 20), 6, &[], &mut leases).unwrap();
    assert_eq!(slot_of(&arena, h), 2);
    {
        let tables = arena.tables.lock().unwrap();
        assert!(tables.ranges.contains_key(&(0, 2)));
        assert!(!tables.ranges.contains_key(&(10, 2)));
    }

    // Asking for 1 more slot must NOT evict direct_pinned Table A (even though A.pins == 0).
    assert_eq!(
        arena.gpu_table(addr_of(&arena, 30), 1, &[], &mut leases).unwrap_err().code(),
        E_OUTOFMEMORY
    );
}

#[test]
fn sustained_turnover_exceeding_limit_and_pinned_oom() {
    let arena = test_arena(20_000, 64);
    let fence = Arc::new(AtomicU64::new(0));
    let mut prev_fence = 0u64;
    // Double-buffered batches churning 16 slots per submission across 1,000 submissions = 16,000 slots >> limit=64!
    for step in 1..=1_000u64 {
        let mut leases = DescriptorLeases::default();
        let base = ((step as u32 - 1) * 16) % 19_000;
        arena.gpu_table(addr_of(&arena, base), 16, &[], &mut leases).unwrap();
        let mut pending = PendingSubmissions::new();
        pending.pin_from(&leases).unwrap();
        pending.commit(RetireFence::Mock(fence.clone()), step);
        leases.clear();
        if step >= 2 {
            prev_fence += 1;
            fence.store(prev_fence, Ordering::Release);
        }
    }
    fence.store(1_000, Ordering::Release);

    // Genuine pinned > limit returns E_OUTOFMEMORY cleanly.
    let mut open = DescriptorLeases::default();
    for i in 0..4 {
        arena.gpu_table(addr_of(&arena, i * 16), 16, &[], &mut open).unwrap();
    }
    assert_eq!(
        arena.gpu_table(addr_of(&arena, 64), 1, &[], &mut open).unwrap_err().code(),
        E_OUTOFMEMORY
    );
    // in_flight should never leak across steady-state turnover.
    assert!(arena.tables.lock().unwrap().in_flight.is_empty());
}

#[test]
fn leases_deduplicate_within_list_and_across_bundle_adoption() {
    let arena = test_arena(64, 8);
    let mut list_leases = DescriptorLeases::default();
    let h1 = arena.gpu_table(addr_of(&arena, 0), 4, &[], &mut list_leases).unwrap();
    let h2 = arena.gpu_table(addr_of(&arena, 0), 4, &[], &mut list_leases).unwrap();
    assert_eq!(h1.ptr, h2.ptr);
    assert_eq!(arena.tables.lock().unwrap().ranges.get(&(0, 4)).unwrap().pins, 1);

    let mut bundle_leases = DescriptorLeases::default();
    arena.gpu_table(addr_of(&arena, 0), 4, &[], &mut bundle_leases).unwrap();
    arena.gpu_table(addr_of(&arena, 8), 4, &[], &mut bundle_leases).unwrap();
    assert_eq!(arena.tables.lock().unwrap().ranges.get(&(0, 4)).unwrap().pins, 2);

    list_leases.extend_from(&bundle_leases).unwrap();
    // (0, 4) already leased by list_leases -> still 2; (8, 4) newly adopted -> 2.
    assert_eq!(arena.tables.lock().unwrap().ranges.get(&(0, 4)).unwrap().pins, 2);
    assert_eq!(arena.tables.lock().unwrap().ranges.get(&(8, 4)).unwrap().pins, 2);

    bundle_leases.clear();
    assert_eq!(arena.tables.lock().unwrap().ranges.get(&(0, 4)).unwrap().pins, 1);
    assert_eq!(arena.tables.lock().unwrap().ranges.get(&(8, 4)).unwrap().pins, 1);
}

#[test]
fn inline_stride_mismatch_eviction_cleans_only_evicted_resident_records() {
    let arena = Arc::new(Arena {
        base_address: 0x1000_0000,
        native: None,
        registry: Arc::default(),
        stride: 64,
        count: 64,
        limit: 6,
        inline: true,
        authored: Mutex::default(),
        tables: Mutex::new(Tables::default()),
    });
    let mut leases = DescriptorLeases::default();
    arena.gpu_table(addr_of(&arena, 0), 3, &[], &mut leases).unwrap();
    leases.clear(); // slot 0..3, unpinned
    arena.gpu_table(addr_of(&arena, 10), 3, &[], &mut leases).unwrap();
    // Keep second table (slot 3..6) pinned, and populate synthetic resident_records.
    {
        let mut tables = arena.tables.lock().unwrap();
        let rec = InlineDescriptor { source: 1, version: 1, reserved: [0; 2] };
        for slot in 0..6 {
            tables.resident_records.insert(slot, rec);
        }
    }
    // Allocate third table of size 3: evicts first table (slot 0..3), cleans 0..3 only!
    arena.gpu_table(addr_of(&arena, 20), 3, &[], &mut leases).unwrap();
    let tables = arena.tables.lock().unwrap();
    for slot in 0..3 {
        assert!(!tables.resident_records.contains_key(&slot));
    }
    for slot in 3..6 {
        assert!(tables.resident_records.contains_key(&slot));
    }
}

#[test]
fn index_map_activation_and_publish_reclaim_unpinned_and_protect_direct_pinned() {
    let arena = test_arena(32, 4);
    let mut leases = DescriptorLeases::default();
    // Table A (0..2) -> slot 0..2, unpinned
    arena.gpu_table(addr_of(&arena, 0), 2, &[], &mut leases).unwrap();
    leases.clear();
    // Redundant alias (0..1) -> slot 2..3, unpinned (contributes 0 slots when 0..2 adopts first!)
    arena.gpu_table(addr_of(&arena, 0), 1, &[], &mut leases).unwrap();
    leases.clear();

    // Pre-activation direct writes on logical indices 4..6 (2 missing slots; used=3, limit=4 -> needs eviction of redundant alias!).
    arena.publish(addr_of(&arena, 4) as usize, 2);
    {
        let mut tables = arena.tables.lock().unwrap();
        let mut retired = RetiredViews::default();
        index_map::activate_for_test(&arena, &mut tables, &mut retired).unwrap();
        assert!(tables.ranges.get(&(0, 2)).unwrap().direct_pinned);
        assert!(!tables.ranges.contains_key(&(0, 1)));
        // Table A (slot 0..2) stays untouched; direct indices 4..6 took slots 2 and 3.
        assert_eq!(tables.direct.contiguous(0, 2), Some(0));
        assert!(tables.direct.contiguous(4, 1).is_some());
        assert!(tables.direct.contiguous(5, 1).is_some());
    }

    // Now free one direct-compatible alias after activation (e.g. a limit=6 arena) to test post-activation publish eviction.
    let arena2 = test_arena(32, 6);
    let mut leases2 = DescriptorLeases::default();
    arena2.gpu_table(addr_of(&arena2, 0), 2, &[], &mut leases2).unwrap();
    leases2.clear();
    arena2.publish(addr_of(&arena2, 4) as usize, 2);
    {
        let mut tables = arena2.tables.lock().unwrap();
        let mut retired = RetiredViews::default();
        index_map::activate_for_test(&arena2, &mut tables, &mut retired).unwrap();
    }
    // Allocate an unpinned non-contiguous root table (0..3? no, 10..12) after activation using the remaining 2 slots [4..6).
    arena2.gpu_table(addr_of(&arena2, 10), 2, &[], &mut leases2).unwrap();
    leases2.clear();

    // Publish a new direct descriptor (logical 6..7) when heap is full (used=6/6):
    // must evict unpinned (10, 2) and reuse its slot while leaving direct_pinned (0, 2) intact!
    arena2.publish(addr_of(&arena2, 6) as usize, 1);
    let tables = arena2.tables.lock().unwrap();
    assert!(!tables.ranges.contains_key(&(10, 2)));
    assert!(tables.direct.contiguous(6, 1).is_some());
    assert!(tables.ranges.get(&(0, 2)).unwrap().direct_pinned);
}
