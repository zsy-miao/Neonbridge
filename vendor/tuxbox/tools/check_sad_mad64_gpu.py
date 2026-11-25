#!/usr/bin/env python3
"""LLVM-assembled SAD and 64-bit multiply-add controls on a D3D12 GPU.

v_sad_u8, v_sad_hi_u8, v_sad_u16, v_sad_u32, and v_mad_i64_i32 and
v_mad_u64_u32 with the null carry destination LLVM emits when the carry
is unused. Every destination holds a stale value beforehand, and two
forms accumulate in place into their own destination.
"""
import argparse
import hashlib
import json
from pathlib import Path
import random
import re
import struct
import subprocess
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
from check_packed_integer_gpu import assemble, digest, load_input, translate
from frosty_gpu_guest import run_gpu_guest

# v4, v5: multiplicands / SAD operands; v6: SAD accumulator and low addend word; v7: high addend word.
INIT = (0x13572468, 0x24681357, 0x35792468, 0x46813579)
# The guest's guarded view covers words 96..288: twelve groups of 15 words.
GROUPS, STRIDE, START = 12, 15, 96
OUTPUTS = 11
PROGRAM = [
    "v_sad_u8 v10, v4, v5, v6",
    "v_sad_hi_u8 v11, v4, v5, v6",
    "v_sad_u16 v12, v4, v5, v6",
    "v_sad_u32 v13, v4, v5, v6",
    "v_mad_i64_i32 v[14:15], null, v4, v5, v[6:7]",
    "v_mad_u64_u32 v[16:17], null, v4, v5, v[6:7]",
    # Accumulate in place, as compilers emit for running sums.
    "v_mov_b32_e32 v18, v6", "v_mov_b32_e32 v19, v7",
    "v_mad_i64_i32 v[18:19], null, v4, v5, v[18:19]",
    "v_mov_b32_e32 v20, v6",
    "v_sad_u8 v20, v4, v5, v20",
]
EDGE = [0, 1, 0xffffffff, 0x80000000, 0x7fffffff, 0x00ff00ff, 0xff00ff00, 0x80808080,
        0x7f7f7f7f, 0xffff0000, 0x0000ffff, 0x01fe7f80]


def signed(x):
    return x - (1 << 32) if x >> 31 else x


def sad(a, b, width):
    mask = (1 << width) - 1
    return sum(abs((a >> s & mask) - (b >> s & mask)) for s in range(0, 32, width))


def expected_outputs(a, b, c, ch):
    c64 = c | ch << 32
    i64 = (signed(a) * signed(b) + c64) % 2**64
    u64 = (a * b + c64) % 2**64
    return [(sad(a, b, 8) + c) % 2**32, ((sad(a, b, 8) << 16) + c) % 2**32,
            (sad(a, b, 16) + c) % 2**32, (abs(a - b) + c) % 2**32,
            i64 & 0xffffffff, i64 >> 32, u64 & 0xffffffff, u64 >> 32,
            i64 & 0xffffffff, i64 >> 32, (sad(a, b, 8) + c) % 2**32]


