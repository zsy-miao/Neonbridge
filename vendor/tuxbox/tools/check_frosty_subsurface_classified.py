#!/usr/bin/env python3
"""Verify subsurface tile classification through the Xbox compute provider."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from check_frosty_shaders import GLOBAL_SHA, containers, decoder, decompress, global_archive
from frosty_gpu_guest import run_gpu_guest
from project_tuxbox.inspect import digest

SHADERS = {824: "d74826fb0054b57193e687148b6ab2c0dbe576a9312505e916bc25b79a37d078",
           828: "312ef909f0c66a7b66a2e2d45b4299506c56aa333198bfb046af187b1bb8d1a5"}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("mount", "xshader", "provider", "proton", "prefix", "compiler-dir", "work", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    work, proton = args.work.resolve(), args.proton.resolve()
    work.mkdir(parents=True, exist_ok=True)
    archive, entries = global_archive(args.mount)
    os.environ["TUXBOX_XSHADER"] = str(args.xshader.resolve())
    results, adapters, guest_hash = {}, set(), None
    for entry, expected_hash in SHADERS.items():
        ((_, native, _),) = containers(decompress(archive, entries[entry], decoder()))
        if hashlib.sha256(native).hexdigest() != expected_hash:
            raise ValueError(f"unexpected global shader {entry}")
        with TemporaryDirectory(dir=work) as temp:
            trial = Path(temp)
            (trial / "retail.dxbc").write_bytes(native)
            (trial / "depth-only").touch()
            exe, status, actual = run_gpu_guest(ROOT / "tools/reference/subsurface-classified/guest.cpp", "subsurface.exe",
                                                trial, work, proton, args.prefix, args.provider, args.compiler_dir)
            expected = {}
            for capacity in (1, 5):
                for prefix, counter, other in (("", 4, 0), ("second_", 1, 3)):
                    for key, value in (("channels", 1024), ("counter", counter), ("other", other), ("valid_records", 1)):
                        expected[f"{prefix}capacity_{capacity}_{key}"] = str(value)
            if status or any(actual.get(key) != value for key, value in expected.items()):
                raise ValueError(f"subsurface {entry}: {status}, {actual}. See {work / 'gpu.log'}")
            guest_hash = digest(exe)
            adapters.update(line.split("Found device: ", 1)[1] for line in (work / "gpu.log").read_text().splitlines()
                            if "Found device: " in line)
            results[entry] = {"cooked_sha256": expected_hash, "readback": actual}
    result = {"scope": "Two packaged subsurface tile-classification shaders through TuxBox’s provider and runtime shader worker. Both indirect lists and capacities are checked against CPU expectations; this does not validate complete skin rendering.",
              "archive_sha256": GLOBAL_SHA, "adapters": sorted(adapters), "proton": (proton / "version").read_text().strip(),
              "xshader_sha256": digest(args.xshader), "provider_sha256": digest(args.provider), "guest_sha256": guest_hash, "shaders": results}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
