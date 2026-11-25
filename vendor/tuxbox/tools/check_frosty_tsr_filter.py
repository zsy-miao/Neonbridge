#!/usr/bin/env python3
"""Dispatch Frosty's packaged temporal super-resolution frequency filter and check its output texels."""
import argparse
import json
import math
from pathlib import Path
import shutil
import struct
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
import frosty_tsr_history as history
from build import Toolchain
from check_frosty_shaders import GLOBAL_SHA, containers, decompress, global_archive
from project_tuxbox.host import Host
from project_tuxbox.inspect import digest
from project_tuxbox.runtime_shaders import shader_session
from project_tuxbox.shader_inventory import decoder
from project_tuxbox.shader_prep import compiler_tools

ENTRY = 2653
SOURCE = "d70b3993585759774396d39ec7471a3a598a80801d986a29265c6f5f47b73b6e"
CASES = ("zero", "flat", "reverse", "pattern", "clip")
SIZE = 32
SENTINEL = -19.0
INPUT = ((1, 2, 4, 8), (8, 4, 2, 1))
FLAT = ((1, 2, 4, 1), (8, 4, 2, 8))


def gain(x, y):
    return 1 + min(max(x, 0), SIZE - 1) // 8 + 2 * (min(max(y, 0), SIZE - 1) // 8)


def prepare(trial):
    for case in CASES:
        path = trial / case
        path.mkdir()
        b0 = [0] * 1024
        b0[38:40] = (29, 27) if case == "clip" else (SIZE, SIZE)
        b0[112:116] = (0, 0, SIZE - 1, SIZE - 1)
        (path / "b0").write_bytes(struct.pack("<1024I", *b0))
        (path / "b1").write_bytes(bytes(4096))
        for slot, channels in enumerate((4, 4, 1)):
            data = []
            for y in range(SIZE):
                for x in range(SIZE):
                    if case == "zero":
                        data.extend([0] * channels)
                    elif slot == 2:
                        data.append(1)
                    else:
                        scale = gain(x, y) if case in ("pattern", "clip") else 1
                        color = INPUT[1 - slot if case == "reverse" else slot]
                        data.extend(scale * v for v in color)
            (path / f"t{slot}").write_bytes(struct.pack(f"<{len(data)}f", *data))
            (path / f"u{slot}").write_bytes(struct.pack("<f", SENTINEL) * len(data))
    (trial / "cases.txt").write_text("\n".join(CASES) + "\n")


def verify(trial):
    images = {}
    results = {}
    for case in CASES:
        path = trial / case
        surfaces = []
        for slot, channels in enumerate((4, 4, 1)):
            file = path / f"out{slot}"
            rows = list(struct.iter_unpack("<" + "f" * channels, file.read_bytes()))
            if len(rows) != SIZE * SIZE:
                raise ValueError(f"{case} u{slot} has {len(rows)} texels")
            images[case, slot] = rows
            for i, row in enumerate(rows):
                x, y = i % SIZE, i // SIZE
                if case == "zero":
                    ok = row == (0,) * channels
                elif case == "clip":
                    expected = images["pattern", slot][i] if x < 29 and y < 27 else (SENTINEL,) * channels
                    ok = row == expected
                elif case == "reverse" and slot < 2:
                    ok = row == images["flat", 1 - slot][i]
                elif slot == 2:
                    ok = 0 < row[0] < 1
                    if case != "pattern":
                        ok &= row == rows[0]
                else:
                    # Each output color lies within the neighborhood of its input.
                    # The spatially constant neighborhoods also give exact reference colors up to FP16 rounding.
                    lo, hi = (gain(x - 4, y - 4), gain(x + 4, y + 4)) if case == "pattern" else (1, 1)
                    ok = all(lo * 0.995 <= v / base <= hi * 1.005 for v, base in zip(row, FLAT[slot]))
                if not ok or not all(map(math.isfinite, row)):
                    raise ValueError(f"{case} u{slot} failed at ({x},{y}): {row}")
            distinct = len(set(rows))
            if case == "pattern" and distinct <= (1 if slot == 2 else 10):
                raise ValueError(f"Pattern u{slot} produced only {distinct} distinct output values")
            if case == "reverse" and slot == 2 and rows[0] == images["flat", slot][0]:
                raise ValueError("Swapping the input colors did not change the filter weight")
            surfaces.append(dict(sha256=digest(file), distinct_texels=distinct, verified_texels=len(rows)))
        results[case] = dict(outputs=surfaces)
    return results


