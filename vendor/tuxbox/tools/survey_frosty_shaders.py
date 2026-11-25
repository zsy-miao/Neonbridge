#!/usr/bin/env python3
"""Survey Frosty's shader translations without launching the title.

Example:
    python tools/survey_frosty_shaders.py /path/to/MA/Mount --output /tmp/survey.json
    python tools/survey_frosty_shaders.py /path/to/MA/Mount --archive material --output /tmp/material.json
    python tools/survey_frosty_shaders.py /path/to/MA/Mount --archive material --stage material-hit-group --output /tmp/material-hits.json

Add --trace-log for each TuxBox log to flag shader sources requested by the
title. The receipt contains metadata and diagnostics, not game shader bytes.
"""
import argparse
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
from tempfile import TemporaryDirectory

from check_frosty_shaders import GLOBAL_SHA, containers, decompress, global_archive
from project_tuxbox.shader_inventory import ARCHIVES, ENTRY as ARCHIVE_ENTRY, decoder, dxbc, parse_tables, serialized_state
from project_tuxbox.shader_prep import shader_compiler


STAGES = {0: "vertex", 1: "mesh", 3: "pixel", 4: "geometry", 5: "compute", 8: "material-hit-group"}
REQUEST = re.compile(r"shader request (?:vertex|mesh|pixel|geometry|compute|ray library) ([0-9a-f]{64})")
MATERIAL_HIT = re.compile(rb"(?<![A-Za-z0-9_])Material(CHS|AHS)_[a-fA-F0-9]{8}\.ss_0\x00")
ENTRY = re.compile(r'entry=Some\("([^"]+)"\)')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mount", type=Path, help="unpacked title Mount directory")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--trace-log", type=Path, action="append", default=[])
    parser.add_argument("--xshader", type=Path, help="use a particular compiler build")
    parser.add_argument("--workers", type=int, default=2)
    parser.add_argument("--archive", choices=("global", "material"), default="global",
                        help="shader archive to survey (default: global)")
    selection = parser.add_mutually_exclusive_group()
    selection.add_argument("--include-compute", action="store_true", help="also survey compute shaders")
    selection.add_argument("--stage", choices=STAGES.values(), help="survey just this stage")
    args = parser.parse_args()
    if args.workers < 1:
        parser.error("--workers must be positive")

    selected = {args.stage} if args.stage else {"vertex", "pixel", "geometry"}
    if args.include_compute:
        selected.add("compute")
    material_hits = args.stage == "material-hit-group"
    if material_hits and args.archive != "material":
        parser.error("material-hit-group requires --archive material")
    observed = set()
    for log in args.trace_log:
        observed.update(REQUEST.findall(log.read_text(errors="replace")))
    decode = decoder()
    if args.archive == "global":
        payload, entries = global_archive(args.mount)
        archive_hash = GLOBAL_SHA
    else:
        offset, size, archive_hash = ARCHIVES["material"]
        with (args.mount / "Frosty/Content/Paks/Frosty-XSX.pak").open("rb") as pak:
            pak.seek(offset)
            data = pak.read(size)
        if hashlib.sha256(data).hexdigest() != archive_hash:
            raise ValueError("Unexpected material archive fingerprint")
        _, _, programs, _, payload = parse_tables(memoryview(data))
        entries = list(ARCHIVE_ENTRY.iter_unpack(programs))
    compiler = (args.xshader or shader_compiler()).resolve()
    compiler_hash = hashlib.sha256(compiler.read_bytes()).hexdigest()

    with TemporaryDirectory(prefix="tuxbox-frosty-survey-") as tmp:
        work = Path(tmp)
        snapshot = work / "xshader"
        shutil.copy2(compiler, snapshot)
        if hashlib.sha256(snapshot.read_bytes()).hexdigest() != compiler_hash:
            raise ValueError("shader compiler changed while being copied")
        jobs = []
        for index, entry in enumerate(entries):
            if STAGES.get(entry[3]) not in selected:
                continue
            packed = decompress(payload, entry, decode)
            stage = STAGES[entry[3]]
            if material_hits:
                _, native = serialized_state(packed)
                exports = set(MATERIAL_HIT.findall(native))
                if b"CHS" not in exports:
                    continue
                stage = "paired" if b"AHS" in exports else "opaque"
            elif args.archive == "global":
                native = next(containers(packed))[1]
            else:
                native = dxbc(packed)
            source_hash = hashlib.sha256(native).hexdigest()
            source = work / f"{index}.{'sptr' if material_hits else 'dxbc'}"
            source.write_bytes(native)
            jobs.append((index, stage, source_hash, source))

        def translate(job):
            index, stage, source_hash, source = job
            hlsl = work / f"{index}.hlsl"
            result = subprocess.run(
                [snapshot, source, "--hlsl-out", hlsl],
                capture_output=True, text=True, check=False,
            )
            emitted = result.returncode == 0 and hlsl.is_file() and hlsl.stat().st_size > 0
            if emitted and material_hits:
                library = hlsl.read_bytes()
                emitted = b'[shader("closesthit")]' in library and (
                    stage == "opaque" or b'[shader("anyhit")]' in library)
            hlsl.unlink(missing_ok=True)
            name = ENTRY.search(result.stdout)
            return {
                "index": index,
                "stage": stage,
                "source_sha256": source_hash,
                "entry": name.group(1) if name else None,
                "observed": source_hash in observed,
                "translated": emitted,
                "diagnostic": None if emitted else result.stderr.strip() or "Missing HLSL output or shader export",
            }

        with ThreadPoolExecutor(max_workers=args.workers) as pool:
            results = sorted(pool.map(translate, jobs), key=lambda row: row["index"])

    stages = {}
    stage_names = ("opaque", "paired") if material_hits else (s for s in STAGES.values() if s in selected)
    for stage in stage_names:
        rows = [row for row in results if row["stage"] == stage]
        failures = [row for row in rows if not row["translated"]]
        stages[stage] = {
            "entries": len(rows),
            "translated": len(rows) - len(failures),
            "failed": len(failures),
            "observed": sum(row["observed"] for row in rows),
            "observed_failed": sum(row["observed"] for row in failures),
            "failure_reasons": dict(Counter(
                re.sub(r" for MaterialCHS_.*", "", re.sub(r" (?:at|@) 0x[0-9a-f]+", "", row["diagnostic"]))
                for row in failures
            )),
        }
    receipt = {
        "scope": f"{args.archive.capitalize()} archive HLSL translation; no DXIL, pipeline, or GPU validation",
        f"{args.archive}_archive_sha256": archive_hash,
        "compiler_sha256": compiler_hash,
        "stages": stages,
        "entries": results,
    }
    if material_hits:
        receipt["scope"] = "Frosty material archive, full HLSL emission from MaterialCHS hit groups. Paired entries require both closest-hit and any-hit HLSL exports. DXIL compilation, GPU output and title dispatch are evaluated separately."
        if not args.trace_log:
            for stage in stages.values():
                del stage["observed"], stage["observed_failed"]
        # Compact receipts still identify exactly which archive entries emitted.
        receipt.pop("entries")
        receipt["result_set_sha256"] = hashlib.sha256("".join(
            f"{row['index']} {row['stage']} {row['source_sha256']} {int(row['translated'])}\n"
            for row in results
        ).encode()).hexdigest()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps(stages, indent=2))


if __name__ == "__main__":
    main()
