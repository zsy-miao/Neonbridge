#!/usr/bin/env python3
"""LLVM-assembled exact f64 rounding and conversion controls on a D3D12 GPU.

v_trunc/floor/ceil/rndne_f64 and conversions between f64 and 32-bit
integers, in VOP1 and VOP3 encodings, against an independent model built
on Python floats. One variant first sets s_denorm_mode 0 so f64 input
denormals are flushed. Each destination high register holds a stale
value beforehand, and one form aliases its source pair.
"""
import argparse
import hashlib
import json
import math
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

INIT = (0x13572468, 0x24681357)
# The guest's guarded view covers words 96..288: eight groups of 18 words.
GROUPS, STRIDE, START = 8, 18, 96
ROUNDING = ("v_trunc_f64", "v_floor_f64", "v_ceil_f64", "v_rndne_f64")
# (instruction, destination register, destination words)
FORMS = [(name, 10 + 2 * i, 2) for i, name in enumerate(ROUNDING)] + [
    ("v_cvt_i32_f64", 18, 1), ("v_cvt_u32_f64", 19, 1),
    ("v_cvt_f64_i32", 20, 2), ("v_cvt_f64_u32", 22, 2)]
QUIET = 0x0008_0000_0000_0000


def bits(x):
    return struct.unpack("<Q", struct.pack("<d", x))[0]


def value(b):
    return struct.unpack("<d", struct.pack("<Q", b))[0]


def edge_doubles():
    xs = [0.0, 0.5, 1.5, 2.5, 3.5, 0.49999999999999994, 1 - 2**-53, 2**52 - 0.5, 2**52 + 1, 2**53,
          2**31, 2**31 - 0.5, 2**31 + 0.5, 2**32 - 0.5, 2**32, 4294967295.75, 1e300, 123456.789,
          7.5, 1e-300, 2**-1022, math.inf, 1.0, 2**51 + 0.5, 2**51 + 1.5]
    out = []
    for x in xs:
        out += [bits(x), bits(-x)]
    out += [1, 0x000f_ffff_ffff_ffff, 0x8000_0000_0000_0001, 0x7ff8_0000_0000_0000,
            0x7ff0_0000_0000_0001, 0xfff4_0000_dead_beef, bits(-(2**31) - 1), bits(-(2**31) - 0.5),
            0xffff_ffff_8000_0000, 0x7fff_ffff_0000_0000, 0x0000_0001_0000_0001, 0x8000_0000_ffff_ffff]
    return out


def reference(name, b, keep_denormals=True):
    lo, hi = b & 0xffffffff, b >> 32
    if name == "v_cvt_f64_i32":
        return bits(float(lo - (1 << 32) if lo >> 31 else lo))
    if name == "v_cvt_f64_u32":
        return bits(float(hi))
    x = value(b)
    if math.isnan(x):
        return (b | QUIET) if name in ROUNDING else 0
    if name in ROUNDING:
        if math.isinf(x):
            return b
        if (b >> 52) & 0x7ff == 0 and not keep_denormals:
            x = math.copysign(0.0, x)
        rounded = {"v_trunc_f64": math.trunc, "v_floor_f64": math.floor,
                   "v_ceil_f64": math.ceil, "v_rndne_f64": round}[name](x)
        return bits(math.copysign(float(rounded), x) if rounded == 0 else float(rounded))
    low, high = (-(2**31), 2**31 - 1) if name == "v_cvt_i32_f64" else (0, 2**32 - 1)
    if math.isinf(x):
        return (high if x > 0 else low) % 2**32
    return max(low, min(high, math.trunc(x))) % 2**32


