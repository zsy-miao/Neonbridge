#!/usr/bin/env python3
"""Dispatch Frosty's original particle sort-key shader and check keys, IDs, counters and guard words."""
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
    fingerprints = {
        3333: "1bb75d7eee96ddae9c7a19fa5dd91c551a894120999daa71218c254705850a33",
        3335: "5201c5a3f6c6c556de14ec9036c4bcb4c21ea5b6fb0236340273d7735a838eaf",
    }
    parser.add_argument("--entry", type=int, choices=fingerprints, default=3333)
    args = parser.parse_args()
    work, proton = args.work.resolve(), args.proton.resolve()
    work.mkdir(parents=True, exist_ok=True)
    archive, entries = global_archive(args.mount)
    lz4 = ctypes.CDLL(find_library("lz4")).LZ4_decompress_safe
    lz4.argtypes = [ctypes.c_char_p, ctypes.c_void_p, ctypes.c_int, ctypes.c_int]
    with TemporaryDirectory(dir=work) as temp:
        trial = Path(temp)
        ((_, native, _),) = containers(decompress(archive, entries[args.entry], lz4))
        fingerprint = hashlib.sha256(native).hexdigest()
        if fingerprint != fingerprints[args.entry]:
            raise ValueError("Unexpected cooked particle sort shader")
        if args.entry == 3335:
            (trial / "full-width").touch()
        (trial / "retail.dxbc").write_bytes(native)
        exe, status, actual = run_gpu_guest(ROOT / "tools/reference/particle-sort/guest.cpp", "particles.exe", trial,
                                            work, proton, args.prefix, args.provider, args.compiler_dir)
        expected = {f"live_{n}_matched": "384" for n in [67, 99, 0]}
        expected.update({"live_67_count": "74", "live_99_count": "14", "live_0_count": "7"})
        if status or actual != expected:
            raise ValueError(f"Particle sort GPU control returned {status}: {actual}. See {work / 'gpu.log'}")
        adapters = sorted({line.split("Found device: ", 1)[1] for line in (work / "gpu.log").read_text().splitlines()
                           if "Found device: " in line})
        result = dict(shader="GenerateParticleSortKeys", global_archive_entry=args.entry, cooked_sha256=fingerprint,
                      global_archive_sha256=GLOBAL_SHA, group=[64, 1, 1], groups_dispatched=2, guest_slot=62,
                      key_bits=32 if args.entry == 3335 else 16,
                      scope="Three dispatches using genuine particle attribute, indirect count and visibility tag SRVs. Ascending and descending keys including negative, zero and fractional values that round to duplicate half-precision keys. Destination offset, 103 capacity, two particle groups, sparse visibility across lanes 31/32 and 63/64, non-zero initial count, empty input, guard words.",
                      words_per_case=384, adapters=adapters, readback=actual, guest_sha256=digest(exe),
                      provider_sha256=digest(args.provider), proton=(proton / "version").read_text().strip())
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
