#!/usr/bin/env python3
"""Create 24 Xbox compute pipelines concurrently with 16 distinct native inputs.

Use the synthetic corpus from reference/scarlett_worker_fixture.py and a
new output path. Requires a built provider and the standard Proton controls.
The no-op fallback is disabled: real pipeline creation and validated host
translations are required. This does not exercise the retail game.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from frosty_gpu_guest import run_gpu_guest
from project_tuxbox.shader_prep import SOURCE_CACHE

parser = argparse.ArgumentParser(description=__doc__)
for name in ("corpus", "provider", "proton", "prefix", "compiler-dir", "work", "output"):
    parser.add_argument("--" + name, type=Path, required=True)
parser.add_argument("--adapter", required=True)
parser.add_argument("--jobs", type=int, choices=(1, 2, 4, 8), default=4)
args = parser.parse_args()
if not re.fullmatch(r"[0-9a-f]{4}:[0-9a-f]{4}", args.adapter):
    parser.error("adapter must be a lower-case PCI vendor:device pair")
if args.output.exists():
    parser.error("choose a fresh output path")
work = args.work.resolve()
work.mkdir(parents=True, exist_ok=True)
provider = args.provider.resolve()
inputs = sorted(args.corpus.glob("*.dxbc"))[:16]
if len(inputs) != 16:
    parser.error("need sixteen distinct synthetic requests")

def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

assert all(digest(path) == path.stem for path in inputs)
with TemporaryDirectory(dir=work) as td:
    trial = Path(td)
    for n, source in enumerate(inputs):
        shutil.copyfile(source, trial / f"shader{n}.dxbc")
    (trial / "adapter.txt").write_text(args.adapter)
    shutil.copyfile(ROOT / "tools/reference/scarlett_worker_guest.cpp", trial / "guest.cpp")
    exe, status, fields = run_gpu_guest(
        trial / "guest.cpp", "worker.exe", trial, work,
        args.proton.resolve(), args.prefix.resolve(), provider,
        args.compiler_dir.resolve(),
        link_libraries=(provider.with_suffix(".lib"),),
        extra_env={"TUXBOX_NULL_PIPELINES": "0", "TUXBOX_TRACE_SHADERS": "1",
                   "TUXBOX_LOG": "stderr", "TUXBOX_RUNTIME_SHADER_JOBS": str(args.jobs)},
    )
    log = (work / "gpu.log").read_text(errors="replace")
    assert status == 0 and fields.get("created") == "24" and fields.get("calls") == "24", (status, fields, log[-3000:])
    assert fields.get("unique") == "16" and fields.get("adapter") == args.adapter
    compiled = re.findall(r"^Compiled Xbox shader ([0-9a-f]{64})$", log, re.M)
    expected = {source.stem for source in inputs}
    assert len(compiled) == 16 and set(compiled) == expected, compiled
    cache = trial / "prefix" / SOURCE_CACHE.parent / "runtime"
    replies = list(cache.glob("*/*.cso"))
    assert len(replies) == 16 and {f.stem for f in replies} == expected
    for path in replies:
        assert path.with_suffix(".sha256").read_text() == digest(path)
    actual = re.search(r"Xbox shader worker: up to (\d+) concurrent requests", log)
    assert actual is not None
    receipt = {
        "scope": "Real Xbox provider pipeline creation through Linux host worker; synthetic resource-free compute, null fallback disabled",
        "adapter": args.adapter, "distinct_requests": 16, "concurrent_calls": 24,
        "created": 24, "compiled": 16, "jobs": int(actual.group(1)),
        "provider_sha256": digest(provider), "guest_sha256": digest(exe),
        "guest_source_sha256": digest(ROOT / "tools/reference/scarlett_worker_guest.cpp"),
        "proton": (args.proton / "version").read_text().strip(),
        "replies": {path.stem: digest(path) for path in replies},
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("x") as stream:
        json.dump(receipt, stream, indent=2)
        stream.write("\n")
    print(f"PASS: 24 pipelines, 16 compiled once, {receipt['jobs']} workers, {args.adapter}")
