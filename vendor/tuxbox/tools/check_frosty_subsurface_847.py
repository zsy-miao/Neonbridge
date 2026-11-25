#!/usr/bin/env python3
"""Verify Frosty's subsurface indirect setup with the genuine Xbox compute provider and texture/UAV readback."""
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
    ((_, native, _),) = containers(decompress(archive, entries[847], lz4))
    fingerprint = hashlib.sha256(native).hexdigest()
    if fingerprint != "93932adbc024e0a75d8a5fbbf27ab4a640d152fd6e95eb56c8bf2fc9ba11d329":
        raise ValueError("Unexpected cooked SetupIndirectCS shader")
    with TemporaryDirectory(dir=work) as temp:
        trial = Path(temp)
        (trial / "retail.dxbc").write_bytes(native)
        (trial / "depth-only").touch()
        exe, status, actual = run_gpu_guest(ROOT / "tools/reference/subsurface/guest.cpp", "subsurface.exe", trial,
                                            work, proton, args.prefix, args.provider, args.compiler_dir)
        expected = {f"capacity_{capacity}_{key}": str(value) for capacity in (1, 5)
                    for key, value in (("channels", 1024), ("counter", 4), ("valid_records", 1))}
        if status or any(actual.get(key) != value for key, value in expected.items()):
            raise ValueError(f"Subsurface GPU control returned {status}: {actual}. See {work / 'gpu.log'}")
        adapters = sorted({line.split("Found device: ", 1)[1]
                           for line in (work / "gpu.log").read_text().splitlines()
                           if "Found device: " in line})
        result = dict(shader="FSubsurfaceIndirectDispatchSetupCS / SetupIndirectCS", archive_entry=847,
                      global_archive_sha256=GLOBAL_SHA, cooked_sha256=fingerprint,
                      guest_slot=62, size=[16, 16], workgroup=[8, 8, 1], source_isa_instructions=183,
                      source_isa_bytes=1008, first_election="0x290", second_election="0x358",
                      test_inputs="Four qualifying pixels with material IDs 5/9 in three groups. Depth dithering, zero RGB output, independent material masks and viewport guards.",
                      expected_allocations=[[0, 0], [1, 0], [1, 1]], initial_count=1,
                      pixel_tolerance=0.00001, adapters=adapters, readback=actual, guest_sha256=digest(exe),
                      provider_sha256=digest(args.provider),
                      proton=(proton / "version").read_text().strip())
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
