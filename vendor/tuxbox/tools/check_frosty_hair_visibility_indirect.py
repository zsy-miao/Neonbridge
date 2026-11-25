#!/usr/bin/env python3
"""Check indirect-tile hair visibility compaction through the Xbox provider."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import struct
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from frosty_gpu_guest import run_gpu_guest
from check_frosty_hair_visibility_compact import GUARD, words
from check_frosty_shaders import GLOBAL_SHA, containers, decoder, decompress, global_archive
from project_tuxbox.inspect import digest

SHADERS = {990: (8, "2cd7b9d37e5a7749f794e3281cb3623f5c7725827d5c8913be0ca6e4fe12f801"),
           996: (4, "2fbe1948ea24cc99f921b815fa8ee2df55ac4b269caf7f19f0ed8a6fdbc75399")}
CASES = ("none", "all", "sparse", "last", "depth", "offset", "notile")


def inputs(trial, case, height):
    local = {"none": set(), "all": set(range(8 * height)),
             "sparse": {1, 7, 15, 24, 31} | ({32, 47, 60} if height == 8 else set()),
             "last": {8 * height - 1}, "depth": {1, 7, 15, 24, 31},
             "offset": {1, 7, 15, 24, 31}, "notile": set()}[case]
    active = {7, 24} if case == "depth" else local
    base = 8 if case == "offset" else 0
    xy = lambda i: (i % 8 + base, i // 8 + base)
    mask, active_xy = {xy(i) for i in local}, {xy(i) for i in active}
    rejected = mask - active_xy
    cb = [0] * 64
    cb[:4], cb[4], cb[8], cb[10], cb[12] = [16, 16, 0, 0], 512, 0x3f800000, 1, 8
    words(trial / "cb.bin", cb)
    pixels = [(i % 16, i // 16) for i in range(256)]
    words(trial / "t0.bin", [0x3f400000 if xy in rejected else 0x3e800000 for xy in pixels])
    words(trial / "t1.bin", [x for i, xy in enumerate(pixels) for x in ((i + 1 if xy in mask else 0xffffffff), 0)])
    words(trial / "t2.bin", [0x3f000000] * 256)
    words(trial / "t3.bin", [0 if case == "notile" else 1])
    words(trial / "t4.bin", [1, 1] if case == "offset" else [0, 0])
    words(trial / "t5.bin", [0x3f000000] * 256)
    words(trial / "t6.bin", [1])
    words(trial / "t7.bin", [0, 0])
    words(trial / "u0.bin", [5])
    words(trial / "u1.bin", [GUARD] * 256)
    words(trial / "u2.bin", [GUARD] * 1024)
    words(trial / "u3.bin", [GUARD] * 1024)
    words(trial / "u4.bin", [GUARD] * 256)
    return active_xy, base


def verify(trial, active, base, height, case):
    def read(slot):
        data = (trial / f"out{slot}.bin").read_bytes()
        return struct.unpack(f"<{len(data) // 4}I", data)
    counter, index, record, coords, coverage = (read(i) for i in range(5))
    if counter[0] != 5 + len(active):
        raise ValueError(f"counter {counter[0]}")
    seen = set()
    for y in range(16):
        for x in range(16):
            xy = (x, y)
            packed, weight = index[y * 64 + x], coverage[y * 64 + x]
            processed = case != "notile" and base <= x < base + 8 and base <= y < base + height
            if not processed:
                if (packed, weight) != (GUARD, GUARD):
                    raise ValueError(f"untouched pixel {xy} changed")
            elif xy not in active:
                if (packed, weight) != (0, 0):
                    raise ValueError(f"empty pixel {xy} changed")
            else:
                dst = packed & 0x3ffffff
                if packed >> 26 != 1 or not 5 <= dst < 5 + len(active) or dst in seen:
                    raise ValueError(f"invalid compacted index at {xy}")
                seen.add(dst)
                if (record[dst * 2:dst * 2 + 2] != (0x7fffffff, y * 16 + x + 1)
                        or coords[dst * 2:dst * 2 + 2] != xy or weight != 0x3f000000):
                    raise ValueError(f"incorrect output at {xy}")
    for dst in [*range(5), *range(5 + len(active), 512)]:
        if record[dst * 2:dst * 2 + 2] != (GUARD, GUARD) or coords[dst * 2:dst * 2 + 2] != (GUARD, GUARD):
            raise ValueError(f"buffer guard overwritten at {dst}")
    return {"active_pixels": len(active), "image_pixels_checked": 256, "buffer_slots_checked": 512, "outputs_checked": 5}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("mount", "xshader", "provider", "proton", "prefix", "compiler-dir", "work", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    work, proton = args.work.resolve(), args.proton.resolve()
    work.mkdir(parents=True, exist_ok=True)
    archive, entries = global_archive(args.mount)
    os.environ["TUXBOX_XSHADER"] = str(args.xshader.resolve())
    results, adapters, guest_hash = {}, set(), None
    for entry, (height, expected) in SHADERS.items():
        ((_, native, _),) = containers(decompress(archive, entries[entry], decoder()))
        if hashlib.sha256(native).hexdigest() != expected:
            raise ValueError(f"unexpected global shader {entry}")
        cases = {}
        for name in CASES:
            with TemporaryDirectory(dir=work) as temp:
                trial = Path(temp)
                (trial / "retail.dxbc").write_bytes(native)
                active, base = inputs(trial, name, height)
                exe, status, actual = run_gpu_guest(ROOT / "tools/reference/hair-visibility-indirect/guest.cpp", "guest.exe",
                                                    trial, work, proton, args.prefix, args.provider, args.compiler_dir)
                if status or actual.get("status") != "ok":
                    raise RuntimeError(f"shader {entry} {name}: {status}, {actual}. See {work / 'gpu.log'}")
                guest_hash = digest(exe)
                adapters.update(line.split("Found device: ", 1)[1] for line in (work / "gpu.log").read_text().splitlines()
                                if "Found device: " in line)
                cases[name] = verify(trial, active, base, height, name)
        results[entry] = {"cooked_sha256": expected, "workgroup": [8, height, 1], "cases": cases}
    result = {"scope": "Two indirect-tile hair visibility compaction shaders through TuxBox’s provider and runtime shader worker, with CPU-modeled GPU readback. Does not validate rendered hair.",
              "archive_sha256": GLOBAL_SHA, "adapters": sorted(adapters), "proton": (proton / "version").read_text().strip(),
              "xshader_sha256": digest(args.xshader), "provider_sha256": digest(args.provider), "guest_sha256": guest_hash, "shaders": results}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