def run(mount, host, adapter, provider, trial, update_history=False):
    entry = history.ENTRY if update_history else ENTRY
    source_sha = history.SOURCE if update_history else SOURCE
    cases = history.CASES if update_history else CASES
    archive, entries = global_archive(mount)
    ((_, shader, _),) = containers(decompress(archive, entries[entry], decoder()))
    source = trial / "filter.dxbc"
    source.write_bytes(shader)
    if digest(source) != source_sha:
        raise ValueError("Unexpected packaged TSR shader")
    (history.prepare if update_history else prepare)(trial)
    tools = Toolchain()
    flags = ["/DTUXBOX_TSR_UPDATE_HISTORY"] if update_history else []
    obj = tools.compile(ROOT / "tools/reference/tsr-filter/guest.cpp", trial, "/W4", "/WX", *flags, f"/I{ROOT / 'runtime/d3d12'}")
    exe = trial / "filter.exe"
    tools.link(exe, obj, str(ROOT / "build/d3d12-smoke/d3d12_xs.lib"), "d3d12.lib", "dxgi.lib", "kernel32.lib")
    shutil.copyfile(provider, trial / "d3d12_xs.dll")
    compiled = []
    cache = trial / "temp/tuxbox_shader_cache"
    with shader_session(cache, host.env, compiled.append):
        session = cache / host.env["TUXBOX_SHADER_SESSION"]
        result = host.run(exe, ["--adapter", adapter, "--report", "result.txt"], capture=True, timeout=150)
        receipt = trial / "result.txt"
        line = receipt.read_text().strip() if receipt.exists() else ""
        if result.returncode or line != f"tsr-filter readbacks {len(cases)}":
            sys.stderr.buffer.write(result.stdout[-4000:])
            raise ValueError(f"TSR dispatch failed: {line}, compilation: {compiled}")
        if compiled != [f"Compiled Xbox shader {source_sha}"]:
            raise ValueError(f"Unexpected runtime compilation: {compiled}")
        dxil = digest(session / (source_sha + ".cso"))
    shader = "TSRUpdateHistory" if update_history else "TSRFilterFrequencies"
    return dict(name=f"F{shader}CS", shader_file=f"/Engine/Private/TemporalSuperResolution/{shader}.usf",
                entry=entry, shader_map=281 if update_history else 217, adapter=adapter, host=host.version, global_archive_sha256=GLOBAL_SHA,
                source_sha256=source_sha, dxil_sha256=dxil, guest_sha256=digest(exe), provider_sha256=digest(provider),
                compiler_sha256=digest(compiler_tools()[0]), dispatch_groups=[2, 2, 1], threads_per_group=[8, 4, 1] if update_history else [16, 16, 1],
                texture_dimensions=[SIZE, SIZE], cases=(history.verify if update_history else verify)(trial))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mount", type=Path)
    parser.add_argument("--proton", type=Path)
    parser.add_argument("--prefix", type=Path)
    parser.add_argument("--adapter", required=True)
    parser.add_argument("--provider", type=Path, default=ROOT / "build/d3d12-smoke/d3d12_xs.dll")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--update-history", action="store_true", help="check the city flyover history update at global entry 3385")
    args = parser.parse_args()
    with TemporaryDirectory(prefix="tsr-filter-", dir=ROOT / "build") as tmp:
        result = run(args.mount, Host("proton", args.proton, args.prefix), args.adapter.lower(), args.provider, Path(tmp), args.update_history)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
