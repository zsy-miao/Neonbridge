"""GPU fixtures for Frosty's Nanite shadow instance culler."""
from pathlib import Path
from tempfile import TemporaryDirectory
from check_frosty_instance_cull import GUARD, SLOTS, f32, load, scene_inputs


def shadow_instances(mode):
    lanes = [2, 33, 60]
    stats = mode != "disabled"
    visible = mode in ("visible", "shadow_cache", "shadow_blocked", "missing_pages")
    views = 2 if mode in ("shadow_cache", "shadow_blocked") else 1
    with TemporaryDirectory() as tmp:
        path = Path(tmp) / "scene"
        scene_inputs(path, lanes, [], instances=64 if mode != "zero" else 0,
                     capacity=SLOTS, stats=stats, image_words=2, view_centers=(0,) * views,
                     hzb_depth=0, hzb_views=range(views))
        cb = load(path / "b0")
        primitive, scene, transforms, packed = (load(path / f"t{i}") for i in range(4))
    # This shader uses primitive flag bit zero to cast a virtual shadow.
    primitive[0] = 0x2001
    primitives = [primitive, scene, transforms]
    frames = [0] * 16384
    count = 0 if mode == "empty_views" else views
    mask = 0 if mode == "missing_pages" else 0xffffffff
    srvs = [*primitives, [f32(1) if mode == "shadow_blocked" else 0] * 64, [mask] * 256, [mask] * 256,
            [0x08000000 if mode in ("shadow_cache", "shadow_blocked") else 0] * 256,
            packed, [value for i in range(views) for value in (i, 1)], [count, 17]]
    if mode not in ("visible", "shadow_cache", "shadow_blocked", "missing_pages"):
        cb[0] = 0 if mode != "empty_views" else 64
    nodes, queue, report = [GUARD] * 400, [0] * 22, [GUARD] * 28
    report[4] = 0
    seed = [nodes, queue, report]
    expected = [a.copy() for a in seed]
    if stats:
        expected[2][12:14] = [count, 17]
    if visible and mask and mode != "shadow_blocked":
        expected[2][4] += len(lanes) * views
        expected[1][4:6] = [len(lanes) * views] * 2
        for view in range(views):
            for j, lane in enumerate(lanes):
                first = 8 + (view * len(lanes) + j) * 2
                expected[0][first:first+2] = [(lane << 3) | 1, view << 16]
    return [cb, frames], srvs, seed, [4, 44, 56], 16, 1, 1, expected, {
        "instances": cb[0], "compacted_views": count, "mode": mode}



def shadow_clusters(mode):
    from check_frosty_persistent_cull import prepare
    candidates = [] if mode in ("empty", "hierarchy") else [2, 33]
    with TemporaryDirectory() as tmp:
        path = Path(tmp) / "clusters"
        _, expected = prepare(path, candidates, outside=[33], children=[2, 5] if mode == "hierarchy" else [],
                              hzb=True, hzb_depth=0)
        cb = load(path / "b0")
        primitive, scene, lookup, page, view, hierarchy = (load(path / f"t{i}") for i in range(6))
        seed = [load(path / f"u{i}") for i in range(6)]
    cb1 = [0] * 16384
    page_mask = 0 if mode == "missing" else 0xffffffff
    srvs = [primitive, scene, lookup, [0] * 64, [page_mask] * 65536, [page_mask] * 65536,
            [0] * 65536, lookup, hierarchy, view, page]
    stats = [0] * 28
    seed.append(stats)
    expected.append(stats.copy())
    return [cb, cb1], srvs, seed, [12,44,4,4,4,4,56], 16, 1, 1, expected, {
        "candidate_clusters": candidates, "mode": mode}
