#!/usr/bin/env python3
"""Measure real live-worker request latency using a synthetic compute corpus.

Run once per variant, with a fresh output path. No game or GPU is required.
Specify --repo pointing to the source tree whose worker is under test.
The exact same compiler tools and corpus must be used for all variants.
"""
import argparse
import hashlib
import inspect
import json
import os
from pathlib import Path
import statistics
import sys
from tempfile import TemporaryDirectory
from threading import Event, Thread
from time import monotonic, sleep
from unittest.mock import patch

parser = argparse.ArgumentParser(description=__doc__)
for name in ("repo", "corpus", "xshader", "dxc", "dxv", "output"):
    parser.add_argument("--" + name, type=Path, required=True)
parser.add_argument("--jobs", type=int, default=1)
parser.add_argument("--rounds", type=int, default=1)
parser.add_argument("--count", type=int, default=64)
parser.add_argument("--arrival-ms", type=float, default=0)
parser.add_argument("--dxc-helper", type=Path, help="opt-in persistent DXC helper")
args = parser.parse_args()
sys.path.insert(0, str(args.repo.resolve()))
from project_tuxbox import runtime_shaders as worker

tools = tuple(getattr(args, name).resolve() for name in ("xshader", "dxc", "dxv"))
sources = sorted(args.corpus.glob("*.dxbc"))[:args.count]
assert len(sources) == args.count and args.rounds > 0 and args.jobs > 0
inputs = {p.stem: p.read_bytes() for p in sources}
assert all(hashlib.sha256(data).hexdigest() == key for key, data in inputs.items())
has_jobs = "jobs" in inspect.signature(worker.serve).parameters
assert has_jobs or args.jobs == 1

def hashes():
    fingerprints = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in tools}
    if args.dxc_helper:
        fingerprints["dxc_helper"] = hashlib.sha256(args.dxc_helper.read_bytes()).hexdigest()
    return fingerprints

def memory_available():
    return int(next(line.split()[1] for line in Path("/proc/meminfo").read_text().splitlines()
                    if line.startswith("MemAvailable:")))

def swap_activity():
    values = dict(line.split() for line in Path("/proc/vmstat").read_text().splitlines())
    return [int(values[name]) for name in ("pswpin", "pswpout")]

def process_rss():
    # Sum RSS of this process and descendants, including compiler children.
    # Shared pages can be counted more than once; the figure is conservative.
    nodes = [os.getpid()]
    seen = set()
    total = 0
    while nodes:
        pid = nodes.pop()
        if pid in seen:
            continue
        seen.add(pid)
        try:
            status = Path(f"/proc/{pid}/status").read_text()
            total += int(next((l.split()[1] for l in status.splitlines() if l.startswith("VmRSS:")), "0"))
            for task in Path(f"/proc/{pid}/task").iterdir():
                try:
                    nodes.extend(int(x) for x in (task / "children").read_text().split())
                except OSError:
                    pass
        except OSError:
            pass
    return total

def run(cache, phase):
    session = cache / ("run-" + phase)
    session.mkdir()
    requests = session / "requests"
    requests.mkdir()
    stop = Event()
    errors = []
    logs = []
    def target():
        try:
            kwargs = {"jobs": args.jobs} if has_jobs else {}
            if args.dxc_helper:
                kwargs["dxc_helper"] = args.dxc_helper.resolve()
            worker.serve(session, stop, logs.append, **kwargs)
        except BaseException as exc:
            errors.append(repr(exc))
    started = {}
    observed = {}
    peak = process_rss()
    minimum = memory_available()
    swap = swap_activity()
    with patch.object(worker, "compiler_tools", return_value=tools):
        thread = Thread(target=target)
        thread.start()
        t0 = monotonic()
        try:
            pending = iter(inputs.items())
            next_item = next(pending, None)
            while len(observed) < len(inputs):
                now = monotonic()
                if errors or not thread.is_alive() or now - t0 > 100:
                    raise RuntimeError(f"worker failed or timed out: {errors}")
                while next_item and now >= t0 + len(started) * args.arrival_ms / 1000:
                    key, data = next_item
                    tmp = session / (key + ".tmp")
                    tmp.write_bytes(data)
                    started[key] = monotonic()
                    tmp.replace(requests / (key + ".dxbc"))
                    next_item = next(pending, None)
                for key in started.keys() - observed.keys():
                    out = session / (key + ".cso")
                    fail = session / (key + ".failed")
                    if out.is_file():
                        data = out.read_bytes()
                        if worker.portable_stage(data) != 5:
                            raise ValueError("invalid compute response")
                        observed[key] = {"seconds": monotonic() - started[key],
                                         "sha256": hashlib.sha256(data).hexdigest()}
                    elif fail.is_file():
                        raise RuntimeError(f"unexpected refusal or failure {key}: {logs[-4:]}")
                peak = max(peak, process_rss())
                minimum = min(minimum, memory_available())
                if minimum < 4 * 1024**2:
                    raise RuntimeError("memory guard exceeded")
                if len(observed) < len(inputs):
                    sleep(0.005)
            # All terminal replies are visible; wait for source cleanup too.
            while any(requests.iterdir()):
                if monotonic() - t0 > 105:
                    raise RuntimeError("cleanup timed out")
                sleep(0.005)
            elapsed = monotonic() - t0
        finally:
            stop.set()
            thread.join(90)
            if thread.is_alive():
                raise RuntimeError("worker did not stop")
    assert not errors, errors
    latency = sorted(row["seconds"] for row in observed.values())
    return {"phase": phase, "seconds": elapsed, "p50": statistics.median(latency),
            "p95": latency[(95 * len(latency) + 99)//100 - 1], "max": max(latency),
            "valid_per_second": len(inputs)/elapsed, "peak_rss_kib": peak,
            "min_mem_available_kib": minimum,
            "swap_delta_pages": [a-b for a,b in zip(swap_activity(), swap)],
            "timeouts": sum(row["seconds"] >= 90 for row in observed.values()),
            "results": observed}

fingerprint = hashes()
rows = []
for number in range(args.rounds):
    with TemporaryDirectory(prefix="scarlett-worker-benchmark-") as tmp:
        cache = Path(tmp)
        rows.extend([run(cache, "cold"), run(cache, "warm")])
assert hashes() == fingerprint, "compiler binaries changed"
receipt = {"repo": str(args.repo.resolve()), "jobs": args.jobs, "count": len(inputs),
           "arrival_ms": args.arrival_ms, "tools_sha256": fingerprint, "rows": rows}
with args.output.open("x") as out:
    json.dump(receipt, out, indent=2)
    out.write("\n")
for row in rows:
    print(f"{row['phase']} {row['seconds']:.3f}s, p95 {row['p95']:.3f}s, "
          f"RSS {row['peak_rss_kib']/1024:.1f} MiB, {row['timeouts']} timeouts", flush=True)
