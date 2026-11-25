#!/usr/bin/env python3
"""Verify Frosty's hair cluster culling with the genuine Xbox compute provider and texture/UAV readback."""
import argparse
import ctypes
from ctypes.util import find_library
import hashlib
import json
from pathlib import Path
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from check_frosty_shaders import GLOBAL_SHA, global_archive, decompress, containers
from frosty_gpu_guest import run_gpu_guest
from project_tuxbox.inspect import digest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("mount", "provider", "proton", "prefix", "compiler-dir", "work", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    work, proton = args.work.resolve(), args.proton.resolve()
    work.mkdir(parents=True, exist_ok=True)
    archive, entries = global_archive(args.mount)
    lz4 = ctypes.CDLL(find_library("lz4")).LZ4_decompress_safe
    lz4.argtypes = [ctypes.c_char_p, ctypes.c_void_p, ctypes.c_int, ctypes.c_int]
    ((_, native, _),) = containers(decompress(archive, entries[506], lz4))
    fingerprint = hashlib.sha256(native).hexdigest()
    if fingerprint != "7ec661710d5cf782cd2269d90db832fad2d349c6b05970e98bf099dc54bf2d39":
        raise ValueError("Unexpected cooked MainClusterCullingCS shader")
    with TemporaryDirectory(dir=work) as temp:
        trial = Path(temp)
        (trial / "retail.dxbc").write_bytes(native)
        exe, status, actual = run_gpu_guest(ROOT / "tools/reference/hair-cluster/guest.cpp", "hair.exe", trial,
                                            work, proton, args.prefix, args.provider, args.compiler_dir)
        expected = {
            "lod0": (8, 753), "blended": (7, 529), "automatic": (8, 753),
            "hidden": (2, 17), "frustum": (7, 509),
        }
        for mode, (count, vertices) in expected.items():
            values = tuple(int(actual.get(f"{mode}_{key}", -1)) for key in ("matched", "cluster_count", "draw_vertices"))
            if status or values != (420, count, vertices):
                raise ValueError(f"Hair cluster GPU control returned {status}: {actual}. See {work / 'gpu.log'}")
        adapters = [line.split("Found device: ", 1)[1] for line in (work / "gpu.log").read_text().splitlines()
                    if "Found device: " in line]
        result = dict(shader="FHairClusterCullingCS / MainClusterCullingCS", archive_entry=506,
                      global_archive_sha256=GLOBAL_SHA, cooked_fnv1a64="1be6d7f0a5efda73", cooked_sha256=fingerprint,
                      guest_slot=62, workgroup=[64, 1, 1], source_isa_instructions=343, source_isa_bytes=1816,
                      first_election="0x5d8", second_election="0x694", adapters=adapters,
                      test_inputs="Sparse clusters across both wave halves, forced and automatic LOD, blended vertex counts and FP16 radius values, view plane excluding cluster 60, partial range, hidden group.",
                      initial_cluster_count=2, initial_draw_vertices=17, checked_words_per_case=420,
                      readback=actual, guest_sha256=digest(exe), provider_sha256=digest(args.provider),
                      proton=(proton / "version").read_text().strip())
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
