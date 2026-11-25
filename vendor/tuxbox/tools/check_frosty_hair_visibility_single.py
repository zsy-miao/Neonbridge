#!/usr/bin/env python3
"""Check single-sample and linked-list hair visibility compaction with full GPU readbacks."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import struct
import subprocess
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from frosty_gpu_guest import run_gpu_guest
from check_frosty_hair_visibility_compact import GUARD, words
from check_frosty_shaders import GLOBAL_SHA, containers, decoder, decompress, global_archive
from project_tuxbox.inspect import digest

SHADERS = {979: (4, "695a0af2716be84c7101edcb8970f48f40f2115a6f1e89b74c1f1d74f5dfafbb"),
           982: (8, "f5c3578c399acdb88eb90fd1fd852157a627056799680cb2cc325a84114db424")}

# Tile height, maximum records per pixel, optional velocity output, cooked shader digest.
LINKED = {
    977: (4, 16, False, "606d6db2cb579a1890371e9b8db187975c72afb1f1383e928244bfe4003b0204"),
    978: (4, 32, True, "5e9fc68a16ca959516c2a0b5bbce62c374cc307da14224b080155c2296aa9ab5"),
    981: (4, 32, False, "30ffac7df80e9f748a01f5cdc7e8e822b8d73639413c7fb9a2ce214b2bad524f"),
    984: (4, 8, True, "80b7fe132f20ec8df1358eac0b4580a3af41e1db07f79b06b6be527d53625571"),
    986: (8, 32, True, "561136aa72421ee6faa5ce54a02218dfac236e05ea74db5e5cd84842fd9a0858"),
    992: (8, 8, True, "d9d70ff972d5c7aaacd70b8088be34659c0aa50d46e266fe028a33c6e93341a3"),
    993: (4, 16, True, "c1e0da293b7aa55dae0652a41b1260785b5687dfcfe5d3c413c59e95ab9bf374"),
    994: (8, 8, False, "7c06c970580eeba7dbb2e6323b7cf4b0db061b682c221232120ff5bb2cba880c"),
    995: (4, 8, False, "8033c0b7f1bbe12511a9ca06bfce3674d25c7730b485ae4b414f268144914dde"),
    998: (8, 16, False, "2b04d2a317ccc596409dd819e27112b979542db0dfe24f63ddeb493de623ddd7"),
    1000: (8, 32, False, "dd76d42d47d6347828e966ab5ce6e0b5f1bbc59dd6096808de0be57004d675bf"),
    1004: (8, 16, True, "1546bfd751e7b13c2760bf1118c6dbf6a32504e9e57c9a85e6974e87026de183"),
}


def depth_bits(level):
    return struct.unpack("<I", struct.pack("<f", (level + 1) / 64))[0]


def node_velocity(level):
    return ((-0.25, 0.0625), (0.0625, -0.015625), (-0.015625, 0.25))[level % 3]


def linked_inputs(work, case, height, velocity):
    (work / "linked-list").touch()
    mask = set() if case == "none" else {1, 7, 24, 31, 32, 47, 63}
    length = {"none": 1, "sparse": 1, "multi": 2, "mode2": 2, "limit": 35}[case]
    active = {pixel: length for pixel in mask if pixel < 8 * height}
    cb = [0] * 64
    cb[:2], cb[8] = [8, 8], 0x3f800000
    cb[9] = {"none": 0, "sparse": 1, "multi": 1, "mode2": 2, "limit": 3}[case] if velocity else 0
    words(work / "cb.bin", cb)
    words(work / "t0.bin", [p if p in mask else 0xffffffff for p in range(64)])
    # The linked list walks shallow to deep. Reverse-Z sorting keeps the deepest
    # nodes first. Full opacity gives only the first retained node a nonzero weight.
    nodes = []
    for i in range(64 * length):
        following = i + 64 if i + 64 < 64 * length else 0xffffffff
        x, y = node_velocity(i // 64)
        packed_velocity = struct.unpack("<I", struct.pack("<ee", y, x))[0]
        nodes.extend((depth_bits(i // 64), 0xffffffff, 0x110000 + i,
                      0xff000040 + i, 0x220000 + i, following, packed_velocity))
    words(work / "t1.bin", nodes)
    for slot, data in enumerate(([5], [GUARD] * 64, [GUARD] * 2560, [GUARD] * 1024, [GUARD] * 64)):
        words(work / f"u{slot}.bin", data)
    if velocity:
        words(work / "u5.bin", [GUARD] * 128)
    return active


def inputs(work, case, height):
    cases = {"none": set(), "all": set(range(8 * height)),
             "sparse": {1, 7, 15, 24, 31} | ({32, 47, 60} if height == 8 else set()),
             "last": {8 * height - 1}, "depth": {1, 7, 15, 24, 31}}
    mask = cases[case]
    active = {7, 24} if case == "depth" else mask
    rejected = mask - active
    cb = [0] * 64
    cb[:4], cb[4], cb[8] = [8, 8, 0, 0], 512, 0x3f800000
    words(work / "cb.bin", cb)
    words(work / "t0.bin", [0x3f400000 if i in rejected else 0x3e800000 for i in range(64)])
    words(work / "t1.bin", [x for i in range(64) for x in ((i + 1 if i in mask else 0xffffffff), 0)])
    words(work / "t2.bin", [0x3f000000] * 64)
    words(work / "t3.bin", [0x3f000000] * 64)
    words(work / "t4.bin", [0] * 128)
    words(work / "t5.bin", [0] * 64)
    words(work / "t6.bin", [1])
    words(work / "t7.bin", [0, 0])
    words(work / "u0.bin", [5])
    words(work / "u1.bin", [GUARD] * 64)
    words(work / "u2.bin", [GUARD] * 1024)
    words(work / "u3.bin", [GUARD] * 1024)
    words(work / "u4.bin", [GUARD] * 64)
    return active


def verify(work, active, height, limit=None, velocity_mode=None):
    linked = limit is not None
    def read(slot):
        data = (work / f"out{slot}.bin").read_bytes()
        return struct.unpack(f"<{len(data) // 4}I", data)
    counter, index, record, coords, coverage = (read(i) for i in range(5))
    total = sum(min(n, limit) for n in active.values()) if linked else len(active)
    stride = 5 if linked else 2
    if counter[0] != 5 + total:
        raise ValueError(f"counter {counter[0]}")
    motion = read(5) if velocity_mode is not None else None
    seen = set()
    for pixel in range(64):
        x, y = pixel % 8, pixel // 8
        packed, weight = index[y * 64 + x], coverage[y * 64 + x]
        if motion is not None:
            packed_velocity = motion[y * 64 + 2*x:y * 64 + 2*x + 2]
            if pixel in active:
                samples = [node_velocity(level) for level in range(active[pixel] - 1,
                           max(-1, active[pixel] - limit - 1), -1)]
                if velocity_mode == 1:
                    vector = tuple(sum(component) / len(samples) for component in zip(*samples))
                elif velocity_mode == 3:
                    vector = max(samples, key=lambda v: sum(c*c for c in v))
                else:
                    # In these binaries, mode 2 updates the running maximum
                    # before its strict comparison with the same node depth.
                    # For finite depths it retains the initially zero velocity.
                    vector = (0., 0.)
                expected_velocity = [0.49999237060546875 + math.copysign(math.sqrt(abs(c)) * 0.35284629464149475, c)
                                     for c in vector]
                actual_velocity = struct.unpack("<ff", struct.pack("<II", *packed_velocity))
                if not all(math.isclose(a, e, rel_tol=1e-6, abs_tol=1e-6) for a, e in zip(actual_velocity, expected_velocity)):
                    raise ValueError(f"velocity at pixel {pixel}: {actual_velocity} expected {expected_velocity}")
            elif packed_velocity != (GUARD, GUARD):
                raise ValueError(f"velocity guard at pixel {pixel}: {packed_velocity}")
        if y >= height:
            if (packed, weight) != (GUARD, GUARD):
                raise ValueError(f"untouched pixel {pixel} changed")
        elif pixel not in active:
            if (packed >> 26 if linked else packed, weight) != (0, 0):
                raise ValueError(f"empty pixel {pixel} changed")
        else:
            dst = packed & 0x3ffffff
            count = min(active[pixel], limit) if linked else 1
            if packed >> 26 != count or not 5 <= dst <= 5 + total - count:
                raise ValueError(f"invalid compacted index at pixel {pixel}")
            for sample in range(count):
                output = dst + sample
                if output in seen:
                    raise ValueError(f"overlapping compacted index {output}")
                seen.add(output)
                if linked:
                    level = active[pixel] - 1 - sample
                    node = pixel + 64 * level
                    depth = depth_bits(level)
                    expected = (depth, 0x110000 + node, (0xff000000 if sample == 0 else 0) | (0x40 + node),
                                0x220000 + node, 0xff07ff)
                else:
                    expected = (0x7fffffff, pixel + 1)
                if (record[output * stride:(output + 1) * stride] != expected
                        or coords[output * 2:output * 2 + 2] != (x, y)
                        or weight != (0x3f800000 if linked else 0x3f000000)):
                    raise ValueError(f"incorrect output at pixel {pixel}, sample {sample}")
    for dst in [*range(5), *range(5 + total, 512)]:
        if (record[dst * stride:(dst + 1) * stride] != (GUARD,) * stride
                or coords[dst * 2:dst * 2 + 2] != (GUARD, GUARD)):
            raise ValueError(f"buffer guard overwritten at {dst}")
    return {"active_pixels": len(active), "compacted_nodes": total, "image_pixels_checked": 64, "buffer_slots_checked": 512, "outputs_checked": 6 if motion is not None else 5}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("mount", "xshader", "provider", "proton", "prefix", "compiler-dir", "work", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--linked-list", action="store_true", help="check linked-list visibility inputs and five-word records")
    parser.add_argument("--entries", type=int, nargs="+", help="limit the dispatch to selected archive entries")
    args = parser.parse_args()
    work, proton = args.work.resolve(), args.proton.resolve()
    work.mkdir(parents=True, exist_ok=True)
    archive, entries = global_archive(args.mount)
    os.environ["TUXBOX_XSHADER"] = str(args.xshader.resolve())
    results, adapters, guest_hash = {}, set(), None
    selected = LINKED if args.linked_list else {i: (h, None, False, sha) for i, (h, sha) in SHADERS.items()}
    if args.entries:
        if unknown := set(args.entries) - selected.keys():
            parser.error(f"entries outside the selected hair family: {sorted(unknown)}")
        selected = {i: selected[i] for i in args.entries}
    for entry, (height, limit, velocity, expected) in selected.items():
        ((_, native, _),) = containers(decompress(archive, entries[entry], decoder()))
        if hashlib.sha256(native).hexdigest() != expected:
            raise ValueError(f"unexpected global shader {entry}")
        cases = {}
        names = ["none", "sparse", "multi", "limit"] if args.linked_list else ["none", "all", "sparse", "last", "depth"]
        if velocity:
            names.append("mode2")
        for name in names:
            with TemporaryDirectory(dir=work) as temp:
                trial = Path(temp)
                (trial / "retail.dxbc").write_bytes(native)
                active = linked_inputs(trial, name, height, velocity) if args.linked_list else inputs(trial, name, height)
                exe, status, actual = run_gpu_guest(ROOT / "tools/reference/hair-visibility-single/guest.cpp", "guest.exe",
                                                    trial, work, proton, args.prefix, args.provider, args.compiler_dir)
                if status or actual.get("status") != "ok":
                    raise RuntimeError(f"shader {entry} {name}: {status}, {actual}. See {work / 'gpu.log'}")
                guest_hash = digest(exe)
                adapters.update(line.split("Found device: ", 1)[1] for line in (work / "gpu.log").read_text().splitlines()
                                if "Found device: " in line)
                mode = struct.unpack_from("<I", (trial / "cb.bin").read_bytes(), 36)[0] if velocity else None
                cases[name] = verify(trial, active, height, limit, mode)
        results[entry] = {"cooked_sha256": expected, "workgroup": [8, height, 1], "cases": cases}
    family = "linked-list" if args.linked_list else "single-sample"
    result = {"scope": f"Packaged {family} hair visibility compaction shaders through TuxBox’s guest provider and runtime shader worker, with CPU-modeled GPU readback.",
              "archive_sha256": GLOBAL_SHA, "adapters": sorted(adapters), "proton": (proton / "version").read_text().strip(),
              "xshader_sha256": digest(args.xshader), "provider_sha256": digest(args.provider), "guest_sha256": guest_hash, "shaders": results}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
