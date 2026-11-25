#!/usr/bin/env python3
"""Compare two shader compilers on every distinct raster and compute shader in Frosty's archives."""
import argparse
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path
import subprocess
import sys
from tempfile import TemporaryDirectory
from threading import get_ident

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from project_tuxbox.inspect import digest
from project_tuxbox.shader_inventory import ARCHIVES, archive_entries, decoder


def translate(compiler, folder):
    hlsl, profile = folder / "program.hlsl", folder / "profile"
    hlsl.unlink(missing_ok=True)
    profile.unlink(missing_ok=True)
    run = subprocess.run([compiler, folder / "source.dxbc", "--hlsl-out", hlsl, "--profile-out", profile],
                         stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, timeout=30)
    if run.returncode < 0:
        raise RuntimeError(f"{compiler} exited on signal {-run.returncode}")
    if run.returncode:
        return {"error": run.stderr.decode(errors="replace").strip()}
    return {"profile": profile.read_text().strip(), "hlsl_sha256": digest(hlsl)}


def compare(pak, baseline, candidate, jobs):
    seen, pending, changes = set(), [], []
    counts = Counter()
    by_archive = {}
    fingerprints = [digest(baseline), digest(candidate)]
    with TemporaryDirectory(prefix="tuxbox-xshader-") as tmp, ThreadPoolExecutor(jobs) as pool, pak.open("rb") as stream:
        def test(item):
            source, native = item
            folder = Path(tmp) / str(get_ident())
            folder.mkdir(exist_ok=True)
            (folder / "source.dxbc").write_bytes(native)
            return source, translate(baseline, folder), translate(candidate, folder)

        def drain():
            for source, before, after in pool.map(test, pending):
                if before != after:
                    changes.append({"dxbc_sha256": source, "baseline": before, "candidate": after})
                elif "error" in after:
                    counts["same_rejection"] += 1
                else:
                    counts["same_profile_and_hlsl"] += 1
            pending.clear()

        def enqueue(source, native):
            if source not in seen:
                seen.add(source)
                pending.append((source, native))
                if len(pending) == 8 * jobs:
                    drain()

        decode = decoder()
        for name in ARCHIVES:
            by_archive[name] = sum(1 for _ in archive_entries(name, stream, decode, enqueue, {}))
        drain()
    if fingerprints != [digest(baseline), digest(candidate)]:
        raise ValueError("A compiler changed during the comparison")
    return {"archive_entries": by_archive, "unique_dxbc": len(seen),
            "baseline_sha256": fingerprints[0], "candidate_sha256": fingerprints[1],
            "same_profile_and_hlsl": counts["same_profile_and_hlsl"],
            "same_rejection": counts["same_rejection"], "changes": changes}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mount", type=Path)
    for name in ("baseline", "candidate", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--jobs", type=int, default=4)
    args = parser.parse_args()
    if args.jobs < 1:
        parser.error("--jobs must be positive")
    result = compare(args.mount / "Frosty/Content/Paks/Frosty-XSX.pak", args.baseline.resolve(),
                     args.candidate.resolve(), args.jobs)
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(f"{result['unique_dxbc']} unique programs compared, {len(result['changes'])} differences")
    return bool(result["changes"])


if __name__ == "__main__":
    sys.exit(main())
