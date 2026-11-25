#!/usr/bin/env python3
"""Dispatch Frosty's packaged Nanite InstanceCull and check its compacted queues and counters."""
import argparse
import json
from pathlib import Path
import shutil
import struct
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import Toolchain
from check_frosty_shaders import GLOBAL_SHA, containers, decompress, global_archive
from project_tuxbox.host import Host
from project_tuxbox.inspect import digest
from project_tuxbox.runtime_shaders import shader_session
from project_tuxbox.shader_inventory import decoder
from project_tuxbox.shader_prep import compiler_tools


PROGRAMS = {
    3461: ("65751afa1088ed403f694333e09a2372c5ad661363d763b83e218e3fccc68c38", 2),
    3480: ("f156e46ca9464d65b51a6e6e2e606ef6b71b049d89208195a6c3183df8ba90d3", 1),
    3404: ("84563b69998d3b0fd49db337b7d3da8a587f4580916cfe6e96237025c3a8bed0", 2),
    3409: ("613ff2893f65b403027bd09bd315e484bf7acaae4e7f9a8bafab8ac0d30e6702", 2),
    3400: ("b40df2f9cb1784a9723935722f73727e8a4936539de8f84ecc6e432219481656", 2),
    3439: ("337f12e7d9ba902a8fd9c32b46994b2f53d6b0171675c88f9f0c6f0dd0726233", 1),
    3431: ("bc4c6e4eeb8125526dbb41a1024530695f06d07e1941dcbb362c8a315a1e8bc5", 1),
    3405: ("8bc365fc72410ebd3bebf6d20298233e8ae412eeb8047b8c78de6fe31c3e6b80", 1),
    3425: ("b07d956ff61846c8e25f0de1fd0f72d3d72a4910fca9599d990b401d34fc7637", 1),
    3444: ("06444185d9d7411b6a4cdf41ce1bd9a549ed6ab66add5a04b06a6209874a760d", 2),
    3452: ("85fe16be29d6ba9dae77cfc7fb2885b99ed3acb35ce0550e0b7464a90c361c61", 1),
    3473: ("cbe4d099f555f59bd1c3c5c2667918dcf5e01340cf7fbc4cb23f9374f15ffec9", 2),
}
SLOTS = 128
GUARD = 0xa5c3e749
QUEUE_BASE = 32
INITIAL_HEAD = 3
WIDTH = HEIGHT = 9


def save(path, words):
    path.write_bytes(struct.pack(f"<{len(words)}I", *words))


def load(path):
    return [word for word, in struct.iter_unpack("<I", path.read_bytes())]


def f32(number):
    return struct.unpack("<I", struct.pack("<f", number))[0]


