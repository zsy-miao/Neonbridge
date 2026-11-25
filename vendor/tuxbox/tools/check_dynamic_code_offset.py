#!/usr/bin/env python3
"""GPU differential check of the bounded dynamic embedded-table load.

The native fixtures load their input and store every result through UAV u0.
The older HLSL fixture uses input and output instrumentation.
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
from reference.scarlett_worker_fixture import uav_shader


def instrument(hlsl):
    start = "    // 0x0000 s_getpc_b64"
    end = "    // 0x0014 s_endpgm"
    if hlsl.count(start) != 1 or hlsl.count(end) != 1 or "tb_code[t1 + 0u]" not in hlsl:
        raise ValueError("unexpected translated fixture")
    hlsl = hlsl.replace(start, "    s12 = output[0];\n" + start)
    hlsl = hlsl.replace(end, "    output[64u + gtid.x] = s16;\n" + end)
    return "RWBuffer<uint> output : register(u0);\n" + hlsl


# gfx1030 instructions between GETPC and the table load. These sequences
# occupy three dwords, including the literal operand of S_BFE_U32.
SELECTORS = {
    "or":  [0x870a830c, 0x8f0a820a, 0x880a900a],  # ((s12 & 3) << 2) | 16
    "xor": [0x870a830c, 0x8f0a820a, 0x890a940a],  # ((s12 & 3) << 2) ^ 20
    "cselect": [0xbf0d830c, 0x850a8184, 0x8f0a820a],  # bit 3 selects word 4 or 1
    "bfe": [0x938aff0c, 0x0002000b, 0x8f0a820a],  # extract bits 11..12, then scale by 4
}


def native_fixture(pattern):
    code = [
        0xf4080402, 0xfa000000, 0xbf8cc07f,  # load the u0 descriptor and wait
        0xe0300000, 0x80040100, 0xbf8c3f70,  # read the first input word and wait
        0x7e180501, 0xbe881f00,               # readfirstlane s12,v1; getpc s[8:9]
        *SELECTORS[pattern],
        0xf4000584, 0x1400002c, 0xbf8cc07f,  # s22 = table at saved PC + 44 + s10
        0x7e080216, 0x340a0082,              # move s22 to v4; byte offset = lane * 4
        0xe0701100, 0x80040405, 0xbf810000,  # store at byte 256 + lane * 4; endpgm
    ]
    table = [11 * (word + 1) for word in range(8)]
    return uav_shader(code + table)


def cases(pattern=None):
    rng = random.Random(0x53434152)
    values = [0, 1, 2, 3, 4, 7, 0xffffffff, 0x80000002]
    values += [rng.getrandbits(32) for _ in range(56)]
    for i, value in enumerate(values):
        seed = [0x76540000 + n for n in range(384)]
        seed[96] = value
        expected = seed.copy()
        index = value & 3
        if pattern == "or":
            index += 4
        elif pattern == "xor":
            index = (5, 4, 7, 6)[index]
        elif pattern == "cselect":
            index = 4 if value & 8 else 1
        elif pattern == "bfe":
            index = (value >> 11) & 3
        expected[160:224] = [11 * (index + 1)] * 64
        yield f"offset_{i:02}", struct.pack("<384I", *seed), struct.pack("<384I", *expected)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    inputs = parser.add_mutually_exclusive_group(required=True)
    inputs.add_argument("--hlsl", type=Path, help="existing mask-and-shift fixture")
    inputs.add_argument("--xshader", type=Path, help="translator for a complete native fixture")
    parser.add_argument("--pattern", choices=SELECTORS)
    for name in ("provider", "proton", "prefix", "compiler-dir", "work", "output"):
        parser.add_argument("--" + name, required=True, type=Path)
    parser.add_argument("--adapter", required=True)
    args = parser.parse_args()
    if bool(args.xshader) != bool(args.pattern):
        parser.error("--xshader and --pattern must be used together")
    work = args.work.resolve()
    work.mkdir(parents=True, exist_ok=True)
    dxc = Path(find_dxc(None)).resolve()
    with TemporaryDirectory(dir=work) as tmp:
        trial = Path(tmp)
        source = trial / "table.hlsl"
        profile = "cs_6_0"
        if args.xshader:
            native = trial / "table.dxbc"
            native.write_bytes(native_fixture(args.pattern))
            profile_file = trial / "profile"
            subprocess.run([args.xshader.resolve(), native, "--hlsl-out", source,
                            "--profile-out", profile_file], check=True, timeout=30)
            profile = profile_file.read_text().strip()
        else:
            source.write_text(instrument(args.hlsl.read_text()))
        blob = trial / "table.cso"
        subprocess.run([dxc, "-T", profile, "-E", "main", "-Fo", blob, source], check=True, timeout=30)
        subprocess.run([dxc.with_name("dxv"), blob], check=True, timeout=30)
        expected = {}
        for name, seed, reference in cases(args.pattern):
            (trial / f"{name}.seed").write_bytes(seed)
            (trial / f"{name}.cso").hardlink_to(blob)
            expected[name] = reference
        view = "raw" if args.xshader else "uint"
        (trial / "cases.txt").write_text("".join(f"{name} 0 {view} 1\n" for name in expected))
        exe, status, fields = run_gpu_guest(
            ROOT / "tools/reference/independent-shaders/compute.cpp", "dynamic-code-offset.exe",
            trial, work, args.proton.resolve(), args.prefix, args.provider, args.compiler_dir)
        mismatches = [name for name, ref in expected.items()
                      if not (trial / f"{name}.bin").is_file() or (trial / f"{name}.bin").read_bytes() != ref]
        report = dict(passed=status == 0 and fields.get("adapter") == args.adapter and not mismatches,
                      scope=("Synthetic native table indexing with UAV input and output" if args.xshader
                             else "Synthetic runtime table indexing, input/output instrumentation only"),
                      adapter=fields.get("adapter"), cases=len(expected), compared_words=len(expected)*384,
                      result_words=len(expected)*64, mismatches=mismatches, guest_exit=status,
                      emitter_hlsl_sha256=digest(args.hlsl or source), dxil_sha256=digest(blob),
                      dxc_sha256=digest(dxc), guest_sha256=digest(exe),
                      provider_sha256=digest(args.provider))
        if args.xshader:
            report.update(pattern=args.pattern, xshader_sha256=digest(args.xshader),
                          native_sha256=digest(native), shader_profile=profile)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2) + "\n")
    if not report["passed"]:
        raise SystemExit("GPU differential check failed")
    print(f"PASS: {report['cases']} cases, {report['compared_words']} words")

if __name__ == "__main__":
    main()
