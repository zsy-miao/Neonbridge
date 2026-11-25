#!/usr/bin/env python3
"""Verify Frosty's two 512-thread hair prefix sums with the genuine Xbox compute provider and texture/UAV readback."""
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
    fingerprints = {500: "784beb40fe291b076dcab2930238a71b2e2c684813d82705bd6ec8966027a76e",
                    502: "3262d7c9f78d41cb2b552bf0605e0f66963aea7d8630d3c3e8ff5d80656ae5a0"}
    with TemporaryDirectory(dir=work) as temp:
        trial = Path(temp)
        for index, expected_hash in fingerprints.items():
            ((_, native, _),) = containers(decompress(archive, entries[index], lz4))
            if hashlib.sha256(native).hexdigest() != expected_hash:
                raise ValueError(f"Unexpected cooked prefix shader at {index}")
            (trial / f"{index}.dxbc").write_bytes(native)
        exe, status, actual = run_gpu_guest(ROOT / "tools/reference/hair-prefix/guest.cpp", "prefix.exe", trial,
                                            work, proton, args.prefix, args.provider, args.compiler_dir)
        expected = {f"entry_{e}_count_{n}": "2060" for e in fingerprints for n in (769, 512, 1, 0)}
        if status or actual != expected:
            raise ValueError(f"Hair prefix GPU control returned {status}: {actual}. See {work / 'gpu.log'}")
        adapters = [line.split("Found device: ", 1)[1] for line in (work / "gpu.log").read_text().splitlines()
                    if "Found device: " in line]
        result = dict(shader_entries={500: "MainClusterCullingPreFixSumCS", 502: "MainPerBlockPreFixSumCS"},
                      global_archive_sha256=GLOBAL_SHA, cooked_sha256=fingerprints, guest_slot=62,
                      mop="BUFFER_STORE_FORMAT_XY", mop_code="0x5", retail_entry=502,
                      workgroup=[512, 1, 1], shader_views=["R32_UINT", "R32G32_UINT"],
                      test_inputs="769 elements crossing two blocks, 512, one and zero. Nonzero offsets, wrapped unsigned values, exclusive/inclusive prefix, per-block totals and guard words.",
                      checked_words_per_case=2060, adapters=adapters, readback=actual,
                      guest_sha256=digest(exe), provider_sha256=digest(args.provider),
                      proton=(proton / "version").read_text().strip())
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
