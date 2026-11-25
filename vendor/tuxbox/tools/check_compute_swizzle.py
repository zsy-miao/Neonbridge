#!/usr/bin/env python3
"""Run synthetic compute DS_SWIZZLE_B32 programs on a real GPU.

Build the emitter first:
    cargo build --manifest-path tools/shader-compiler/Cargo.toml
        --target x86_64-unknown-linux-gnu --example compute_swizzle
Each program is the translator's output for one swizzle offset. The existing
D3D12 readback guest checks a guarded UAV view. No game data is required.
Only the translator's input and output are instrumented.
"""
import argparse
import json
import random
import struct
import subprocess
import sys
from pathlib import Path
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import find_dxc
from frosty_gpu_guest import run_gpu_guest
from project_tuxbox.inspect import digest

LANES = 64
VIEW = 96  # first word of the guest's UAV view
UNTOUCHED = 0x5a5a0000

# QDMode: identity, reverse, swap pairs, swap halves, broadcast lanes 0 and 3, irregular.
QUAD = [0x80e4, 0x801b, 0x80b1, 0x804e, 0x8000, 0x80ff, 0x8039, 0x80d2]
# Bit-mask mode regression: swap neighbours, reverse 32, broadcast lanes 0 and 31, and/or/xor mix.
BITMASK = [0x041f, 0x7c1f, 0x0000, 0x03e0, 0x2a73]


def partner(offset, lane):
    if offset & 0x8000:
        return (lane & ~3) | ((offset >> (2 * (lane & 3))) & 3)
    return (lane & ~31) | (((lane & (offset & 31)) | ((offset >> 5) & 31)) ^ ((offset >> 10) & 31))


def instrument(hlsl):
    replacements = {
        "    v0 = gtid.x; v1 = gtid.y; v2 = gtid.z;": (
            "    uint input = output[gtid.x];\n"
            "    exec_lo = input >> 31;\n"
            f"    v0 = {UNTOUCHED:#x}u | gtid.x; v1 = input & 0x7fffffffu; v2 = gtid.z;"
        ),
        "    // 0x000c s_endpgm": f"    output[{LANES}u + gtid.x] = v0;\n    // 0x000c s_endpgm",
    }
    for original, replacement in replacements.items():
        if hlsl.count(original) != 1:
            raise ValueError(f"unexpected fixture: {original}")
        hlsl = hlsl.replace(original, replacement)
    return "RWBuffer<uint> output : register(u0);\n" + hlsl


def masks():
    rng = random.Random(0x5357495a)
    full = (1 << LANES) - 1
    yield "full", full
    yield "odd", full & 0xaaaaaaaaaaaaaaaa
    yield "lane_zero", 1
    yield "empty", 0
    for i in range(4):
        yield f"random{i}", rng.getrandbits(LANES)


def seed_and_expected(offset, mask, values):
    seed = [0x76540000 + i for i in range(384)]
    active = [(mask >> lane) & 1 for lane in range(LANES)]
    seed[VIEW:VIEW + LANES] = [v | (a << 31) for v, a in zip(values, active)]
    expected = seed.copy()
    for lane in range(LANES):
        source = partner(offset, lane)
        if active[lane]:
            result = values[source] if active[source] else 0
        else:
            result = UNTOUCHED | lane
        expected[VIEW + LANES + lane] = result
    return struct.pack("<384I", *seed), struct.pack("<384I", *expected)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("emitter", "provider", "proton", "prefix", "compiler-dir", "work", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--adapter", required=True)
    args = parser.parse_args()
    work = args.work.resolve()
    work.mkdir(parents=True, exist_ok=True)
    dxc = Path(find_dxc(None)).resolve()
    rng = random.Random(0x51554144)
    with TemporaryDirectory(dir=work) as tmp:
        trial = Path(tmp)
        expected = {}
        shaders = {}
        for offset in QUAD + BITMASK:
            emitted = subprocess.check_output([args.emitter.resolve(), f"{offset:04x}"], text=True, timeout=30)
            source = trial / f"swizzle_{offset:04x}.hlsl"
            source.write_text(instrument(emitted))
            blob = trial / f"swizzle_{offset:04x}.cso"
            subprocess.run([dxc, "-T", "cs_6_0", "-E", "main", "-Fo", blob, source], check=True, timeout=30)
            subprocess.run([dxc.with_name("dxv"), blob], check=True, timeout=30)
            shaders[f"{offset:#06x}"] = dict(hlsl_sha256=digest(source), dxil_sha256=digest(blob))
            for case, mask in masks():
                name = f"swizzle_{offset:04x}_{case}"
                values = [rng.getrandbits(31) for _ in range(LANES)]
                seed, expected[name] = seed_and_expected(offset, mask, values)
                (trial / f"{name}.seed").write_bytes(seed)
                (trial / f"{name}.cso").hardlink_to(blob)
        (trial / "cases.txt").write_text("".join(f"{name} 0 uint 1\n" for name in expected))
        exe, status, fields = run_gpu_guest(
            ROOT / "tools/reference/independent-shaders/compute.cpp", "compute-swizzle.exe",
            trial, work, args.proton.resolve(), args.prefix, args.provider, args.compiler_dir,
        )
        rows = []
        for name, reference in expected.items():
            path = trial / f"{name}.bin"
            actual = path.read_bytes() if path.is_file() else b""
            rows.append(dict(case=name, matched=actual == reference,
                             readback_sha256=digest(path) if path.is_file() else None))
        passed = status == 0 and fields.get("adapter") == args.adapter and all(row["matched"] for row in rows)
        report = dict(
            passed=passed,
            scope="Synthetic compute ds_swizzle_b32 in QDMode and bit-mask mode, instrumented inputs and outputs only",
            quad_offsets=[f"{o:#06x}" for o in QUAD], bitmask_offsets=[f"{o:#06x}" for o in BITMASK],
            adapter=fields.get("adapter"), proton=(args.proton / "version").read_text().strip(),
            guest_exit=status, cases=len(rows), compared_words=len(rows) * 384,
            result_words=len(rows) * LANES, input_words=len(rows) * LANES,
            emitter_sha256=digest(args.emitter), shaders=shaders,
            dxc_sha256=digest(dxc), guest_sha256=digest(exe),
            d3d12_dlls={name: digest(trial / name) for name in ("d3d12.dll", "d3d12core.dll")},
            rows=rows,
        )
        if fields.get("error"):
            report["error"] = fields["error"]
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2) + "\n")
    if not passed:
        raise SystemExit(f"GPU readback failed: {args.output}")
    print(f"PASS: {report['cases']} cases, {report['compared_words']} words, adapter {report['adapter']}")


if __name__ == "__main__":
    main()
