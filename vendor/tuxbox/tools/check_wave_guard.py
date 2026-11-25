#!/usr/bin/env python3
"""Run synthetic native wave32 and portable wave64 guards on a real GPU.

Build the emitter first:
    cargo build --manifest-path tools/shader-compiler/Cargo.toml
        --target x86_64-unknown-linux-gnu --example wave_guard
The existing D3D12 readback guest checks a guarded UAV view. No game data
is required. Only the translator's input and output are instrumented.
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


def instrument(hlsl, width=32, mode='all', variant='original'):
    if width == 64:
        if mode not in ("values", "state") or "[WaveSize(" in hlsl or "tb_reduction_read(" not in hlsl:
            raise ValueError("fixture must use portable wave64")
        compare, end, label = "0x0050", "0x00ac", "j27"
        if variant != "original":
            end, label = "0x00b4", "j29"
        outputs = ("first", "s7") if mode == "values" else (f"uint({label})", "exec_lo & 1u")
        stores = (f"    output[64u + gtid.x] = {outputs[0]};\n"
                  f"    output[128u + gtid.x] = {outputs[1]};\n")
    else:
        if width != 32 or mode != "all" or "[WaveSize(32)]" not in hlsl:
            raise ValueError("fixture must require wave32")
        compare, end = "0x0044", "0x0094"
        label = "j23"
        if variant != "original":
            end, label = "0x009c", "j25"
        stores = (
            "    output[32u + gtid.x] = first;\n"
            "    output[64u + gtid.x] = s7;\n"
            f"    output[96u + gtid.x] = uint({label});\n"
            "    output[128u + gtid.x] = exec_lo & 1u;\n"
        )
    replacements = {
        "    v0 = gtid.x; v1 = gtid.y; v2 = gtid.z;": (
            "    uint input = output[gtid.x];\n"
            "    exec_lo = input >> 31;\n"
            "    v0 = input & 0x7fffffffu; v1 = gtid.y; v2 = gtid.z;"
        ),
        f"    // {compare} {'s_cmp_eq_u32' if variant == 'original' else 's_mov_b32'}":
            f"    uint first = s7;\n    // {compare} {'s_cmp_eq_u32' if variant == 'original' else 's_mov_b32'}",
        f"    // {end} s_endpgm": stores + f"    // {end} s_endpgm",
    }
    for original, replacement in replacements.items():
        if hlsl.count(original) != 1:
            raise ValueError(f"unexpected fixture: {original}")
        hlsl = hlsl.replace(original, replacement)
    return "RWBuffer<uint> output : register(u0);\n" + hlsl


def cases(width=32):
    rng = random.Random(0x53434152)
    full = (1 << width) - 1
    yield "full", full, list(range(width))
    yield "empty", 0, [123] * width
    yield "odd", full & 0xaaaaaaaaaaaaaaaa, list(range(1, width + 1))
    yield "last", 1 << (width - 1), [91] * width
    yield "zero_sum", full, [0] * width
    yield "wrap_zero", full, [0x40000000] * 4 + [0] * (width - 4)
    yield "wrap_nonzero", full, [0x7fffffff] * width
    yield "lane_zero", 1, [17] * width
    if width == 64:
        yield "upper_half", 0xffffffff00000000, list(range(width))
        yield "lower_half", 0xffffffff, list(range(width))
        yield "two_halves", (1 << 32) | 1, [101] * width
    for i in range(24):
        yield f"random{i:02}", rng.getrandbits(width), [rng.getrandbits(31) for _ in range(width)]


def seed_and_expected(mask, values, mode="all", variant="original"):
    width = len(values)
    seed = [0x76540000 + i for i in range(384)]
    active = [(mask >> lane) & 1 for lane in range(width)]
    seed[96:96 + width] = [v | (a << 31) for v, a in zip(values, active)]
    total = sum(v for v, a in zip(values, active) if a) & 0xffffffff
    skip = total == 0 if variant == "original" else variant == "chain-zero"
    second = 0 if skip and variant == "chain-zero" else total if skip else (total + sum(active)) & 0xffffffff
    expected = seed.copy()
    if mode == "all":
        assert width == 32
        outputs = [total] * width + [second] * width + [int(skip)] * width + active
    elif mode == "values":
        assert width == 64
        outputs = [total] * width + [second] * width
    else:
        assert width == 64 and mode == "state"
        outputs = [int(skip)] * width + active
    expected[96 + width:96 + width + len(outputs)] = outputs
    return struct.pack("<384I", *seed), struct.pack("<384I", *expected)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("emitter", "provider", "proton", "prefix", "compiler-dir", "work", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--adapter", required=True)
    parser.add_argument("--width", type=int, choices=(32, 64), default=32)
    parser.add_argument("--variant", choices=("original", "chain", "chain-zero"), default="original")
    args = parser.parse_args()
    work = args.work.resolve()
    work.mkdir(parents=True, exist_ok=True)
    dxc = Path(find_dxc(None)).resolve()
    emitted = subprocess.check_output([args.emitter.resolve(), str(args.width), args.variant], text=True, timeout=30)
    with TemporaryDirectory(dir=work) as tmp:
        trial = Path(tmp)
        expected = {}
        shaders = {}
        for mode in (("all",) if args.width == 32 else ("values", "state")):
            source = trial / f"{mode}.hlsl"
            source.write_text(instrument(emitted, args.width, mode, args.variant))
            blob = trial / f"{mode}.cso"
            profile = "cs_6_6" if args.width == 32 else "cs_6_0"
            subprocess.run([dxc, "-T", profile, "-E", "main", "-Fo", blob, source], check=True, timeout=30)
            subprocess.run([dxc.with_name("dxv"), blob], check=True, timeout=30)
            shaders[mode] = dict(hlsl_sha256=digest(source), dxil_sha256=digest(blob), profile=profile)
            for case, mask, values in cases(args.width):
                name = f"{mode}_{case}"
                seed, expected[name] = seed_and_expected(mask, values, mode, args.variant)
                (trial / f"{name}.seed").write_bytes(seed)
                (trial / f"{name}.cso").hardlink_to(blob)
        (trial / "cases.txt").write_text("".join(f"{name} 0 uint 1\n" for name in expected))
        exe, status, fields = run_gpu_guest(
            ROOT / "tools/reference/independent-shaders/compute.cpp", "wave-guard.exe",
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
            passed=passed, scope=f"Synthetic native wave{args.width} guarded reductions, instrumented inputs and outputs only",
            guest_wave_width=args.width, variant=args.variant,
            adapter=fields.get("adapter"), proton=(args.proton / "version").read_text().strip(),
            guest_exit=status, cases=len(rows), compared_words=len(rows)*384,
            result_words=len(rows)*128, input_and_guard_words=len(rows)*256,
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
