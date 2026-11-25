#!/usr/bin/env python3
"""Inventory EXEC elections in the supplied Frosty global pixel archive.

Reports hashes, instruction offsets and operation counts only; no shader bytes
are written to the report. This is static analysis, not proof of equivalence.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from check_frosty_shaders import GLOBAL_SHA, containers, decompress, global_archive
from project_tuxbox.shader_inventory import decoder


OPS = ("save_exec", "count_mask", "atomic_add", "read_first_lane", "prefix_low", "prefix_high", "read_lane", "permute_lane16", "conditional_branch")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mount", type=Path)
    parser.add_argument("--entry", type=int, action="append", help="global archive index; defaults to all pixel programs")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    cargo = ROOT / "tools/shader-compiler/Cargo.toml"
    target = ROOT / "build/shader-compiler"
    subprocess.run(["cargo", "build", "--locked", "--release", "--target", "x86_64-unknown-linux-gnu",
                    "--target-dir", str(target), "--manifest-path", str(cargo), "--example", "wave_elections"], check=True)
    helper = target / "x86_64-unknown-linux-gnu/release/examples/wave_elections"
    payload, entries = global_archive(args.mount)
    indexes = args.entry if args.entry is not None else [i for i, e in enumerate(entries) if e[3] == 3]
    decode = decoder()
    rows = []
    with TemporaryDirectory(prefix="tuxbox-wave-survey-") as temp:
        shader = Path(temp) / "shader.dxbc"
        for index in indexes:
            if index < 0 or index >= len(entries) or entries[index][3] != 3:
                raise ValueError(f"global/{index} is not a pixel shader")
            blob = next(containers(decompress(payload, entries[index], decode)))[1]
            shader.write_bytes(blob)
            output = subprocess.run([helper, shader], check=True, capture_output=True, text=True).stdout.splitlines()
            info = {"index": index, "source_sha256": hashlib.sha256(blob).hexdigest(), "elections": []}
            if not output or output[0] != "stage\tPixel":
                raise ValueError(f"global/{index} was not decoded as a pixel shader")
            for line in output:
                kind, *values = line.split("\t")
                if kind == "entry": info["entry"] = values[0]
                elif kind == "election":
                    if len(values) != 10: raise ValueError("unexpected election record")
                    info["elections"].append({"offset": values[0], **dict(zip(OPS, map(int, values[1:])))})
            if info["elections"]: rows.append(info)
    report = {"scope": "Static pixel-shader EXEC election inventory; operation counts are a local 30-instruction window, not proof of data flow or rendering correctness.",
              "archive_sha256": GLOBAL_SHA, "compiler_sha256": hashlib.sha256(helper.read_bytes()).hexdigest(),
              "programs_scanned": len(indexes), "programs_with_elections": len(rows), "programs": rows}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(f"{len(rows)} of {len(indexes)} pixel programs contain EXEC elections")


if __name__ == "__main__":
    main()
