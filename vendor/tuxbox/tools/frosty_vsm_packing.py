"""Build a CPU reference for the packed 800-byte virtual shadow-map views."""

GUARD = 0xdeadbeef


def compact_views(view_count, active_group):
    # Each input view owns up to four mip records. The GPU takes one primary
    # view per lane and reads its mip records at strides of view_count.
    levels, record_words = 4, 200
    stride = max(1, view_count)
    source = [0x44000000 + i for i in range((stride * levels + 1) * record_words)]
    rects = [0] * (stride * 8 * 4)
    selected = []
    for view in range(stride):
        mips = 1 + view % levels
        if view % 64 == 17:
            mips = 0
        source[view * record_words + 194] = mips
        valid_records = []
        for mip in range(levels):
            record = view + mip * stride
            pos = record * record_words
            source[pos + 192:pos + 194] = [view, mip]
            slot = 4 * (view * 8 + mip)
            valid = (view < view_count and view // 64 == active_group and
                     view % 64 in (0, 2, 7, 17, 18, 31, 32, 46, 61, 63) and (mip + view) % 4 != 2)
            # Equal min/max is a valid rectangle. The large unsigned minimum
            # also distinguishes the native unsigned compare from a signed one.
            rects[slot:slot + 4] = ([0x80000001, 7, 0x80000001, 9] if valid else
                                    [0x80000001, 7, 3, 9] if mip % 2 else [0, 7, 9, 3])
            if valid and mip < mips:
                valid_records.append(record)
        if valid_records:
            selected.append(valid_records)

    seed = [[GUARD] * (48 * record_words), [GUARD] * 50, [2, 3, GUARD, GUARD]]
    output = [s.copy() for s in seed]
    view_base, record_base = seed[2][:2]
    for group in selected:
        output[1][2 * view_base:2 * view_base + 2] = [record_base, len(group)]
        for record in group:
            dst, src = record_base * record_words, record * record_words
            output[0][dst:dst + record_words] = source[src:src + record_words]
            record_base += 1
        view_base += 1
    output[2][:2] = [view_base, record_base]
    cb = [0] * 64
    cb[9] = view_count
    return (cb, [rects, source], seed, [800, 8, 4], 16, 2, 1, output,
            {"input_views": view_count, "active_group": active_group, "contributing_views": len(selected),
             "copied_records": record_base - seed[2][1], "record_bytes": record_words * 4})


def free_pages(page_count, initial, mode):
    # Distinguish the two words of each physical page record. Pages outside
    # the range look free so the CPU oracle also checks the dispatch bounds.
    metadata, free = [], []
    for page in range(512):
        available = (page >= page_count or mode == "all" or
                     (mode == "mixed" and ((page % 5 == 2 and page // 64 != 3) or page % 89 == 0)))
        metadata.extend((0 if available else 0x80000000 | page, 0x80000001 if available else 0))
        if available and page < page_count:
            free.append(page)
    cb = [0] * 64
    cb[35] = page_count
    seed = [GUARD] * 528
    seed[page_count] = initial
    expected = seed.copy()
    expected[initial:initial + len(free)] = free
    expected[page_count] += len(free)
    return (cb, metadata, [seed], [4], 8, 2, 1, [expected],
            {"page_count": page_count, "initial": initial, "selected_pages": len(free)})


def unordered_wave_ranges(actual, expected, first, count):
    # A whole wave reserves a single contiguous range, but different waves
    # may reserve their ranges in any order. Normalize whole ranges only.
    by_wave = {}
    for page in expected[first:first + count]:
        by_wave.setdefault(page // 64, []).append(page)
    end = first + count
    cursor = first
    while cursor < end:
        segment = by_wave.pop(actual[cursor] // 64, None)
        if not segment or actual[cursor:cursor + len(segment)] != segment:
            return actual
        cursor += len(segment)
    normalized = actual.copy()
    normalized[first:end] = expected[first:end]
    return normalized
