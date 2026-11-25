#!/usr/bin/env python3
"""Compare seeded triangle hits with the native Radeon instruction.

The input geometry is generated here. The native probe and its HSA runtime must
already be available; no game files are read. This is a narrow numerical test,
not evidence that the prototype handles arbitrary rays or misses.
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


def float32(value):
    return struct.unpack("<f", struct.pack("<f", value))[0]


def make_blas(random_source, bounded_miss=False):
    body = bytearray(64)
    struct.pack_into("<I", body, 12, 6)
    struct.pack_into("<I", body, 52, 64)
    for index in range(6):
        while True:
            vertices = [[float32(random_source.randint(-80, -60) if axis == 0 else
                                 random_source.randint(-20, 20)) if bounded_miss else
                         float32(random_source.uniform(-200, 200)) for axis in range(3)]
                        for _ in range(3)]
            a, b, c = vertices
            area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
            if abs(area) > 100:
                break
        d = [float32(b[i] + c[i] - a[i]) for i in range(3)]
        selectors = random_source.choice((9, 2, 4)) | (random_source.choice((9, 2, 4)) << 8)
        body += struct.pack("<12f4I", *(a + b + c + d), 0x80, 2 * index,
                            2 * index + 1, selectors)
    return bytes(64) + body


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--probe", type=Path, required=True)
    parser.add_argument("--code", type=Path, required=True)
    parser.add_argument("--proton", type=Path, required=True)
    parser.add_argument("--prefix-amd", type=Path, required=True)
    parser.add_argument("--prefix-nvidia", type=Path, required=True)
    parser.add_argument("--amd-vulkan", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--float-rtz", action="store_true", help="test the float32-only RTZ candidate")
    direction = parser.add_mutually_exclusive_group()
    direction.add_argument("--oblique", action="store_true", help="use varied ray directions and magnitudes")
    direction.add_argument("--tied", action="store_true", help="use tied dominant ray axes")
    direction.add_argument("--miss", action="store_true", help="offset rays to exercise misses")
    direction.add_argument("--bounded-miss", action="store_true", help="seed lattice-bounded miss geometry")
    direction.add_argument("--edge", action="store_true", help="target triangle edge midpoints")
    direction.add_argument("--nearedge", action="store_true", help="offset edge rays by one float ULP")
    args = parser.parse_args()
    if (args.edge or args.nearedge) and not args.float_rtz:
        raise ValueError("edge sweeps require the float32 candidate")
    random_source = random.Random(219091)
    rays = ("varied ray directions and magnitudes" if args.oblique else
            "tied dominant ray axes" if args.tied else
            "bounded lattice misses" if args.bounded_miss else
            "offset varied directions" if args.miss else "edge midpoint rays" if args.edge else
            "one-ULP offset edge rays" if args.nearedge else "positive-Z unit rays")
    result = {"scope": f"48 seeded synthetic paired triangle cases, {rays}, native gfx1036 instruction versus dynamic-input experimental HLSL; not general traversal or opcode equivalence",
              "seed": 219091, "float_rtz": args.float_rtz, "batches": [], "cases": 0, "native_hits": 0,
              "native_misses": 0, "mismatches": 0}
    with TemporaryDirectory(prefix="tuxbox-bvh-rtz-sweep-") as directory:
        root = Path(directory)
        for batch in range(8):
            source = root / f"{batch}.blas"
            dump = root / f"{batch}.bin"
            report = root / f"{batch}.json"
            source.write_bytes(make_blas(random_source, args.bounded_miss))
            mode = ("--retail-oblique" if args.oblique else "--retail-tied" if args.tied else
                    "--retail-miss" if args.miss or args.bounded_miss else "--retail-edge" if args.edge else
                    "--retail-nearedge" if args.nearedge else "--retail-triangles")
            native = subprocess.run([str(args.probe), str(args.code), mode,
                                     str(source), str(dump)], capture_output=True, text=True)
            if native.returncode:
                raise RuntimeError(f"native batch {batch} failed: {native.stderr[-1000:]}")
            data = dump.read_bytes()
            if data[:8] != b"TXBVT002" or len(data) != 8 + 6 * 27 * 4:
                raise ValueError(f"invalid native output in batch {batch}")
            for case in range(6):
                if args.bounded_miss:
                    base = 8 + case * 27 * 4
                    words = struct.unpack_from('<27I', data, base)
                    side = words[22]
                    indices = (0, 1, 2) if side == 0 else (1, 3, 2)
                    selected = [words[3 * i + j] for i in indices for j in range(3)]
                    selected += list(words[16:22])
                    for word in selected:
                        value = struct.unpack('<f', struct.pack('<I', word))[0]
                        if not (-256 <= value <= 256 and value * 256 == int(value * 256)):
                            raise RuntimeError('generated case left bounded miss lattice')
                numerator = struct.unpack_from("<I", data, 8 + case * 27 * 4 + 23 * 4)[0]
                if numerator & 0x7fffffff == 0x7f800000:
                    result["native_misses"] += 1
                else:
                    result["native_hits"] += 1
            checker = "check_native_bvh_edges.py" if args.edge or args.nearedge else "check_native_bvh_hlsl.py"
            command = [sys.executable, str(ROOT / "tools" / checker),
                       "--dump", str(dump), "--proton", str(args.proton),
                       "--prefix-amd", str(args.prefix_amd), "--prefix-nvidia", str(args.prefix_nvidia),
                       "--output", str(report)]
            if not (args.edge or args.nearedge):
                command += ["--strict", "--projected-rtz", "--dynamic"]
            if args.amd_vulkan:
                command += ["--amd-vulkan", str(args.amd_vulkan)]
            if args.float_rtz and not (args.edge or args.nearedge):
                command.append("--float-rtz")
            checked = subprocess.run(command, capture_output=True, text=True, cwd=ROOT)
            if not report.exists():
                raise RuntimeError(f"HLSL batch {batch} failed: {checked.stderr[-1000:]}")
            comparison = json.loads(report.read_text())
            mismatches = sum(len(item["mismatches"]) for item in comparison["adapters"].values())
            result["batches"].append({"source_sha256": hashlib.sha256(source.read_bytes()).hexdigest(),
                                      "native_dump_sha256": comparison["native_dump_sha256"],
                                      "hlsl_sha256": comparison["hlsl_sha256"],
                                      "dxil_sha256": comparison["dxil_sha256"],
                                      "adapters": comparison["adapters"]})
            result["cases"] += comparison["cases"]
            result["mismatches"] += mismatches
            if checked.returncode and not mismatches:
                raise RuntimeError(f"HLSL batch {batch} failed: {checked.stderr[-1000:]}")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(f"cases={result['cases']} mismatches={result['mismatches']}")
    if result["mismatches"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
