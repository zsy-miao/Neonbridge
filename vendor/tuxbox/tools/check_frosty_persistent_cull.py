#!/usr/bin/env python3
"""Dispatch Frosty's persistent cluster culler and verify its queues and draw arguments."""
import argparse
import json
from pathlib import Path
import shutil
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import Toolchain
from check_frosty_instance_cull import f32, load, save
from check_frosty_shaders import GLOBAL_SHA, containers, decompress, global_archive
from project_tuxbox.host import Host
from project_tuxbox.inspect import digest
from project_tuxbox.runtime_shaders import shader_session
from project_tuxbox.shader_inventory import decoder
from project_tuxbox.shader_prep import compiler_tools

PROGRAMS = {
    1827: "178e312aad98103a2c922b3c394f39f45f0248383b962cf12e21027810aa5d36",
    1790: "708c697a5b8d94454f00f83f7eb91bf962b53138ad3560c87f6e1217959af4ad",
    1807: "b0cd492f0c3d2067856de6da743dabac06824a3e4a9b48d2f5cafdf6a1bae205",
    1814: "f06a25d40bb2fd4878c6ef759dd470badfca8dfa175103cbb23b95fddedb0e9a",
    1825: "5feb276685031ace58dc8e00634bb712f13caf79dd7cb25ed314177f36fb6e6a",
}
GUARD = 0xa5c3e749
INSTANCES = 64
BATCHES = 4
CAPACITY = 80


def pack_cluster(instance, cluster, view=0):
    return [(instance << 15) | (view << 3) | 1, cluster << 23]


