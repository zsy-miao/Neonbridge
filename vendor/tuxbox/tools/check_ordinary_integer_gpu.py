#!/usr/bin/env python3
"""LLVM-assembled ordinary VOP3 integer controls on a D3D12 GPU.

Ten 16-bit forms (every source and destination half selector) and four
forms with a 32-bit result. Each result aliases one source, so the 16-bit
forms also prove that the unselected destination half is preserved.
Synthetic dynamic inputs and guard words; no game is used.
"""
import argparse
import hashlib
import json
from pathlib import Path
import random
import struct
import subprocess
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
from check_packed_integer_gpu import assemble, digest, instrument, translate, INIT, STRIDE, GROUPS
from frosty_gpu_guest import run_gpu_guest

HALF_OPS = {"v_add_nc_i16": 2, "v_sub_nc_i16": 2, "v_mad_u16": 3, "v_mad_i16": 3,
            "v_min3_u16": 3, "v_max3_u16": 3, "v_med3_u16": 3,
            "v_min3_i16": 3, "v_max3_i16": 3, "v_med3_i16": 3}
WORD_OPS = {"v_add_nc_i32": 2, "v_sub_nc_i32": 2, "v_mad_u32_u16": 3, "v_mad_i32_i16": 3}
# The forms whose signed saturation the translator accepts.
CLAMP_OPS = {"v_add_nc_i32", "v_sub_nc_i32"}
CLAMP = 1 << 8
EDGE = [0, 1, 0xffff, 0x10000, 0x7fff8000, 0x80007fff, 0xffff0001, 0xffffffff,
        0x00008000, 0x7fffffff, 0x80000000, 0x8000ffff]


def signed(value, bits):
    return value - (1 << bits) if value >> (bits - 1) else value


def selectors(name):
    """(op_sel suffix, selector bits) for every legal selection of one form."""
    if name in HALF_OPS:
        n = HALF_OPS[name]
        return [(f" op_sel:[{','.join(str(sel >> j & 1) for j in range(n + 1))}]", sel)
                for sel in range(1 << (n + 1))]
    if name.startswith("v_mad_"):
        return [(f" op_sel:[{sel & 1},{sel >> 1},0,0]", sel) for sel in range(4)]
    return [("", 0), (" clamp", CLAMP)] if name in CLAMP_OPS else [("", 0)]


def reference(name, values, sel, old):
    half = lambda j: values[j] >> (16 * (sel >> j & 1)) & 0xffff
    if name in WORD_OPS:
        a, b = values[0], values[1]
        if sel & CLAMP:
            x, y = signed(a, 32), signed(b, 32)
            total = x + y if name == "v_add_nc_i32" else x - y
            return max(-2**31, min(2**31 - 1, total)) % 2**32
        if name == "v_add_nc_i32":
            return (a + b) % 2**32
        if name == "v_sub_nc_i32":
            return (a - b) % 2**32
        if name == "v_mad_u32_u16":
            return (half(0) * half(1) + values[2]) % 2**32
        return (signed(half(0), 16) * signed(half(1), 16) + signed(values[2], 32)) % 2**32
    n = HALF_OPS[name]
    xs = [half(j) for j in range(n)]
    if name.endswith("_i16") and not name.startswith(("v_add", "v_sub", "v_mad")):
        xs = [signed(x, 16) for x in xs]
    if name.startswith("v_add"):
        result = xs[0] + xs[1]
    elif name.startswith("v_sub"):
        result = xs[0] - xs[1]
    elif name.startswith("v_mad"):
        result = xs[0] * xs[1] + xs[2]
    elif name.startswith("v_min3"):
        result = min(xs)
    elif name.startswith("v_max3"):
        result = max(xs)
    else:
        result = sorted(xs)[1]
    result &= 0xffff
    if sel >> n & 1:
        return (old & 0xffff) | result << 16
    return (old & 0xffff0000) | result


