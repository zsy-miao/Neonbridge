#!/usr/bin/env python3
"""CPU oracle for ordinary native scalar bit operations.

Capture cargo test scalar_bit_cpu_oracle_fixture -- --nocapture from the
shader compiler. This checks observable DXIL stores after DXC constant
folding and DXV validation. It is not a GPU execution test.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess

OPS = (
    "s_brev_b64", "s_bcnt0_i32_b32", "s_bcnt0_i32_b64",
    "s_ff0_i32_b32", "s_ff0_i32_b64", "s_flbit_i32_b64",
    "s_flbit_i32", "s_flbit_i32_i64", "s_bitset0_b64",
    "s_bitset1_b64", "s_bitcmp0_b64", "s_bitcmp1_b64",
)
MASK = (1 << 32) - 1


def reference(name, lo, hi, index):
    """Independent scalar integer oracle, including unchanged registers/SCC."""
    x = lo | (hi << 32)
    scc = int(index == 0)
    if name == "s_brev_b64":
        result = int(f"{x:064b}"[::-1], 2)
        lo, hi = result & MASK, result >> 32
    elif name == "s_bcnt0_i32_b32":
        lo = 32 - lo.bit_count()
        scc = int(lo != 0)
    elif name == "s_bcnt0_i32_b64":
        hi = 64 - x.bit_count()
        scc = int(hi != 0)
    elif name in ("s_ff0_i32_b32", "s_ff0_i32_b64"):
        width = 32 if name.endswith("b32") else 64
        bits = lo if width == 32 else x
        lo = next((i for i in range(width) if not bits & (1 << i)), MASK)
    elif name == "s_flbit_i32_b64":
        hi = next((i for i in range(64) if x & (1 << (63 - i))), MASK)
    elif name in ("s_flbit_i32", "s_flbit_i32_i64"):
        width = 32 if name == "s_flbit_i32" else 64
        bits = lo if width == 32 else x
        sign = (bits >> (width - 1)) & 1
        value = next((i for i in range(1, width)
                      if ((bits >> (width - 1 - i)) & 1) != sign), MASK)
        if width == 32:
            lo = value
        else:
            hi = value
    elif name in ("s_bitset0_b64", "s_bitset1_b64"):
        # The set-one fixture aliases the index with the low destination.
        bit = 1 << ((lo if name == "s_bitset1_b64" else index) & 63)
        value = x | bit if name == "s_bitset1_b64" else x & ~bit
        lo, hi = value & MASK, value >> 32
    elif name in ("s_bitcmp0_b64", "s_bitcmp1_b64"):
        scc = int(((x >> (index & 63)) & 1) == int(name == "s_bitcmp1_b64"))
    else:
        raise ValueError(f"unknown opcode {name}")
    return lo, hi, index, scc


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def stores(disassembly):
    # Require every actual write to be a constant, one-element structured
    # buffer store. Missing, duplicate or nonconstant writes are failures.
    pattern = re.compile(
        r"call void @dx\.op\.bufferStore\.i32\(i32 69, %dx\.types\.Handle %\d+, "
        r"i32 (\d+), i32 0, i32 (-?\d+), i32 undef, i32 undef, i32 undef, i8 1\)"
    )
    result = {}
    for line in disassembly.splitlines():
        if "call void @dx.op.bufferStore" not in line:
            continue
        match = pattern.search(line)
        if not match:
            raise ValueError(f"nonconstant or unexpected store: {line}")
        index, value = map(int, match.groups())
        if index in result:
            raise ValueError(f"duplicate store {index}")
        result[index] = value & MASK
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("fixture", "dxc", "dxv", "work", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    args.work.mkdir(parents=True, exist_ok=False)
    if args.output.exists():
        parser.error("choose a fresh receipt")
    tools = {name: digest(getattr(args, name)) for name in ("dxc", "dxv")}
    fixture = args.fixture.read_text()
    blocks = re.findall(r"BEGIN_SCALAR (\w+)\n(.*?)END_SCALAR \1", fixture, re.S)
    if tuple(name for name, _ in blocks) != OPS:
        raise ValueError("missing, duplicate or unexpected native fixture")
    rows = []
    for name, source in blocks:
        cases = [tuple(map(int, v)) for v in re.findall(r"^// CASE (\d+) (\d+) (\d+)$", source, re.M)]
        if len(cases) != 162 or any(not 0 <= v <= MASK for case in cases for v in case):
            raise ValueError(f"incomplete {name} fixture")
        expected = {4 * n + k: value
                    for n, case in enumerate(cases)
                    for k, value in enumerate(reference(name, *case))}
        hlsl = args.work / (name + ".hlsl")
        blob, asm = hlsl.with_suffix(".cso"), hlsl.with_suffix(".ll")
        hlsl.write_text(source)
        subprocess.run([args.dxc, "-T", "cs_6_0", "-E", "main", "-O3",
                        "-Fo", blob, "-Fc", asm, hlsl], check=True, timeout=30)
        subprocess.run([args.dxv, blob], check=True, timeout=15)
        actual = stores(asm.read_text())
        if actual != expected:
            differences = [(i, expected.get(i), actual.get(i))
                           for i in sorted(expected.keys() | actual.keys())
                           if expected.get(i) != actual.get(i)]
            raise ValueError(f"{name}: {len(differences)} mismatches; {differences[:8]}")
        rows.append(dict(opcode=name, cases=len(cases), words=len(expected),
                         hlsl_sha256=digest(hlsl), dxil_sha256=digest(blob),
                         mismatches=0))
        print(f"PASS {name}: {len(expected)} result/preservation words", flush=True)
    if tools != {name: digest(getattr(args, name)) for name in ("dxc", "dxv")}:
        raise ValueError("compiler tools changed")
    receipt = dict(scope="CPU-only native instruction emitter statements, DXC constant-folded observable stores, DXV; no GPU or retail game",
                   tools_sha256=tools, fixture_sha256=digest(args.fixture),
                   instructions=len(rows), cases=sum(row["cases"] for row in rows),
                   compared_words=sum(row["words"] for row in rows), mismatches=0, rows=rows)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("x") as stream:
        json.dump(receipt, stream, indent=2)
        stream.write("\n")
    print(f"PASS {receipt['cases']} cases, {receipt['compared_words']} words")


if __name__ == "__main__":
    main()
