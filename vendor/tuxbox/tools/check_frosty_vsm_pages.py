#!/usr/bin/env python3
"""Read back Frosty's physical shadow page allocation and initialization shaders."""
import argparse
import json
from pathlib import Path
import struct
import subprocess
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from frosty_shadow_instances import shadow_instances, shadow_clusters
from check_frosty_instance_cull import scene_inputs, f32
from build import Toolchain
from frosty_vsm_packing import compact_views, free_pages, unordered_wave_ranges
from check_frosty_shaders import containers, decoder, decompress, global_archive
from project_tuxbox.host import Host
from project_tuxbox.inspect import digest
from project_tuxbox.shader_prep import compiler_tools

GUARD = 0xdeadbeef


def hair_pages(count, capacity, force_direct):
    # The macro-group bounds cover 16 world units per axis on an 8³ grid.
    # Each cluster box is in whole world units, while macro bounds use thousandths.
    boxes = {0: (2, 2, 2, 3, 3, 3), 3: (1, 3, 1, 5, 5, 3),
             9: (4, 8, 2, 6, 9, 3), 15: (0, 0, 0, 7, 5, 5),
             31: (10, 2, 0, 15, 5, 3)}
    clusters = [0] * (32 * 6)
    for lane, bounds in boxes.items():
        clusters[lane*6:(lane+1)*6] = bounds
    cb = [count, 0, capacity, int(force_direct)] + [0] * 60
    page_base = 3
    inputs = [clusters, [0, 0, 0, 16000, 16000, 16000], [8, 8, 8, page_base]]
    seed = [[0x30] * (page_base + 512 + 3), [2, GUARD], [GUARD] * 128]
    pages, counter, records = (row.copy() for row in seed)
    for lane, bounds in boxes.items():
        if lane >= count:
            continue
        x, y, z, end_x, end_y, end_z = (v // 2 for v in bounds)
        width, height, depth = end_x - x + 1, end_y - y + 1, end_z - z + 1
        volume = width * height * depth
        if volume <= 4 or force_direct:
            for k in range(z, end_z + 1):
                for j in range(y, end_y + 1):
                    for i in range(x, end_x + 1):
                        pages[page_base + i + 8 * (j + 8 * k)] |= 1
        else:
            start = counter[0]
            counter[0] += (volume + 7) // 8
            if counter[0] < capacity:
                for tile, dst in enumerate(range(start, counter[0])):
                    records[2*dst] = x | y << 8 | z << 16 | tile << 24
                    records[2*dst + 1] = width | height << 8 | depth << 16
    return cb, inputs, seed, [4, 4, 8], 4, 2, 1, [pages, counter, records], {
        "clusters": count, "capacity": capacity, "force_direct": force_direct,
        "marked_pages": pages.count(0x31), "counter": counter[0],
    }


def compact_feedback(active, count, capacity, valid_limit):
    # One guest wave reserves a contiguous range before checking the
    # referenced material index. Invalid indices still consume a slot.
    cb = [0] * 64
    cb[12], cb[6], cb[18] = count, capacity, valid_limit
    records = [word for i in range(64) for word in (0x12340000 + i, 0x40 + i)]
    indices = list(range(64))
    hits = [0] * 64
    for lane in active:
        hits[lane] = 7 + lane
    seed = [[GUARD] * 160]
    seed[0][0] = 2
    expected = [seed[0].copy()]
    selected = [i for i in active if i < count]
    expected[0][0] += len(selected)
    for rank, lane in enumerate(selected):
        dst = 3 + rank
        index = indices[lane]
        if dst < capacity and index < valid_limit:
            expected[0][2 * dst] = records[2 * index]
            expected[0][2 * dst + 1] = (hits[lane] << 16) | records[2 * index + 1]
    return cb, [records, indices, hits], seed, [8], 4, 1, 1, expected, {
        "selected": len(selected), "count": count,
        "capacity": capacity, "valid_limit": valid_limit,
    }


def cull_voxel(active, count, small):
    cb = [0] * 64
    cb[8] = count
    cb[19] = 0x3f000000  # Minimum radius 0.5.
    source = [0] * (128 * 4)
    for lane in range(64):
        center = 0 if lane in active else 0x42c80000  # 0.0 or 100.0.
        extent = 0 if lane in small else 0x3f800000  # 0.0 or 1.0.
        source[8 * lane:8 * lane + 3] = [center] * 3
        source[8 * lane + 4:8 * lane + 7] = [extent] * 3
    selected = [lane for lane in active if lane < count and lane not in small]
    seed = [[5, GUARD], [GUARD] * 80]
    expected = [row.copy() for row in seed]
    expected[0][0] += len(selected)
    expected[1][5:5 + len(selected)] = selected
    return cb, source, seed, [4, 4], 16, 1, 1, expected, {
        "count": count, "selected": len(selected),
    }


def command_space(counts):
    cb = [0] * 64
    cb[0] = len(counts)
    source = [0] * 320
    for index, count in enumerate(counts):
        source[5 * index + 1] = count
    seed = [[GUARD] * 80 for _ in range(3)]
    seed[1][0] = 13
    expected = [array.copy() for array in seed]
    offset = 13
    for index, count in enumerate(counts):
        expected[0][index] = expected[2][index] = offset if count else 0
        offset += count
    expected[1][0] = offset
    return cb, source, seed, [4, 4, 4], 4, 1, 1, expected, {
        "commands": len(counts), "instances": sum(counts),
    }


def card_update(count):
    constants = [0] * 64
    constants[24] = 0x3f800000
    constants[26] = count
    sources = [[0] * 512 for _ in range(4)]
    seed = [[0] * 4096 for _ in range(6)]
    for card in range(count):
        # Each card occupies five 16-byte records. These fields give it
        # two units of work, which the shader adds to two statistics counters.
        seed[0][card * 20 + 2] = 0x43000000
        seed[0][card * 20 + 3] = 0x3f800000
    expected = [data.copy() for data in seed]
    expected[5][1] = expected[5][3] = 2 * count
    return ([constants, [0] * 272], sources, seed, [16, 4, 4, 4, 4, 4],
            4, 1, 1, expected, {"cards": count, "expected_work": 2 * count})


def words(path, data):
    path.write_bytes(struct.pack(f"<{len(data)}I", *data))


def allocation(entry, count, layer):
    # All requested lanes belong to a single guest wave. Its pop order is
    # deterministic even if the host uses smaller physical subgroups.
    offset = layer * 21845
    size = offset + 260
    requests, flags, mapping = [0] * size, [0] * size, [GUARD] * size
    physical = [GUARD] * 160
    free = [(7 * i + 3) % 73 for i in range(32)] + [GUARD] * 16
    free[37] = count
    active = [2, 9, 31, 32, 35, 56, 63]
    for i in [*active, 47]:
        requests[offset+i] = 8 if i & 1 else 1
    flags[offset+47] = 0x1234
    cb = [0] * 64
    cb[35], cb[38], cb[39] = 37, 7, 3
    seed = [flags, mapping, physical, free]
    if entry == 1235:
        seed.insert(0, [13, GUARD])
    expected = [s.copy() for s in seed]
    mapped, table, meta, pool = expected[-4:]
    pool[37] = (count - len(active)) & 0xffffffff
    if entry == 1235:
        expected[0][0] += len(active)
    for j, lane in enumerate(active[:count]):
        pid = free[count-j-1]
        f = 7 | (requests[offset+lane] & 8)
        mapped[offset+lane] = meta[2*pid] = f
        table[offset+lane] = 0x08000000 | ((pid >> 3) << 10) | (pid & 7)
    return (cb, requests, seed, [4, 4, 8, 4] if entry == 1224 else [4, 4, 4, 8, 4],
            4, 1, layer+1, expected, {"requested": len(active), "available": count, "allocated": min(count, len(active)), "layer": layer})


def selection(entry, count, extra):
    # The second metadata word is deliberately unrelated to its page flags.
    active_flags = {2: 3, 9: 5, 31: 7, 32: 3, 35: 1, 47: 6, 56: 5, 63: 7, 64: 5}
    metadata = [word for i in range(256) for word in (active_flags.get(i, 0), 7)]
    cb = [0] * 64
    cb[35:38] = [count, int(extra), 256]
    hzb = entry == 1215
    merge = entry in (1219, 1230)
    stats = {1219: 12, 1236: 13}.get(entry)
    base = [i for i in range(count) if active_flags.get(i, 0) & 1 and (hzb or active_flags[i] & 6)]
    second = [256+i for i in base if extra and not merge and (hzb or active_flags[i] & 4)]
    indirect = [32, GUARD, GUARD, GUARD, 17, GUARD] if hzb else [32, GUARD, GUARD, GUARD]
    seed = [indirect, [GUARD] * 20]
    if stats is not None:
        seed.insert(0, [GUARD] * stats + [17, GUARD])
    out = [a.copy() for a in seed]
    selected = base + second
    out[-2][0] += 16 * len(selected)
    if hzb:
        out[-2][4] += len(selected)
    out[-1][2:2+len(selected)] = selected
    if stats is not None:
        out[0][stats] += len(selected)
    return cb, metadata, seed, [4] * len(seed), 8, 1, 1, out, {
        "pages": count, "selected": len(base), "additional_pages": len(second), "initial_indirect_x": 32}


def hair_mesh(count, points, groups, irregular=False):
    import math
    def bits(value): return struct.unpack("<I", struct.pack("<f", value))[0]
    cb = [0] * 64
    cb[0:2] = [count, points]
    centres = [bits(float(i % 2)) for i in range(max(1, points)) for _ in range(3)]
    weights = [[(i + 1) / 16, (i + 2) / 16, (i + 3) / 16] for i in range(points + 4)]
    vectors = [([1., 0., 0.] if irregular else [1., 1., 1.]) if i % 2 else [0., 0., 0.] for i in range(128 * groups)]
    seed = [[GUARD] * (4 * 128 * groups)]
    expected = [seed[0].copy()]
    for lane in range(min(count, len(vectors))):
        point = vectors[lane]
        result = [point[k] + sum(weights[i][k] * math.sqrt(1. + sum((point[j] - float(i % 2)) ** 2 for j in range(3))) for i in range(points)) + weights[points][k]
                  + sum(weights[points + j + 1][k] * point[j] for j in range(3)) for k in range(3)]
        expected[0][lane * 4:lane * 4 + 4] = [bits(v) for v in (*result, 1.)]
    return cb, [centres, [bits(v) for w in weights for v in w], [bits(v) for p in vectors for v in p]], seed, [16], 12, groups, 1, expected, {"count": count, "control_points": points, "groups": groups, "irregular": irregular}


def visualize_tiles(gx, gy, offset, extent):
    import math
    def cvt(v):
        if math.isnan(v) or v <= 0: return 0
        if v >= 4294967296: return 0xffffffff
        return int(v)
    def f32(v): return struct.unpack("<f", struct.pack("<f", v))[0]
    def bits(v): return struct.unpack("<I", struct.pack("<f", v))[0]
    cb = [0] * 576
    cb[532:534] = [bits(v) for v in offset]
    cb[536:538] = [bits(v) for v in extent]
    selected = []
    for y in range(gy):
        for x in range(gx):
            if any(cvt(f32(offset[0] + (x * 8 + dx))) < cvt(extent[0]) and
                   cvt(f32(offset[1] + (y * 8 + dy))) < cvt(extent[1])
                   for dy in range(8) for dx in range(8)):
                selected.append((x & 0xffff) | (y << 16))
    seed = [[3] + [GUARD] * 15, [GUARD] * 160]
    expected = [row.copy() for row in seed]
    expected[0][0] += len(selected)
    expected[1][3:3 + len(selected)] = sorted(selected)
    return cb, [0] * 4, seed, [4, 4], 4, gx, gy, expected, {"selected": selected}


def mesh_sdf_offsets(count, groups, dispatch_count, zero=False, wrap=False):
    cb = [0] * 64
    cb[10] = count
    values = [0 if zero else (0xfffffff0 + lane % 8 if wrap else (lane * 7 + 3) % 9) for lane in range(64 * groups)]
    seed = [[GUARD] * 160, [11] + [GUARD] * 15, [GUARD] * 16]
    expected = [row.copy() for row in seed]
    expected[2][:3] = [((dispatch_count + 63) & 0xffffffff) >> 6, 1, 1]
    sizes = [sum(values[g * 64:min((g + 1) * 64, count)]) for g in range(groups)]
    def offsets(order):
        result = seed[0].copy()
        base = 11
        for group in order:
            running = base
            for lane in range(group * 64, min((group + 1) * 64, count)):
                result[lane] = running
                running = (running + values[lane]) & 0xffffffff
            base = (base + sizes[group]) & 0xffffffff
        return result
    expected[0] = offsets(range(groups))
    expected[1][0] = (expected[1][0] + sum(sizes)) & 0xffffffff
    other = offsets(reversed(range(groups))) if groups == 2 else []
    return cb, [[dispatch_count] * 64, values], seed, [4, 4, 4], 4, groups, 1, expected, {
        "count": count, "groups": groups, "dispatch_count": dispatch_count,
        "alternate_offsets": other}


def compact_visualize_rays(entry, mode, groups):
    mask, flag = {248: (1, 1), 249: (1, 0), 259: (3, 1)}[entry]
    count = 64 * groups
    values = [0x12340000 + lane for lane in range(count)]
    def bits(lane):
        if mode == "all": return flag
        if mode == "none": return flag ^ 1
        return lane % 4
    records = [[0xabc00000 | bits(lane), 0x24680000 + lane] for lane in range(count)]
    selected = [lane for lane in range(count) if (records[lane][0] & mask) == flag]
    seed = [[3] + [GUARD] * 15, [GUARD] * 150, [GUARD] * 300]
    expected = [row.copy() for row in seed]
    expected[0][0] += len(selected)
    for index, lane in enumerate(selected, 3):
        expected[1][index] = values[lane]
        expected[2][2 * index:2 * index + 2] = records[lane]
    return [0] * 64, [values, [v for record in records for v in record]], seed, [4, 4, 8], 4, groups, 1, expected, {
        "selected": selected, "mode": mode, "groups": groups}


def compact_hwrt_rays(entry, count, sparse, groups=1):
    counts = [count if not sparse or lane in (0, 2, 31, 32, 33, 63) else 0 for lane in range(64)]
    records = [[0x12340000 + lane, 0x43210000 + lane, 0xabc00000 | (lane % 4),
                0x24680000 + lane, 0x13570000 + lane] for lane in range(64 * groups)]
    mask, flag = {507: (1, 1), 509: (0, 0), 512: (1, 0), 513: (3, 1)}[entry]
    selected = [lane for lane in range(64 * groups) if lane < counts[lane % 64] and (records[lane][2] & mask) == flag]
    seed = [[3] + [GUARD] * 15, [GUARD] * (5 * 150)]
    expected = [row.copy() for row in seed]
    expected[0][0] += len(selected)
    for index, lane in enumerate(selected, 3):
        expected[1][5 * index:5 * index + 5] = records[lane]
    return [0] * 64, [counts, [v for record in records for v in record]], seed, [4, 20], 4, groups, 1, expected, {
        "selected": selected, "count": count, "sparse_counts": sparse, "groups": groups}


def distance_field_requests(active, count, capacity, start):
    cb = [0] * 64
    cb[44:46] = [count, capacity]
    assets, requests = [0] * (64 * 9), [0] * 64
    for lane in range(64):
        prior = 1 + lane % 3
        assets[lane * 9] = prior << 30
        requests[lane] = (prior % 3) + 1 if lane in active else prior
    # Reject a zero resident mip and a zero requested mip independently.
    assets[5 * 9] = 0
    requests[5] = 2
    requests[6] = 0
    seed = [[GUARD] * 270]
    seed[0][0] = start
    expected = [seed[0].copy()]
    chosen = [lane for lane in range(min(count, 64)) if lane in active and lane not in (5, 6)]
    expected[0][0] += len(chosen)
    for offset, lane in enumerate(chosen):
        index = start + offset
        if index < capacity:
            expected[0][2 * index + 1:2 * index + 3] = [lane, requests[lane]]
    return cb, [assets, requests], seed, [4], 4, 1, 1, expected, {
        "selected": chosen, "count": count, "capacity": capacity, "initial": start}


def visibility_instance(visible, occluders, protected=None, post=False, stats_variant=False, no_hzb=False):
    # Exercise the packaged shader with an actual depth texture and visibility UAVs.
    with TemporaryDirectory(dir=ROOT / "build") as tmp:
        scene = Path(tmp) / "scene"
        scene_inputs(scene, visible, [3], instances=64, capacity=128, stats=True,
                     image_words=1, view_centers=(0,), hzb_depth=0, hzb_views=(0,),
                     occluders=occluders, protected_depth=protected)
        def read(name):
            data = (scene / name).read_bytes()
            return list(struct.unpack(f"<{len(data) // 4}I", data))
        cb = read("b0")
        srv = [read(name) for name in ("t0", "t1", "t2", "hzb", "t3", "t4")]
        srv[3] = srv[3][:64]
    depth = f32(protected if protected is not None else .25)
    seed = [[0x77] * (9 * 9 * 4), [depth] * 81,
            [0xa5c3e749] * 512, [0] * 64, [0xa5c3e749] * 512, [0] * 22]
    if no_hzb:
        srv = [srv[i] for i in (0, 1, 2, 4, 5)]
        seed = seed[:2] + [[0xa5c3e749] * 512, [0] * 11]
    if post:
        srv += [[word for lane in visible for word in (lane, 0)] or [0, 0],
                [1, 1, 1, len(visible)] + [0xa5c3e749] * 4]
        seed = seed[:2] + [[0xa5c3e749] * 512, [0xa5c3e749] * 11]
        seed[3][9:11] = [3, 29]
    if stats_variant:
        seed += [[0xa5c3e749] * 14]
        seed[-1][8] = 43
    expected = [row.copy() for row in seed]
    if stats_variant and no_hzb and not occluders:
        expected[-1][4] = 0xa5c3e749 + len(visible)
    if stats_variant and post:
        expected[-1][7] = len(visible)
        expected[-1][8] = 43 + (0 if occluders else len(visible))
    if stats_variant and not post and not occluders:
        expected[-1][4] = 0xa5c3e749 + len(visible)
    if post and not occluders:
        start = (32 << 4) // 8 // 4 + 128 * 2 + 3 * 3
        for i, lane in enumerate(visible):
            expected[2][start + i * 3:start + i * 3 + 3] = [(lane << 3) | 5, 0, 255]
        expected[3][9:11] = [3 + len(visible), 29 + len(visible)]
    elif no_hzb and not occluders:
        expected[2][8:16] = [9, 0, 249, 0, 257, 0, 505, 0]
        expected[3][4:6] = [4, 4]
    elif not post and not occluders:
        expected[4][8:16] = [9, 0, 249, 0, 257, 0, 505, 0]
        expected[5][4:6] = [4, 4]
    elif protected is None:
        expected[0][160:164] = [0x800000a1 + (occluders[0] << 8), 0, 0, 0]
        expected[1][40] = f32(.75)
    # CopyTextureRegion has 256-byte row alignment; padding is unspecified.
    for i, width in ((0, 36), (1, 9)):
        expected[i] = [v for y in range(9) for v in
                       (expected[i][y * width:(y + 1) * width] + [0] * (64 - width))]
    return cb, srv, seed, (([16, 4, 4, 44] if post or no_hzb else [16, 4, 8, 4, 4, 44]) + ([56] if stats_variant else [])), 16, 1, 1, expected, {
        "visible": visible, "occluders": occluders, "protected": protected}


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("mount", type=Path)
    p.add_argument("--prefix", type=Path)
    p.add_argument("--proton", type=Path)
    p.add_argument("--xshader", type=Path)
    p.add_argument("--entry", type=int, choices=(752, 1865, 257, 728, 248, 249, 259, 507, 509, 512, 513, 517, 1016, 1019, 1213, 1215, 1219, 1223, 1224, 1228, 1230, 1235, 1236, 1243, 1257, 1793, 2138, 3396, 3397, 3398, 3399, 3403, 3412, 3413, 3416, 3426, 3433, 3437, 3440, 3468, 3474, 3422, 3430, 3432, 3438, 3449, 3451, 3454, 3456, 3457, 3458, 3470, 3477),
                   help="run the controls for one archive entry")
    p.add_argument("--output", type=Path, required=True)
    args = p.parse_args()
    host = Host(None, args.proton, args.prefix)
    compiler, dxc, dxv = compiler_tools()
    compiler = args.xshader.resolve() if args.xshader else compiler
    data, entries = global_archive(args.mount)
    cases, libraries = {}, {}
    for name, count, capacity, force_direct in (
            ("mixed", 32, 64, False), ("partial", 14, 64, False),
            ("bounded", 32, 5, False), ("all_direct", 32, 64, True),
            ("empty", 0, 64, False)):
        cases["hair_pages_" + name] = 752, hair_pages(count, capacity, force_direct)
    for name, count, points, groups, irregular in (("empty", 0, 0, 1, False), ("no_points", 128, 0, 1, False),
        ("one", 37, 1, 1, False), ("three", 128, 3, 1, False), ("seven", 128, 7, 1, False),
        ("two_groups", 193, 3, 2, False), ("irrational", 128, 3, 1, True)):
        cases["hair_mesh_" + name] = (1865, hair_mesh(count, points, groups, irregular))
    for name, gx, gy, offset, extent in (
        ("empty", 3, 2, (0., 0.), (0., 0.)),
        ("one", 3, 2, (0., 0.), (1., 1.)),
        ("edge", 3, 2, (0., 0.), (9., 9.)),
        ("full", 3, 2, (0., 0.), (24., 16.)),
        ("offset", 3, 2, (7.75, 3.5), (17., 11.)),
        ("negative", 3, 2, (-3.5, -2.25), (5., 6.)),
        ("nan_offset", 3, 2, (float("nan"), 0.), (5., 6.)),
        ("nan_extent", 3, 2, (0., 0.), (float("nan"), 6.)),
        ("positive_infinity", 3, 2, (float("inf"), 0.), (float("inf"), 6.)),
        ("negative_infinity", 3, 2, (float("-inf"), 0.), (5., 6.)),
    ):
        cases["tiles_" + name] = (257, visualize_tiles(gx, gy, offset, extent))
    for name, count, groups, dispatch, zero, wrap in (
        ("empty", 0, 1, 0, False, False), ("single", 1, 1, 1, False, False),
        ("lower", 32, 1, 64, False, False), ("boundary", 33, 1, 65, False, False),
        ("full", 64, 1, 127, False, False), ("zero_weights", 64, 1, 128, True, False),
        ("two_groups", 128, 2, 129, False, False), ("partial_groups", 97, 2, 64, False, False),
        ("wrapping", 64, 1, 0xffffffff, False, True),
    ):
        cases["mesh_offsets_" + name] = (728, mesh_sdf_offsets(count, groups, dispatch, zero, wrap))
    for entry in (248, 249, 259):
        for mode, groups in (("all", 1), ("none", 1), ("mixed", 1), ("mixed", 2)):
            cases[f"visualize_{entry}_{mode}_{groups}"] = (entry, compact_visualize_rays(entry, mode, groups))
    for entry in (507, 509, 512, 513):
        for name, count, sparse, groups in (("empty", 0, False, 1), ("full", 64, False, 1),
                                    ("partial", 33, False, 1), ("sparse", 64, True, 1),
                                    ("two_groups", 128, False, 2)):
            cases[f"hwrt_{entry}_" + name] = (entry, compact_hwrt_rays(entry, count, sparse, groups))
    for name, active, count, capacity, start in (
        ("empty", [], 64, 100, 3),
        ("zero_count", [0, 2, 31, 32, 33, 63], 0, 100, 3),
        ("zero_capacity", [0, 2, 31, 32, 33, 63], 64, 0, 3),
        ("sparse", [0, 2, 31, 32, 33, 63], 64, 100, 3),
        ("partial", [0, 2, 31, 32, 33, 63], 33, 100, 3),
        ("capacity", [0, 2, 31, 32, 33, 63], 64, 6, 3),
        ("full", list(range(64)), 64, 100, 3),
    ):
        cases["distance_" + name] = (517, distance_field_requests(active, count, capacity, start))
    for name, active, count, small in (
        ("empty", [], 64, []),
        ("sparse", [0, 2, 31, 32, 33, 63], 64, []),
        ("partial", [0, 2, 31, 32, 33, 63], 33, []),
        ("small", [0, 2, 31, 32, 33, 63], 64, [2, 32]),
        ("full", list(range(64)), 64, []),
    ):
        cases["voxel_" + name] = (2138, cull_voxel(active, count, small))
    for name, active, count, capacity, valid_limit in (
        ("empty", [], 64, 80, 64),
        ("sparse", [0, 2, 31, 32, 33, 63], 64, 80, 64),
        ("partial", [0, 2, 31, 32, 33, 63], 33, 80, 64),
        ("capacity", [0, 2, 31, 32, 33, 63], 64, 6, 64),
        ("indices", [0, 2, 31, 32, 33, 63], 64, 80, 33),
        ("full", list(range(64)), 64, 80, 64),
    ):
        cases["feedback_" + name] = (1016, compact_feedback(active, count, capacity, valid_limit))
    for name, counts in {
        "empty": [],
        "one": [3],
        "sparse": [3 if i in (0, 2, 31, 32, 33, 63) else 0 for i in range(64)],
        "full": [i % 5 + 1 for i in range(64)],
        "partial": [i % 3 for i in range(37)],
    }.items():
        cases["commands_" + name] = (1019, command_space(counts))
    for count, start, mode in ((449, 5, "mixed"), (449, 0, "all"), (317, 3, "none"), (0, 0, "none")):
        cases[f"pack_{count}_{mode}"] = (1223, free_pages(count, start, mode))
    for count, group in ((75, 0), (75, 1), (75, -1), (0, -1)):
        cases[f"views_{count}_{group}"] = (3477, compact_views(count, group))
    for entry in (1243, 1257):
        for count in (0, 1, 31, 32, 33, 64):
            cases[f"cards_{entry}_{count}"] = (entry, card_update(count))
    for e in (1224, 1235):
        for count, layer in [(10, 0), (3, 0), (0, 0), (8, 1)]:
            cases[f"alloc_{e}_{count}_{layer}"] = (e, allocation(e, count, layer))
    for e in (1213, 1215, 1219, 1230, 1236):
        for count, extra in [(64, False), (64, True), (35, True), (0, True)]:
            cases[f"init_{e}_{count}_{int(extra)}"] = (e, selection(e, count, extra))
    for mode in ("zero", "disabled", "empty_views", "missing_pages", "visible", "shadow_cache", "shadow_blocked"):
        cases["instance_" + mode] = (3403, shadow_instances(mode))
    for mode in ("empty", "visible", "missing", "hierarchy"):
        cases["cluster_" + mode] = (1793, shadow_clusters(mode))
    for name, visible, occluders, protected in (
        ("empty", [1, 31, 32, 63], [], None),
        *((f"lane{lane}", [lane], [lane], None) for lane in (1, 31, 32, 63)),
        ("pair", [1, 32], [1, 32], None),
        ("full", [1, 31, 32, 63], [1, 31, 32, 63], None),
        ("protected", [1, 32], [1, 32], 1.0),
    ):
        cases["visibility_" + name] = (3396, visibility_instance(visible, occluders, protected))
        cases["visibility_post_" + name] = (3397, visibility_instance(visible, occluders, protected, post=True))
        for entry in (3398, 3399, 3412, 3422, 3430, 3432, 3438, 3449, 3451, 3454, 3456, 3457, 3458, 3470):
            cases[f"visibility_{entry}_" + name] = (entry, visibility_instance(
                visible, occluders, protected, post=entry not in (3399, 3438, 3430, 3449, 3451, 3456, 3458), stats_variant=entry in (3398, 3399, 3412, 3422, 3430, 3456, 3458, 3470)))
    if args.entry in (3413, 3416, 3426, 3433, 3437, 3440, 3468, 3474):
        for name, visible, occluders, protected in (
            ("empty", [1, 31, 32, 63], [], None),
            *((f"lane{lane}", [lane], [lane], None) for lane in (1, 31, 32, 63)),
            ("pair", [1, 32], [1, 32], None),
            ("full", [1, 31, 32, 63], [1, 31, 32, 63], None),
            ("protected", [1, 32], [1, 32], 1.0),
        ):
            cases["visibility_no_hzb_" + name] = (args.entry, visibility_instance(
                visible, occluders, protected, no_hzb=True,
                stats_variant=args.entry in (3413, 3437, 3440, 3474)))
    if args.entry is not None:
        cases = {name: case for name, case in cases.items() if case[0] == args.entry}
        if not cases:
            p.error(f"no GPU fixture for archive entry {args.entry}")
    with TemporaryDirectory(dir=ROOT / "build") as tmp:
        work = Path(tmp)
        unpack = decoder()
        for entry in sorted({entry for entry, _ in cases.values()}):
            native, source, profile, dxil = (work / f"{entry}.{ext}" for ext in ("dxbc", "hlsl", "profile", "dxil"))
            ((_, cooked, _),) = containers(decompress(data, entries[entry], unpack))
            native.write_bytes(cooked)
            subprocess.run([compiler, native, "--hlsl-out", source, "--profile-out", profile], check=True)
            subprocess.run([dxc, "-E", "main", "-T", profile.read_text().strip(), "-O3", "-Fo", dxil, source], check=True)
            subprocess.run([dxv, dxil], check=True, stdout=subprocess.DEVNULL)
            libraries[entry] = {"source_sha256": digest(native), "dxil_sha256": digest(dxil)}
        manifest = []
        for name, (entry, (cb, srv, buffers, strides, srv_stride, gx, gy, _, _)) in cases.items():
            manifest.append(" ".join(map(str, [name, entry, gx, gy, srv_stride, len(strides), *strides])))
            if entry in (1243, 1257, 1793, 3403):
                for i, constants in enumerate(cb):
                    words(work / f"{name}.b{i}", constants)
            else:
                words(work / (name+".cbv"), cb)
            if entry in (752, 1865, 728, 248, 249, 259, 507, 509, 512, 513, 517, 1016, 1243, 1257, 1793, 3477, 3396, 3397, 3398, 3399, 3403, 3412, 3413, 3416, 3426, 3433, 3437, 3440, 3468, 3474, 3422, 3430, 3432, 3438, 3449, 3451, 3454, 3456, 3457, 3458, 3470):
                for i, records in enumerate(srv):
                    words(work / f"{name}.t{i}", records)
            else:
                words(work / (name+".srv"), srv)
            for i, initial in enumerate(buffers):
                words(work / f"{name}.u{i}", initial)
        (work / "vsm.inputs").write_text("\n".join(manifest)+"\n")
        toolchain = Toolchain()
        obj = toolchain.compile(ROOT / "tools/reference/page-list/mappings.cpp", work, "/W4", "/WX", f"/I{ROOT / 'runtime/d3d12'}")
        exe = work / "vsm.exe"
        toolchain.link(exe, obj, "d3d12.lib", "dxgi.lib", "kernel32.lib")
        run = host.run(exe, [], timeout=60, capture=True)
        info = dict(line.split("=", 1) for line in (work / "vsm.txt").read_text().splitlines())
        results = {}
        passed = run.returncode == 0
        for name, (entry, case) in cases.items():
            expected, metadata = case[-2:]
            diffs = []
            for i, array in enumerate(expected):
                path = work / f"{name}.out{i}.bin"
                actual = list(struct.unpack(f"<{len(array)}I", path.read_bytes())) if path.is_file() else []
                if len(array) != len(actual):
                    diffs.append([i, "missing readback"])
                else:
                    if entry in (3396, 3397, 3398, 3399, 3412, 3413, 3416, 3426, 3433, 3437, 3440, 3468, 3474, 3422, 3430, 3432, 3438, 3449, 3451, 3454, 3456, 3457, 3458, 3470) and i < 2:
                        width = 36 if i == 0 else 9
                        for y in range(9):
                            actual[y * 64 + width:(y + 1) * 64] = [0] * (64 - width)
                        if i == 0 and len(metadata["occluders"]) > 1 and metadata["protected"] is None:
                            # Equal-depth atomics race: whichever lane wins owns the pixel.
                            winners = {0x800000a1 + (lane << 8) for lane in metadata["occluders"]}
                            if actual[272] in winners:
                                array = array.copy()
                                array[272] = actual[272]
                    if entry in (248, 249, 259) and metadata["groups"] > 1 and i in (1, 2):
                        # Sort both outputs using the same value permutation.
                        data_bytes = (work / f"{name}.out1.bin").read_bytes()
                        data = list(struct.unpack(f"<{len(data_bytes) // 4}I", data_bytes))
                        count = len(metadata["selected"])
                        order = sorted(range(count), key=lambda j: data[3 + j])
                        width = i
                        first = 3 * width
                        actual[first:first + count * width] = [
                            actual[first + j * width + k] for j in order for k in range(width)]
                    if entry in (507, 509, 512, 513) and i == 1 and metadata["groups"] > 1:
                        # Groups reserve contiguous blocks in unspecified order.
                        first, end = 15, 15 + 5 * len(metadata["selected"])
                        actual[first:end] = [word for row in sorted(
                            [actual[j:j + 5] for j in range(first, end, 5)]) for word in row]
                    if entry == 728 and i == 0 and metadata["alternate_offsets"] == actual:
                        array = metadata["alternate_offsets"]
                    if entry == 257 and i == 1:
                        end = 3 + len(metadata["selected"])
                        actual[3:end] = sorted(actual[3:end])
                    if entry == 1865 and metadata["irregular"]:
                        # Shader f32 sqrt/FMA rounding may differ by a few ULPs.
                        def close(a, b):
                            if a == b: return True
                            if a == GUARD or b == GUARD: return False
                            fa = struct.unpack("<f", struct.pack("<I", a))[0]
                            fb = struct.unpack("<f", struct.pack("<I", b))[0]
                            return abs(fa - fb) <= 2e-6
                        actual = [x if close(x, y) else y for x, y in zip(array, actual)]
                    if entry == 1223:
                        actual = unordered_wave_ranges(actual, array, metadata["initial"], metadata["selected_pages"])
                    diffs.extend([i, j, x, y] for j,(x,y) in enumerate(zip(array,actual)) if x != y)
            results[name] = {**metadata, "checked_words": sum(map(len,expected)), "mismatches": len(diffs), "first_mismatches": diffs[:4]}
            passed &= not diffs
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps({"gpu":info, "host":host.version, "exit":run.returncode, "libraries":libraries, "cases":results},indent=2)+"\n")
        if not passed:
            sys.stderr.buffer.write(run.stdout[-1500:])
            return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