def instrument(text):
    marker = "[numthreads(1, 1, 1)]"
    if text.count(marker) != 1 or not text.rstrip().endswith("}"):
        raise ValueError("unexpected compute entry")
    text = text.replace(marker, "RWStructuredBuffer<uint> output : register(u0);\n" + marker)
    for j, v in enumerate(INIT):
        text = load_input(text, 4 + j, v, f"output[{STRIDE}u * gid.x + {j}u]")
    stores = "".join(f"    output[{STRIDE}u * gid.x + {len(INIT) + i}u] = v{10 + i};\n" for i in range(OUTPUTS))
    return text.rstrip()[:-1] + stores + "}\n"


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    for n in ("emitter", "llvm-mc", "dxc", "dxv", "provider", "proton", "prefix", "compiler-dir", "work", "output"):
        ap.add_argument("--" + n, type=Path, required=True)
    ap.add_argument("--baseline", type=Path, help="earlier emitter that must refuse the program")
    ap.add_argument("--adapter", required=True)
    ap.add_argument("--dispatches", type=int, default=24)
    a = ap.parse_args()
    if a.output.exists():
        ap.error("choose a fresh receipt")
    a.work.mkdir(parents=True, exist_ok=False)
    rng = random.Random(0x5341444d)
    tool_hashes = {n: digest(getattr(a, n)) for n in ("emitter", "dxc", "dxv", "provider")}
    if a.baseline:
        tool_hashes["baseline"] = digest(a.baseline)
    lines = [f"v_mov_b32_e32 v{4 + j}, 0x{v:x}" for j, v in enumerate(INIT)]
    lines += [f"v_mov_b32_e32 v{reg}, 0x{0xdead0000 + reg:x}" for reg in range(10, 10 + OUTPUTS)]
    lines += PROGRAM + ["s_endpgm"]
    words = assemble(lines, a.llvm_mc)
    if a.baseline:
        before = translate(a.baseline, words)
        if before.returncode != 65:
            raise ValueError(f"baseline did not refuse: {before.returncode} {before.stderr}")
    got = translate(a.emitter, words)
    got.check_returncode()
    source = instrument(got.stdout)
    rows, expected, negatives = [], {}, []
    with TemporaryDirectory(dir=a.work) as td:
        trial = Path(td)
        guest = ROOT / "tools/reference/independent-shaders/compute.cpp"
        (trial / "guest.cpp").write_text(guest.read_text())
        (trial / "context.h").write_text((guest.parent / "context.h").read_text().replace(
            '"../compute_buffers.h"', '"' + str(ROOT / "tools/reference/compute_buffers.h") + '"'))
        for k in range(a.dispatches):
            label = f"sad_mad64_{k:02}"
            shader = trial / (label + ".hlsl")
            shader.write_text(source)
            blob = shader.with_suffix(".cso")
            subprocess.run([a.dxc, "-T", "cs_6_0", "-E", "main", "-O3", "-Fo", blob, shader],
                           check=True, capture_output=True, timeout=60)
            subprocess.run([a.dxv, blob], check=True, capture_output=True, timeout=15)
            seed = [rng.getrandbits(32) for _ in range(384)]
            want = seed.copy()
            for lane in range(GROUPS):
                if k < 4:
                    vals = [EDGE[(k * GROUPS + lane + 5 * j) % len(EDGE)] for j in range(4)]
                else:
                    vals = [rng.getrandbits(32) for _ in range(4)]
                offset = START + STRIDE * lane
                seed[offset:offset + 4] = vals
                want[offset:offset + 4] = vals
                want[offset + 4:offset + STRIDE] = expected_outputs(*vals)
            (trial / (label + ".seed")).write_bytes(struct.pack("<384I", *seed))
            expected[label] = struct.pack("<384I", *want)
            rows.append({"case": label, "dxil_sha256": digest(blob)})
        for kind, inst in [("clamp", "v_sad_u8 v10, v4, v5, v6 clamp"),
                           ("scalar_source", "v_sad_u32 v10, s4, v5, v6"),
                           ("carry_destination", "v_mad_i64_i32 v[14:15], vcc_lo, v4, v5, v[6:7]"),
                           ("masked_sad", "v_msad_u8 v10, v4, v5, v6")]:
            bad = translate(a.emitter, assemble([inst, "s_endpgm"], a.llvm_mc))
            if bad.returncode != 65:
                raise ValueError(f"unexpected acceptance {inst}")
            negatives.append({"kind": kind, "assembly": inst, "refusal": bad.stderr.strip()})
        (trial / "cases.txt").write_text("".join(f"{n} 4 structured {GROUPS}\n" for n in expected))
        exe, status, fields = run_gpu_guest(trial / "guest.cpp", "sadmad64.exe", trial, a.work, a.proton,
                                            a.prefix, a.provider, a.compiler_dir)
        if status or fields.get("adapter") != a.adapter:
            raise RuntimeError(f"guest failed: {status} {fields}")
        failures = []
        for row in rows:
            actual = trial / (row["case"] + ".bin")
            observed = actual.read_bytes() if actual.is_file() else b""
            if observed != expected[row["case"]]:
                lhs = struct.unpack(f"<{len(observed) // 4}I", observed)
                rhs = struct.unpack("<384I", expected[row["case"]])
                failures.append({"case": row["case"], "different_words": [
                    (i, f"{x:08x}", f"{y:08x}") for i, (x, y) in enumerate(zip(lhs, rhs)) if x != y][:16]})
            else:
                row["readback_sha256"] = digest(actual)
        receipt = {"scope": "Synthetic LLVM-assembled SAD and 64-bit multiply-add instructions; dynamic instrumented inputs and observed outputs",
                   "adapter": fields["adapter"], "proton": (a.proton / "version").read_text().strip(),
                   "seed": 0x5341444d, "assembly": lines, "native_words": [f"{v:08x}" for v in words],
                   "emitted_sha256": hashlib.sha256(got.stdout.encode()).hexdigest(),
                   "shader_cases": len(rows), "inputs": len(rows) * GROUPS,
                   "result_words": len(rows) * GROUPS * OUTPUTS, "compared_words": 384 * len(rows),
                   "mismatches": len(failures), "failures": failures,
                   "baseline_refused": bool(a.baseline), "negative_controls": negatives,
                   "tool_sha256": tool_hashes, "guest_sha256": digest(exe), "rows": rows}
    with a.output.open("x") as f:
        json.dump(receipt, f, indent=2)
        f.write("\n")
    if failures:
        print(f"FAIL {len(failures)} of {len(rows)} dispatches; first: {failures[0]}")
        raise SystemExit(1)
    print(f"PASS {receipt['inputs']} inputs, {receipt['result_words']} results, {receipt['compared_words']} GPU words; zero mismatches")


if __name__ == "__main__":
    main()