def program(e64, flush):
    suffix = "_e64" if e64 else "_e32"
    lines = [f"v_mov_b32_e32 v{4 + j}, 0x{v:x}" for j, v in enumerate(INIT)]
    if flush:
        lines.append("s_denorm_mode 0")
    # Stale destination values; the translator must replace both words.
    lines += [f"v_mov_b32_e32 v{reg}, 0x{0xdead0000 + reg:x}" for reg in range(10, 26)]
    for name, dst, width in FORMS:
        target = f"v[{dst}:{dst + 1}]" if width == 2 else f"v{dst}"
        source = "v4" if name == "v_cvt_f64_i32" else "v5" if name == "v_cvt_f64_u32" else "v[4:5]"
        lines.append(f"{name}{suffix} {target}, {source}")
    lines += ["v_mov_b32_e32 v24, v4", "v_mov_b32_e32 v25, v5",
              f"v_rndne_f64{suffix} v[24:25], v[24:25]", "s_endpgm"]
    return lines


def expected_outputs(b, flush):
    out = []
    for name, _, width in FORMS:
        r = reference(name, b, not flush)
        out += [r & 0xffffffff, r >> 32] if width == 2 else [r]
    r = reference("v_rndne_f64", b, not flush)
    return out + [r & 0xffffffff, r >> 32]