def prepare(path, candidates=(), *, outside=(), groups=1, flags=0, capacity=CAPACITY,
            children=(), stream=False, nested=False, high_error=(), long_edges=(), second_view=(),
            indexed_hierarchy=False, hzb=False, hzb_depth=None, split_depth=False, coarse=False, post_head=0):
    path.mkdir()
    # b0[4] determines both the number of cluster batches and the byte offset
    # of node entries in u2.
    constants = [0] * 128
    constants[:7] = [INSTANCES, 1, 0, 0, BATCHES * 64, capacity, flags]
    constants[64] = 8 if children or hzb else 0
    if hzb:
        constants[12:14] = [f32(8)] * 2
        pyramid = [f32(hzb_depth or 0)] * 85
        if split_depth:
            pyramid[:64] = [f32(float(x < 4)) for _ in range(8) for x in range(8)]
        if coarse:
            pyramid = [f32(1 - hzb_depth)] * 64 + [f32(hzb_depth)] * 16 + [f32(1 - hzb_depth)] * 5
        save(path / "hzb", pyramid)
    save(path / "b0", constants)
    primitive = [0] * (38 * 4)
    primitive[26 * 4 + 3] = int(indexed_hierarchy)
    primitive[22 * 4 + 3] = 0x57
    save(path / "t0", primitive)
    scene = [0] * (12 * INSTANCES)
    for i in range(INSTANCES):
        k = 4 * (INSTANCES + i)
        scene[k:k + 4] = [0x80008000, 0x4000, 0x80018001, 0x008e8001]
        scene[4 * (2 * INSTANCES + i)] = f32(4 if i in outside else 0)
    if indexed_hierarchy:
        scene[0] |= 1 << 25
    if hzb:
        # Previous packed transform and translation occupy two more instance columns.
        scene.extend(scene[4 * INSTANCES:])
        if split_depth:
            for i in candidates:
                scene[4 * (4 * INSTANCES + i)] = f32(-.5 if i % 2 else .5)
    save(path / "t1", scene)
    lookup = [0] * 16
    # The instance hierarchy offset is the first word of a float4-sized record.
    lookup[4] = int(indexed_hierarchy)
    save(path / "t2", lookup)
    # Two clusters in one resident page. Bounds are stored in float4 columns.
    count = 2
    page = [0] * 128
    page[0] = count
    for i in range(count):
        box = 4 + 4 * i + 16 * count
        # Center.w packs two IEEE half-floats. For this orthographic view,
        # error 2 exceeds the LOD threshold 1. Edge length 2 selects hardware.
        page[box - 4 * count + 3] = (0x4000 if i in high_error else 0) | (0x40000000 if i in long_edges else 0)
        page[box:box + 4] = [f32(.125)] * 3 + [0 if i in high_error else 1]
    save(path / "t3", page)
    view = [0] * 200
    view[48] = view[53] = view[63] = view[79] = f32(1)
    view[62] = f32(.75)
    view[162:164] = [128, 128]
    view[188:190] = [f32(1), f32(1)]
    view[191] = 2
    if hzb_depth is not None:
        view[112:128] = view[48:64]
        if coarse:
            view[112] = view[117] = f32(8)
        view[191] |= 4
        view[196:200] = [0, 0, 16, 16]
    if second_view:
        # View zero sees instances at x=0. View one sees instances at x=4.
        translated = view.copy()
        translated[60] = f32(-4)
        view.extend(translated)
    save(path / "t4", view)
    hierarchy = [0] * 104
    for cluster, child in enumerate(children):
        hierarchy[32 + 4 * child + 3] = 0x40000000  # upper half-float LOD error 2
        box = 64 + 4 * child
        hierarchy[box:box + 4] = [f32(.25)] * 3 + [0xffffffff if stream else cluster]
        hierarchy[96 + child] = 0x123200 if stream else 1
    if nested:
        # Internal child five refers to hierarchy node one. Its leaf children
        # reside in the second 416-byte packed node.
        root = [0] * 104
        root[32 + 4 * 5 + 3] = 0x40000000
        root[64 + 4 * 5:64 + 4 * 5 + 4] = [f32(.25)] * 3 + [1]
        root[96 + 5] = 0xffffffff
        hierarchy = root + hierarchy
    if indexed_hierarchy:
        hierarchy = [0] * 104 + hierarchy
    save(path / "t5", hierarchy)
    offsets = [3, 4]
    save(path / "t6", offsets)
    # Nonzero initial draw counters exercise atomic return values and allow
    # sentinel checks before and after both ends of the visible list.
    draws = [GUARD] * 12
    draws[0], draws[4], draws[5] = 2, 3, 5
    work = [GUARD] * 33
    work[:6] = [0, 0, len(candidates), 0, 0, 0]
    nodes = [0] * BATCHES + [0xffffffff] * 16
    if hzb:
        nodes.extend([0] * BATCHES + [GUARD] * (3 * constants[64]))
    nodes.extend([GUARD] * 16)
    nodes[0] = len(candidates)
    # Post-pass records grow down from the end of the BATCHES*64 cluster array.
    raw = [GUARD] * (2 * (BATCHES * 64 if hzb else 64) + 8)
    if hzb:
        work[0] = post_head
        work[6:11] = [0, post_head, 0, 0, 0]
        for rank in range(post_head):
            slot = BATCHES * 64 - 1 - rank
            raw[2 * slot:2 * slot + 2] = [(rank % INSTANCES) << 15, (rank % count) << 23]
            nodes[BATCHES + 2 * constants[64] + rank // 64] += 1
    for i, instance in enumerate(candidates):
        raw[2 * i:2 * i + 2] = pack_cluster(instance, i % count, int(instance in second_view))
    requests = [GUARD] * 24
    requests[0] = 0
    if children:
        work[4:6] = [1, 1]
        nodes[BATCHES:BATCHES + 2] = [1, 0]  # instance zero and hierarchy root
    uavs = [requests, work, nodes, raw, [GUARD] * (2 * CAPACITY + 8), draws]
    for i, words in enumerate(uavs):
        save(path / f"u{i}", words)
    if children:
        node_hidden = hzb_depth is not None and hzb_depth > .75
        work[4] += int(nested and not node_hidden)
        work[5] -= 1
        nodes[BATCHES:BATCHES + 2] = [0xffffffff] * 2
        if node_hidden:
            # One three-word post-pass node retains the bit mask of hidden children.
            work[9:11] = [1, 1]
            post_nodes = 2 * BATCHES + 2 * constants[64]
            mask = 1 << 5 if nested else sum(1 << child for child in children)
            nodes[post_nodes:post_nodes + 3] = [0, 0, mask]
        elif stream:
            requests[0] += len(children)
            for rank in range(len(children)):
                start = 3 * (rank + 1)
                requests[start:start + 3] = [0x5700, 0x91900, 0x90000000]
        else:
            candidates = [0] * len(children)
            work[2] += len(candidates)
            if hzb:
                work[0] += len(candidates)
            for i in range(len(candidates)):
                raw[2 * i:2 * i + 2] = pack_cluster(0, i)
    work[3] += 8 * groups
    work[1] += groups + int(bool(candidates))
    nodes[0] = 0
    for rank, instance in enumerate(candidates):
        cluster = rank % count
        view_index = int(instance in second_view)
        clipped = (instance in outside) != bool(view_index)
        if clipped or cluster in high_error:
            continue
        hardware = bool(flags & 2) or cluster in long_edges
        if hzb_depth is not None and (instance % 2 if split_depth else hzb_depth > .75):
            work[0] += 1
            end = work[7]
            work[7] += 1
            slot = BATCHES * 64 - 1 - end
            nodes[BATCHES + 2 * constants[64] + end // 64] += 1
            raw[2 * slot:2 * slot + 2] = [(instance << 15) | (view_index << 3) | (2 if hardware else 0), cluster << 23]
            continue
        draw_arg = (4 if flags & 12 else 5) if hardware else 0
        index = draws[draw_arg] + (offsets[int(hardware)] if flags & 1 else 0)
        draws[draw_arg] += 1
        if index < capacity:
            slot = capacity - 1 - index if hardware else index
            uavs[4][2 * slot:2 * slot + 2] = pack_cluster(instance, cluster, view_index)
    return groups, uavs


def cases(trial, entry):
    upper = range(64)
    sparse = [i for i in upper if i not in (0, 7, 31, 32, 37, 63)]
    specs = {
        "empty": {},
        "empty-groups": dict(groups=3),
        "culled": dict(candidates=[2, 37], outside=[2, 37]),
        "software": dict(candidates=[7, 37], outside=[7]),
        "hardware": dict(candidates=[2, 33, 49], outside=[33], flags=2),
        "offset": dict(candidates=[5, 37], flags=1),
        "hardware-capacity": dict(candidates=[3, 7, 37], flags=7, capacity=8),
        "wave": dict(candidates=upper, outside=sparse),
        "lod": dict(candidates=[2, 7, 9, 37, 41], high_error=[0]),
        "mixed-routing": dict(candidates=upper, outside=sparse, long_edges=[1]),
        "hierarchy": dict(children=[0, 7]),
        "indexed-hierarchy": dict(children=[0, 7], indexed_hierarchy=True),
        "nested-hierarchy": dict(children=[2, 6], nested=True),
        "multiple-groups": dict(candidates=upper, outside=sparse, groups=3),
        "stream-request": dict(children=[7], flags=16, stream=True),
    }
    if entry in (1790, 1814):
        specs["two-views"] = dict(candidates=[1, 2, 3, 4], outside=[2, 4], second_view=[2, 3])
    if entry == 1827:
        batch = [1, 31, 32, 63]
        specs["hzb-clear"] = dict(candidates=batch, hzb_depth=0)
        specs["hzb-covered"] = dict(candidates=batch, hzb_depth=1)
        specs["hzb-mip1-clear"] = dict(candidates=batch, hzb_depth=0, coarse=True)
        specs["hzb-mip1-covered"] = dict(candidates=batch, hzb_depth=1, coarse=True)
        specs["hzb-batch-boundary"] = dict(candidates=batch, hzb_depth=1, post_head=62, flags=2)
        specs["hzb-left-half"] = dict(candidates=[1, 2, 31, 32], hzb_depth=0, split_depth=True)
        specs["hzb-resident-node"] = dict(children=[0, 7], hzb_depth=0)
        specs["hzb-covered-node"] = dict(children=[0, 7], hzb_depth=1)
        specs["hzb-covered-parent"] = dict(children=[0, 7], hzb_depth=1, nested=True)
    return {name: prepare(trial / name, hzb=entry == 1827, **options) for name, options in specs.items()}


def verify(path, expected):
    outputs = []
    for i, words in enumerate(expected):
        actual = load(path / f"out{i}")
        if len(words) != len(actual):
            raise ValueError(f"{path.name} partial u{i} readback")
        if words != actual:
            diffs = [(k, hex(a), hex(b)) for k, (a, b) in enumerate(zip(words, actual)) if a != b]
            raise ValueError(f"{path.name} u{i} mismatches (word, expected, GPU): {diffs[:8]}")
        outputs.append(actual)
    writes = [[k // 2, *outputs[4][k:k + 2]] for k in range(0, len(outputs[4]), 2)
              if outputs[4][k] != GUARD]
    requests = [outputs[0][i * 3:i * 3 + 3] for i in range(1, 1 + outputs[0][0])]
    candidate_count = outputs[1][2] - load(path / "u1")[2]
    generated = [outputs[3][2 * i:2 * i + 2] for i in range(candidate_count)]
    occlusion = {}
    if (path / "hzb").exists():
        start, end = load(path / "u1")[7], outputs[1][7]
        slots = (BATCHES * 64 - 1 - i for i in range(start, end))
        post_nodes = 2 * BATCHES + 2 * load(path / "b0")[64]
        node_offsets = (post_nodes + i * 3 for i in range(outputs[1][9]))
        occlusion = dict(post_head=end, occluded_records=[[slot, *outputs[3][2 * slot:2 * slot + 2]] for slot in slots],
                         occluded_nodes=[outputs[2][offset:offset + 3] for offset in node_offsets])
    return dict(**occlusion, queue_words=outputs[1][:6], streaming_requests=requests, generated_candidates=generated,
                draw_counts=[outputs[5][i] for i in (0, 4, 5)],
                visible_records=writes, verified_uav_words=sum(map(len, expected)))


def dispatch(host, adapter, trial, entry, shader):
    fingerprint = PROGRAMS[entry]
    source = trial / "original.dxbc"
    source.write_bytes(shader)
    if digest(source) != fingerprint:
        raise ValueError("Unexpected PersistentClusterCull package shader")
    fixtures = cases(trial, entry)
    (trial / "cases.txt").write_text("".join(f"{name} {groups}\n" for name, (groups, _) in fixtures.items()))
    exe = trial / "persistent.exe"
    cache = trial / "temp/tuxbox_shader_cache"
    compiled = []
    with shader_session(cache, host.env, compiled.append):
        session = cache / host.env["TUXBOX_SHADER_SESSION"]
        result = host.run(exe, ["--adapter", adapter, "--report", "result.txt"], capture=True, timeout=150)
        receipt = trial / "result.txt"
        line = receipt.read_text().strip() if receipt.exists() else ""
        if result.returncode or line != f"persistent-cull PASS pci={adapter} cases={len(fixtures)}":
            sys.stderr.buffer.write(result.stdout[-4000:])
            raise ValueError(f"PersistentClusterCull dispatch failed: {line}, compilation: {compiled}")
        if compiled != [f"Compiled Xbox shader {fingerprint}"]:
            raise ValueError(f"Unexpected runtime compilation: {compiled}")
        dxil = digest(session / (fingerprint + ".cso"))
    return dict(entry=entry, source_sha256=fingerprint, dxil_sha256=dxil,
                cases={name: verify(trial / name, expected) for name, (_, expected) in fixtures.items()})


def run(mount, host, adapter, provider, trial, selected):
    archive, entries = global_archive(mount)
    decode = decoder()
    build = Toolchain()
    obj = build.compile(ROOT / "tools/reference/persistent-cull/guest.cpp", trial,
                        "/W4", "/WX", f"/I{ROOT / 'runtime/d3d12'}")
    exe = trial / "persistent.exe"
    build.link(exe, obj, str(ROOT / "build/d3d12-smoke/d3d12_xs.lib"), "dxgi.lib", "kernel32.lib")
    programs = []
    for entry in selected:
        work = trial / str(entry)
        work.mkdir()
        shutil.copyfile(provider, work / "d3d12_xs.dll")
        shutil.copyfile(exe, work / exe.name)
        ((_, shader, _),) = containers(decompress(archive, entries[entry], decode))
        programs.append(dispatch(host, adapter, work, entry, shader))
    return dict(name="PersistentClusterCull", adapter=adapter, host=host.version, global_archive_sha256=GLOBAL_SHA,
                guest_sha256=digest(exe), provider_sha256=digest(provider), compiler_sha256=digest(compiler_tools()[0]),
                programs=programs, scope="Original Xbox compute programs and embedded root signatures at device slot "
                "62. Every case reads all six UAVs. Fixtures check queue reservations, resident children, streaming "
                "requests, projection clipping, LOD rejection, hardware and software lists, previous-pass offsets "
                "and visible-list capacity. All variants read an indexed 16-byte hierarchy lookup, and the two multiview "
                "variants select the second 800-byte view. The HZB checks cover previous transforms, mixed visibility and "
                "post-pass cluster reservations across a 64-record batch boundary. Each visible or occluded record "
                "contains the slot followed by the two packed words.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mount", type=Path)
    parser.add_argument("--proton", type=Path)
    parser.add_argument("--prefix", type=Path)
    parser.add_argument("--adapter", required=True)
    parser.add_argument("--entry", type=int, choices=sorted(PROGRAMS), help="select one program (default: all)")
    parser.add_argument("--provider", type=Path, default=ROOT / "build/d3d12-smoke/d3d12_xs.dll")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    with TemporaryDirectory(prefix="persistent-cull-", dir=ROOT / "build") as tmp:
        result = run(args.mount, Host("proton", args.proton, args.prefix), args.adapter.lower(),
                     args.provider, Path(tmp), [args.entry] if args.entry else sorted(PROGRAMS))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