def scene_inputs(path, visible, outside, *, instances, capacity, stats, image_words, view_centers,
                 hzb_depth, hzb_views, occluders=(), protected_depth=None, global_gate=True,
                 shadow_instances=(), coarse=False, split_depth=False, viewport=(WIDTH, HEIGHT)):
    path.mkdir()
    cb = [0] * 64
    cb[:5] = [instances, capacity, 64, 0, SLOTS]
    cb[8] = QUEUE_BASE << 4
    cb[10] = 64 if global_gate else 0
    cb[11] = int(stats)
    cb[12] = len(view_centers)
    if hzb_depth is not None:
        cb[16:18] = [f32(8)] * 2
    save(path / "b0", cb)
    # One primitive, with its local center at zero and extent 1/8 on each axis.
    # Its instance stream stores metadata, a packed unit transform and translation in separate columns.
    primitive = [0] * (38 * 4)
    primitive[0] = 0x4000 if occluders else 0
    primitive[27 * 4:27 * 4 + 3] = [f32(.125)] * 3
    if shadow_instances:
        shadow = primitive.copy()
        shadow[0] |= 0x2000
        primitive.extend(shadow)
    save(path / "t0", primitive)
    scene = [0] * (SLOTS * 3 * 4)
    for index in range(SLOTS):
        active = index in visible or index in outside or index in occluders
        scene[4 * index] = int(index in shadow_instances) if active else 0xfffff
        transform = 4 * (SLOTS + index)
        scene[transform:transform + 4] = [0x80008000, 0x4000, 0x80018001, 0x008e8001]
        x = 4 if index in outside else 0
        if split_depth:
            x = -.5 if index % 2 else .5
        scene[4 * (2 * SLOTS + index)] = f32(x)
    save(path / "t1", scene)
    save(path / "t2", [0] * 16)
    view = [0] * 200
    # Orthographic X and Y, fixed Z=0.75 and W=1. A center at X=4 falls outside.
    view[48] = view[53] = view[63] = f32(1)
    view[62] = f32(.75)
    view[162:164] = viewport
    view[78:80] = [f32(.375), f32(.5)]
    view[92:95] = [f32(-.125), f32(-.125), f32(-1)]
    views = []
    for view_id, center in enumerate(view_centers):
        # Translating the last matrix row changes which instance bounds fit
        # inside this view. Repeated centers must emit separate view IDs.
        view[60] = f32(-center)
        if hzb_depth is not None:
            view[112:128] = view[48:64]  # Previous view's clip transform.
            view[191] = 4 if view_id in hzb_views else 0
            view[196:200] = [0, 0, 16, 16]
            if coarse:
                # Project the 1/8-extent bounds across the full previous
                # viewport. An 8x8 HZB footprint fits a 4x4 tile at mip 1.
                view[112] = view[117] = f32(8)
        views.extend(view)
    save(path / "t3", views or view)
    # Each packed impostor sample has the same material ID (0x21) and coverage.
    save(path / "t4", [0x7f217f21] * (12 * 1024) if occluders else [0] * 16)
    pixels = []
    for i in range(WIDTH * HEIGHT):
        if image_words == 2:
            pixels.append(i)
        pixels.append(f32(.25))
    center = ((HEIGHT // 2) * WIDTH + WIDTH // 2) * image_words
    if protected_depth is not None:
        pixels[center:center + image_words] = ([0xffffffff] if image_words == 2 else []) + [f32(protected_depth)]
    save(path / "u0", pixels)
    if occluders and protected_depth is None:
        # All selected impostors project to the middle texel at depth .375 / .5.
        # Unsigned 64-bit max breaks equal-depth ties with the larger instance ID.
        pixels[center:center + image_words] = ([0x800000a1 + (max(occluders) << 8)] if image_words == 2 else []) + [f32(.75)]
    if hzb_depth is not None:
        levels = [1 - hzb_depth, hzb_depth, 1 - hzb_depth, 1 - hzb_depth] if coarse else [hzb_depth] * 4
        texels = [f32(depth) for level, depth in enumerate(levels) for _ in range(64 >> (2 * level))]
        if split_depth:
            texels[:64] = [f32(1 if x < 4 else 0) for _ in range(8) for x in range(8)]
        save(path / "hzb", texels)
    return pixels


def prepare_case(path, visible, outside, *, instances=64, groups=1, capacity=128, stats=False,
                 image_words=2, occluders=(), protected_depth=None, view_centers=(0,),
                 global_gate=True, shadow_instances=(), hzb_depth=None, hzb_views=(), coarse=False, split_depth=False):
    pixels = scene_inputs(path, visible, outside, instances=instances, capacity=capacity, stats=stats,
                          image_words=image_words, occluders=occluders, protected_depth=protected_depth,
                          view_centers=view_centers, global_gate=global_gate, shadow_instances=shadow_instances,
                          hzb_depth=hzb_depth, hzb_views=hzb_views, coarse=coarse, split_depth=split_depth)
    queue = [GUARD] * (2 * SLOTS + 16)
    counters = [GUARD] * 32
    counters[4:6] = [INITIAL_HEAD, 19]
    stats_buffer = [GUARD] * 32
    stats_buffer[4] = 43
    if hzb_depth is not None:
        save(path / "u1", [GUARD] * 512)
        save(path / "u2", [1, 1, 1, 62] + [GUARD] * 12)
    for i, words in enumerate((queue, counters, stats_buffer), 1 if hzb_depth is None else 3):
        save(path / f"u{i}", words)
    active = [(i, view_id) for view_id, center in enumerate(view_centers)
              for i in sorted((*visible, *outside)) if i < min(instances, groups * 64)
              and (global_gate or i in shadow_instances) and abs((4 if i in outside else 0) - center) <= 1]
    rejected = [(i, v) for i, v in active if v in hzb_views and
                (i % 2 if split_depth else hzb_depth > .75)]
    active = [pair for pair in active if pair not in rejected]
    return dict(groups=groups, active=active, rejected=rejected, hzb=hzb_depth is not None, capacity=capacity, stats=stats, pixels=pixels,
                views=len(view_centers))


def verify_case(path, case):
    start = 3 if case["hzb"] else 1
    queue, counters, stats = (load(path / f"u{i}") for i in range(start, start + 3))
    observed, read_counts, read_stats = (load(path / f"out{i}") for i in range(start, start + 3))
    if len(observed) != len(queue):
        raise ValueError(f"{path.name} truncated queue readback")
    active = case["active"]
    counters[4] += len(active)
    counters[5] += len(active)
    if case["stats"]:
        stats[4] += len(active)
    if counters != read_counts or stats != read_stats:
        raise ValueError(f"{path.name} counter mismatch: expected {counters[4:6]}, {stats[4]}, "
                         f"got {read_counts[4:6]}, {read_stats[4]}")
    stored = min(len(active), max(0, case["capacity"] - INITIAL_HEAD))
    # Each workgroup reserves in increasing view order. Reservations from
    # different workgroups may interleave, while each reservation is contiguous.
    first = QUEUE_BASE // 4 + 2 * INITIAL_HEAD
    records = [(observed[first + 2 * i] >> 3, observed[first + 2 * i + 1] >> 16) for i in range(stored)]
    batches = [(instance // 64, view) for instance, view in records]
    order = list(dict.fromkeys(batches))
    expected = [(i, v) for group, view in order for (i, v) in active if i // 64 == group and v == view][:stored]
    if len(expected) != stored or records != expected or (stored == len(active) and sorted(records) != sorted(active)):
        raise ValueError(f"{path.name} compacted records: expected visible {active}, got {records}")
    for group in range(case["groups"]):
        expected_views = sorted({v for i, v in active if i // 64 == group})
        observed_views = [v for g, v in order if g == group]
        if observed_views != expected_views[:len(observed_views)]:
            raise ValueError(f"{path.name} group {group} reserved views out of order: {observed_views}")
    for offset, (instance, view) in enumerate(records):
        queue[first + 2 * offset:first + 2 * offset + 2] = [(instance << 3) | 1, view << 16]
    if observed != queue:
        diff = next((i, a, b) for i, (a, b) in enumerate(zip(queue, observed)) if a != b)
        raise ValueError(f"{path.name} queue mismatch at word {diff[0]}: {diff[1]:08x} != {diff[2]:08x}")
    pixels = load(path / "out0")
    if pixels != case["pixels"]:
        diff = [(i, hex(a), hex(b)) for i, (a, b) in enumerate(zip(case["pixels"], pixels)) if a != b]
        raise ValueError(f"{path.name} visibility image mismatches: {diff[:12]}")
    changed = sum(a != b for a, b in zip(pixels, load(path / "u0")))
    words = len(pixels) // (WIDTH * HEIGHT)
    middle = ((HEIGHT // 2) * WIDTH + WIDTH // 2) * words
    hzb_result = {}
    if case["hzb"]:
        slots, args = load(path / "u1"), load(path / "u2")
        expected_pairs = case["rejected"]
        pos = args[3]
        args[3] += len(expected_pairs)
        args[0] = max(args[0], (args[3] + 63) // 64)
        for index, (instance, view) in enumerate(expected_pairs, pos):
            slots[2 * index:2 * index + 2] = [instance, view]
        actual = load(path / "out2")
        if args != actual or slots != load(path / "out1"):
            raise ValueError(f"{path.name} incorrect HZB arguments {actual[:4]} or reject list")
        hzb_result = dict(occluded=expected_pairs, dispatch_args=args[:4], checked_hzb_words=len(args) + len(slots))
    return dict(**hzb_result, checked_texels=WIDTH * HEIGHT, changed_visibility_words=changed,
                center_words=[f"{value:08x}" for value in pixels[middle:middle + words]],
                groups=case["groups"], visible=len(active), queue_ids=[i for i, _ in records], counters=read_counts[4:6],
                stats_counter=read_stats[4], checked_words=len(queue) + len(counters) + len(stats),
                **({"views": case["views"], "queue_views": [v for _, v in records]} if case["views"] != 1 else {}))


def post_cases(trial, entry, words):
    cases = {}

    def add(name, records, *, stats=False, depth=0., capacity=16, split=False, occluders=(), active=None):
        path = trial / name
        count = len(records)
        ids = {instance for instance, _ in records}
        views = (0, 0) if entry == 3480 else (0,)
        pixels = scene_inputs(path, ids, [], instances=SLOTS, image_words=words, hzb_depth=depth,
                              hzb_views=range(len(views)), view_centers=views, stats=stats,
                              capacity=capacity, occluders=occluders, split_depth=split,
                              viewport=(16, 16) if split else (WIDTH, HEIGHT))
        # The post pass reads the first pass's compacted list and indirect
        # count, then appends three-word nodes after the primary queue.
        save(path / "t6", [word for pair in records for word in pair] or [0, 0])
        save(path / "t7", [1, 1, 1, count] + [GUARD] * 4)
        nodes, counters, stats_buffer = [GUARD] * 512, [GUARD] * 11, [GUARD] * 14
        counters[9:11] = [INITIAL_HEAD, 29]
        stats_buffer[8] = 43
        for i, values in enumerate((nodes, counters, stats_buffer), 1):
            save(path / f"u{i}", values)
        visible = records if active is None else active
        stored = visible[:max(0, capacity - INITIAL_HEAD)]
        start = ((QUEUE_BASE << 4) >> 3) // 4 + capacity * 2 + INITIAL_HEAD * 3
        for i, (instance, view) in enumerate(stored):
            nodes[start + i * 3:start + i * 3 + 3] = [(instance << 3) | 5, view << 16, 255]
        counters[9] += len(visible)
        counters[10] += len(visible)
        if stats:
            stats_buffer[7:9] = [count, 43 + len(visible)]
        cases[name] = dict(groups=1, post=[pixels, nodes, counters, stats_buffer],
                           visible=len(visible), nodes=stored)

    v = 1 if entry == 3480 else 0
    add("empty-post", [], stats=True)
    records = [(36, v), (3, 0), (36, 0), (54, v)]
    add("previously-hidden", records, stats=True)
    add("post-limit", records, capacity=5)
    add("post-hzb-hidden", records, stats=True, depth=1., active=[])
    add("post-hzb-left-half", records, stats=True, split=True, active=[r for r in records if r[0] % 2 == 0])
    add("post-visibility", [(2, v), (37, 0)], stats=True, occluders=[2, 37], active=[])
    add("post-upper-lanes", [(63 - i, i % (v + 1)) for i in range(39)], stats=True, capacity=48)
    return cases


def verify_post(path, case):
    for slot, expected in enumerate(case["post"]):
        actual = load(path / f"out{slot}")
        if actual != expected:
            wrong = [(i, hex(w), hex(a)) for i, (w, a) in enumerate(zip(expected, actual)) if w != a]
            raise ValueError(f"{path.name} u{slot} mismatches {wrong[:8]}")
    return dict(groups=case["groups"], nodes=case["nodes"], visible=case["visible"], checked_texels=WIDTH * HEIGHT,
                checked_words=sum(len(buffer) for buffer in case["post"][1:]),
                counters=case["post"][2][9:11], statistics=case["post"][3][7:9])


def first_pass_cases(trial, entry, words):
    cases = {}

    has_hzb = entry in (3400, 3404, 3431, 3439)
    def add(name, visible, outside, **options):
        if has_hzb:
            options.setdefault("hzb_depth", 0.0)
        cases[name] = prepare_case(trial / name, visible, outside, image_words=words, **options)

    add("empty", [], [])
    add("clip", [], [2, 34])
    add("upper", [37], [35], stats=True)
    add("sparse", [3, 7, 31, 32, 47, 63], [4, 35])
    add("capacity", [3, 15, 36, 47, 61], [4], capacity=5, stats=True)
    add("two-groups", [3, 34, 61, 64, 95, 106, 122], [2, 98], instances=107, groups=2, stats=True)
    add("visibility", [], [], occluders=[0, 37], stats=True)
    add("visibility-guard", [], [], occluders=[37], protected_depth=.875)
    if has_hzb:
        add("hzb-clear", [1, 31, 32, 63], [], hzb_views=(0,))
        add("hzb-occluded", [1, 31, 32, 63], [], hzb_depth=1, hzb_views=(0,))
        add("hzb-left-half", [1, 2, 31, 32, 47, 48], [], hzb_depth=0, hzb_views=(0,), split_depth=True)
        add("hzb-mip1-hidden", [7, 48], [], hzb_depth=1, hzb_views=(0,), coarse=True)
        add("hzb-mip1-clear", [7, 48], [], hzb_depth=0, hzb_views=(0,), coarse=True)
    if entry in (3404, 3431):
        add("hzb-view-mask", [1, 31, 32, 63], [], view_centers=(0, 0), hzb_depth=1, hzb_views=(1,), stats=True)
    if entry in (3404, 3405, 3409, 3431):
        add("zero-views", [0, 37], [63], view_centers=(), stats=True)
        add("two-views", [3, 32, 47], [7, 31, 63], view_centers=(0, 4), stats=True)
        add("three-views", [32, 47], [7, 63], view_centers=(4, 0, 4))
        add("view-capacity", [3, 32, 47], [7, 31, 63], view_centers=(0, 4), capacity=7, stats=True)
        add("view-groups", [3, 34, 64, 106, 122], [31, 63, 98, 104], view_centers=(0, 4, 0),
            instances=107, groups=2, stats=True)
        add("primitive-gate", [3, 32, 47], [7, 63], view_centers=(0, 4), global_gate=False,
            shadow_instances=[32, 63], stats=True)
        add("gate-closed", [32], [63], view_centers=(0, 4), global_gate=False, stats=True)
    if words == 2:
        add("visibility-tie", [], [], occluders=[37], protected_depth=.75)
    return cases


def dispatch(original, host, adapter, trial, entry):
    fingerprint, words = PROGRAMS[entry]
    source = trial / "instance.dxbc"
    source.write_bytes(original)
    if digest(source) != fingerprint:
        raise ValueError("Unexpected source for Frosty InstanceCull")
    if entry in (3461, 3480):
        cases, verify = post_cases(trial, entry, words), verify_post
    else:
        cases, verify = first_pass_cases(trial, entry, words), verify_case
    (trial / "image.txt").write_text(f"{WIDTH} {HEIGHT} {words}\n")
    (trial / "cases.txt").write_text("".join(f"{name} {case['groups']}\n" for name, case in cases.items()))
    exe = trial / "cull.exe"
    cache = trial / "temp/tuxbox_shader_cache"
    compiled = []
    with shader_session(cache, host.env, compiled.append):
        session = cache / host.env["TUXBOX_SHADER_SESSION"]
        result = host.run(exe, ["--adapter", adapter, "--report", "result.txt"], capture=True, timeout=150)
        receipt = trial / "result.txt"
        line = receipt.read_text().strip() if receipt.exists() else ""
        if result.returncode or line != f"instance-cull PASS pci={adapter} cases={len(cases)}":
            sys.stderr.buffer.write(result.stdout[-4000:])
            raise ValueError(f"InstanceCull GPU test failed: {line}, compilation: {compiled}")
        if compiled != [f"Compiled Xbox shader {fingerprint}"]:
            raise ValueError(f"Unexpected runtime compilation: {compiled}")
        dxil = digest(session / (fingerprint + ".cso"))
    return dict(entry=entry, visibility_format="R32G32_UINT" if words == 2 else "R32_UINT",
                source_sha256=fingerprint, dxil_sha256=dxil,
                cases={name: verify(trial / name, case) for name, case in cases.items()})


def run(mount, host, adapter, provider, trial, selected):
    payload, entries = global_archive(mount)
    decode = decoder()
    build = Toolchain()
    obj = build.compile(ROOT / "tools/reference/instance-cull/guest.cpp", trial, "/W4", "/WX",
                        f"/I{ROOT / 'runtime/d3d12'}")
    exe = trial / "cull.exe"
    build.link(exe, obj, str(ROOT / "build/d3d12-smoke/d3d12_xs.lib"), "dxgi.lib", "kernel32.lib")
    runs = []
    for entry in selected:
        work = trial / str(entry)
        work.mkdir()
        shutil.copyfile(provider, work / "d3d12_xs.dll")
        shutil.copyfile(exe, work / exe.name)
        ((_, original, _),) = containers(decompress(payload, entries[entry], decode))
        runs.append(dispatch(original, host, adapter, work, entry))
    return dict(host=host.version, adapter=adapter, global_archive_sha256=GLOBAL_SHA,
                name="InstanceCull", guest_sha256=digest(exe), provider_sha256=digest(provider),
                compiler_sha256=digest(compiler_tools()[0]), programs=runs,
                scope="Original compute shader and root signature at Xbox device slot 62. The fixtures check frustum "
                      "culling, wave allocation, clipped queue writes, optional statistics, multiview candidate records and impostor visibility atomics. "
                      "HZB permutations additionally check the occluded instance list and next dispatch arguments, including "
                      "spatially distinct depths and explicit sampling of mip 1. Post-pass checks consume that list and "
                      "verify three-word candidate records, clipping, counters and impostors. Each check compares every output word.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mount", type=Path)
    parser.add_argument("--entry", type=int, choices=sorted(PROGRAMS), help="test one program")
    parser.add_argument("--adapter", required=True)
    parser.add_argument("--proton", type=Path)
    parser.add_argument("--prefix", type=Path)
    parser.add_argument("--provider", type=Path, default=ROOT / "build/d3d12-smoke/d3d12_xs.dll")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    with TemporaryDirectory(prefix="instance-cull-", dir=ROOT / "build") as tmp:
        result = run(args.mount, Host("proton", args.proton, args.prefix), args.adapter.lower(),
                     args.provider, Path(tmp), [args.entry] if args.entry else sorted(PROGRAMS))
    args.output.parent.mkdir(exist_ok=True, parents=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