def instrument(text):
    marker = "[numthreads(1, 1, 1)]"
    if text.count(marker) != 1 or not text.rstrip().endswith("}"):
        raise ValueError("unexpected compute entry")
    text = text.replace(marker, "RWStructuredBuffer<uint> output : register(u0);\n" + marker)
    for j, v in enumerate(INIT):
        text = load_input(text, 4 + j, v, f"output[{STRIDE}u * gid.x + {j}u]")
    stores = "".join(f"    output[{STRIDE}u * gid.x + {2 + i}u] = v{10 + i};\n" for i in range(16))
    return text.rstrip()[:-1] + stores + "}\n"


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    for n in ("emitter", "llvm-mc", "dxc", "dxv", "provider", "proton", "prefix", "compiler-dir", "work", "output"):
        ap.add_argument("--" + n, type=Path, required=True)
    ap.add_argument("--baseline", type=Path, help="earlier emitter that must refuse every program")
    ap.add_argument("--adapter", required=True)
    ap.add_argument("--dispatches", type=int, default=24, help="dispatches per variant, 8 inputs each")
    a = ap.parse_args()
    if a.output.exists():
        ap.error("choose a fresh receipt")
    a.work.mkdir(parents=True, exist_ok=False)
    rng = random.Random(0x46363445)
    edges = edge_doubles()
    tool_hashes = {n: digest(getattr(a, n)) for n in ("emitter", "dxc", "dxv", "provider")}
    if a.baseline:
        tool_hashes["baseline"] = digest(a.baseline)
    rows, expected, negatives = [], {}, []
    with TemporaryDirectory(dir=a.work) as td:
        trial = Path(td)
        guest = ROOT / "tools/reference/independent-shaders/compute.cpp"
        (trial / "guest.cpp").write_text(guest.read_text())
        (trial / "context.h").write_text((guest.parent / "context.h").read_text().replace(
            '"../compute_buffers.h"', '"' + str(ROOT / "tools/reference/compute_buffers.h") + '"'))
        inputs_used = 0
        for e64, flush in [(False, False), (True, False), (False, True)]:
            lines = program(e64, flush)
            words = assemble(lines, a.llvm_mc)
            if a.baseline:
                before = translate(a.baseline, words)
                if before.returncode != 65 or "_f64" not in before.stderr:
                    raise ValueError(f"baseline did not refuse: {before.returncode} {before.stderr}")
            got = translate(a.emitter, words)
            got.check_returncode()
            source = instrument(got.stdout)
            for k in range(a.dispatches):
                label = f"f64_{'e64' if e64 else 'e32'}{'_flush' if flush else ''}_{k:02}"
                shader = trial / (label + ".hlsl")
                shader.write_text(source)
                blob = shader.with_suffix(".cso")
                subprocess.run([a.dxc, "-T", "cs_6_0", "-E", "main", "-O3", "-Fo", blob, shader],
                               check=True, capture_output=True, timeout=60)
                subprocess.run([a.dxv, blob], check=True, capture_output=True, timeout=15)
                seed = [rng.getrandbits(32) for _ in range(384)]
                want = seed.copy()
                for lane in range(GROUPS):
                    index = k * GROUPS + lane
                    if index < len(edges):
                        b = edges[index]
                    elif lane % 2:
                        # Exponents around the integral boundary.
                        b = (rng.getrandbits(1) << 63) | (rng.randrange(1010, 1090) << 52) | rng.getrandbits(52)
                    else:
                        b = rng.getrandbits(64)
                    offset = START + STRIDE * lane
                    seed[offset:offset + 2] = [b & 0xffffffff, b >> 32]
                    want[offset:offset + 2] = seed[offset:offset + 2]
                    want[offset + 2:offset + STRIDE] = expected_outputs(b, flush)
                    inputs_used += 1
                (trial / (label + ".seed")).write_bytes(struct.pack("<384I", *seed))
                expected[label] = struct.pack("<384I", *want)
                rows.append({"case": label, "e64": e64, "denormals_flushed": flush,
                             "native_words": [f"{v:08x}" for v in words],
                             "emitted_sha256": hashlib.sha256(got.stdout.encode()).hexdigest(),
                             "dxil_sha256": digest(blob)})
        for name, dst, width in FORMS:
            target = f"v[{dst}:{dst + 1}]" if width == 2 else f"v{dst}"
            pair = name not in ("v_cvt_f64_i32", "v_cvt_f64_u32")
            for kind, operand in [("negated_source", "-v[4:5]" if pair else "-v4"),
                                  ("scalar_source", "s[4:5]" if pair else "s4")]:
                inst = f"{name}_e64 {target}, {operand}" if kind == "negated_source" else f"{name}_e32 {target}, {operand}"
                if kind == "negated_source" and not pair:
                    inst = f"{name}_e64 {target}, v4 clamp"
                    kind = "clamp"
                bad = translate(a.emitter, assemble([inst, "s_endpgm"], a.llvm_mc))
                if bad.returncode != 65:
                    raise ValueError(f"unexpected acceptance {inst}")
                negatives.append({"opcode": name, "kind": kind, "assembly": inst, "refusal": bad.stderr.strip()})
        (trial / "cases.txt").write_text("".join(f"{n} 4 structured {GROUPS}\n" for n in expected))
        exe, status, fields = run_gpu_guest(trial / "guest.cpp", "f64exact.exe", trial, a.work, a.proton,
                                            a.prefix, a.provider, a.compiler_dir)
        if status or fields.get("adapter") != a.adapter:
            raise RuntimeError(f"guest failed: {status} {fields}")
        failures = []
        for row in rows:
            actual = trial / (row["case"] + ".bin")
            observed = actual.read_bytes() if actual.is_file() else b""
            if observed != expected[row["case"]]:
                got_words = struct.unpack(f"<{len(observed) // 4}I", observed)
                want_words = struct.unpack("<384I", expected[row["case"]])
                failures.append({"case": row["case"], "different_words": [
                    (i, f"{x:08x}", f"{y:08x}") for i, (x, y) in enumerate(zip(got_words, want_words)) if x != y][:16]})
            else:
                row["readback_sha256"] = digest(actual)
        receipt = {"scope": "Synthetic LLVM-assembled exact f64 rounding and f64/int32 conversions; dynamic instrumented inputs and observed outputs",
                   "adapter": fields["adapter"], "proton": (a.proton / "version").read_text().strip(),
                   "seed": 0x46363445, "instructions": len(FORMS), "shader_cases": len(rows),
                   "inputs": inputs_used, "edge_inputs": len(edges),
                   "result_words": inputs_used * (STRIDE - 2), "compared_words": 384 * len(rows),
                   "mismatches": len(failures), "failures": failures,
                   "baseline_refused_programs": 3 if a.baseline else None,
                   "negative_controls": negatives, "tool_sha256": tool_hashes,
                   "guest_sha256": digest(exe), "rows": rows}
    with a.output.open("x") as f:
        json.dump(receipt, f, indent=2)
        f.write("\n")
    if failures:
        print(f"FAIL {len(failures)} of {len(rows)} dispatches; first: {failures[0]}")
        raise SystemExit(1)
    print(f"PASS {inputs_used} inputs, {receipt['result_words']} results, {receipt['compared_words']} GPU words; zero mismatches")


if __name__ == "__main__":
    main()