def variant(name, suffix, reg, alias):
    n = (HALF_OPS | WORD_OPS)[name]
    sources = [f"v{4 + j}" for j in range(n)]
    lines = [f"v_mov_b32_e32 v{reg}, {sources[alias]}"]
    sources[alias] = f"v{reg}"
    return lines + [f"{name} v{reg}, " + ", ".join(sources) + suffix]


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    for n in ("emitter", "llvm-mc", "dxc", "dxv", "provider", "proton", "prefix", "compiler-dir", "work", "output"):
        ap.add_argument("--" + n, type=Path, required=True)
    ap.add_argument("--baseline", type=Path, help="earlier emitter that must refuse every case")
    ap.add_argument("--adapter", required=True)
    a = ap.parse_args()
    if a.output.exists():
        ap.error("choose a fresh receipt")
    a.work.mkdir(parents=True, exist_ok=False)
    rng = random.Random(0x4f524449)
    tool_hashes = {n: digest(getattr(a, n)) for n in ("emitter", "dxc", "dxv", "provider")}
    if a.baseline:
        tool_hashes["baseline"] = digest(a.baseline)
    rows, negatives, expected = [], [], {}
    with TemporaryDirectory(dir=a.work) as td:
        trial = Path(td)
        guest = ROOT / "tools/reference/independent-shaders/compute.cpp"
        (trial / "guest.cpp").write_text(guest.read_text())
        (trial / "context.h").write_text((guest.parent / "context.h").read_text().replace(
            '"../compute_buffers.h"', '"' + str(ROOT / "tools/reference/compute_buffers.h") + '"'))
        for name, n in (HALF_OPS | WORD_OPS).items():
            cases = selectors(name)
            for start in range(0, len(cases), 8):
                chunk = cases[start:start + 8]
                aliases = [(start + j) % n for j in range(len(chunk))]
                lines = [f"v_mov_b32_e32 v{4 + j}, 0x{v:x}" for j, v in enumerate(INIT)]
                for j, ((suffix, _), alias) in enumerate(zip(chunk, aliases)):
                    lines += variant(name, suffix, 7 + j, alias)
                lines += ["s_endpgm"]
                words = assemble(lines, a.llvm_mc)
                if a.baseline:
                    before = translate(a.baseline, words)
                    if before.returncode != 65 or name not in before.stderr:
                        raise ValueError(f"baseline did not refuse {name}: {before.returncode} {before.stderr}")
                got = translate(a.emitter, words)
                got.check_returncode()
                label = f"ordinary{len(rows):02}"
                shader = trial / (label + ".hlsl")
                shader.write_text(instrument(got.stdout, len(chunk)))
                blob = shader.with_suffix(".cso")
                subprocess.run([a.dxc, "-T", "cs_6_0", "-E", "main", "-O3", "-Fo", blob, shader],
                               check=True, capture_output=True, timeout=30)
                subprocess.run([a.dxv, blob], check=True, capture_output=True, timeout=15)
                seed = [rng.getrandbits(32) for _ in range(384)]
                want = seed.copy()
                for lane in range(GROUPS):
                    vals = ([EDGE[(lane + 5 * j) % len(EDGE)] for j in range(3)] if lane < 8
                            else [rng.getrandbits(32) for _ in range(3)])
                    offset = 96 + STRIDE * lane
                    seed[offset:offset + 3] = vals
                    want[offset:offset + 3] = vals
                    want[offset + 3:offset + 3 + len(chunk)] = [
                        reference(name, vals, sel, vals[alias]) for (_, sel), alias in zip(chunk, aliases)]
                (trial / (label + ".seed")).write_bytes(struct.pack("<384I", *seed))
                expected[label] = struct.pack("<384I", *want)
                rows.append({"case": label, "opcode": name, "selectors": [s for _, s in chunk],
                             "alias_indices": aliases, "assembly": lines,
                             "native_words": [f"{v:08x}" for v in words],
                             "emitted_sha256": hashlib.sha256(got.stdout.encode()).hexdigest(),
                             "instrumented_sha256": digest(shader), "dxil_sha256": digest(blob)})
            # Clamp (saturation) and non-VGPR sources remain unsupported.
            rest = ", ".join(f"v{5 + j}" for j in range(n - 1))
            refused = [("scalar_source", f"{name} v7, s4, {rest}")]
            if name not in CLAMP_OPS:
                refused.append(("clamp", f"{name} v7, v4, {rest} clamp"))
            for kind, inst in refused:
                bad = translate(a.emitter, assemble([inst, "s_endpgm"], a.llvm_mc))
                if bad.returncode != 65:
                    raise ValueError(f"unexpected acceptance {inst}")
                negatives.append({"opcode": name, "kind": kind, "assembly": inst, "refusal": bad.stderr.strip()})
        (trial / "cases.txt").write_text("".join(f"{n} 4 structured {GROUPS}\n" for n in expected))
        exe, status, fields = run_gpu_guest(trial / "guest.cpp", "ordinary.exe", trial, a.work, a.proton,
                                            a.prefix, a.provider, a.compiler_dir)
        if status or fields.get("adapter") != a.adapter:
            raise RuntimeError(f"guest failed: {status} {fields}")
        for row in rows:
            actual = trial / (row["case"] + ".bin")
            observed = actual.read_bytes()
            if observed != expected[row["case"]]:
                lhs, rhs = struct.unpack("<384I", observed), struct.unpack("<384I", expected[row["case"]])
                raise ValueError(f"{row['case']} {row['opcode']} GPU mismatch "
                                 f"{[(i, x, y) for i, (x, y) in enumerate(zip(lhs, rhs)) if x != y][:8]}")
            row["readback_sha256"] = digest(actual)
        selections = sum(len(r["selectors"]) for r in rows)
        receipt = {"scope": "Synthetic LLVM-assembled ordinary VOP3 integer instructions; dynamic instrumented inputs and observed outputs, no game",
                   "adapter": fields["adapter"], "proton": (a.proton / "version").read_text().strip(),
                   "seed": 0x4f524449, "instructions": len(HALF_OPS) + len(WORD_OPS), "shader_cases": len(rows),
                   "selector_combinations": selections, "result_words": GROUPS * selections,
                   "compared_words": 384 * len(rows), "mismatches": 0,
                   "baseline_refused_shaders": len(rows) if a.baseline else None,
                   "negative_controls": negatives, "tool_sha256": tool_hashes,
                   "guest_sha256": digest(exe), "rows": rows}
    with a.output.open("x") as f:
        json.dump(receipt, f, indent=2)
        f.write("\n")
    print(f"PASS {selections} selector combinations, {receipt['result_words']} results, "
          f"{receipt['compared_words']} GPU words; zero mismatches")


if __name__ == "__main__":
    main()
