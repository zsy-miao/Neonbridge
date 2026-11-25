"""Capture native vkd3d-proton pipeline creation and correlate it with Wine log timestamps."""
from collections import Counter
from contextlib import contextmanager
import heapq
import json
import math
from pathlib import Path
import re
from tempfile import mkdtemp

from .pipeline_prep import winpath


def summarize(trace):
    totals = {}
    pipeline_kinds = {}
    slowest = []
    with trace.open(encoding="utf-8", errors="replace") as stream:
        # vkd3d writes one event per line and can exit without closing the array.
        for index, line in enumerate(stream):
            if '"pso"' not in line:
                continue
            try:
                event = json.loads(line.strip().removesuffix(","))
            except json.JSONDecodeError:
                continue
            if not isinstance(event, dict) or event.get("pid") != "pso" or event.get("ph") != "X":
                continue
            name = event.get("name")
            if not isinstance(name, str):
                continue
            match = re.fullmatch(r"([0-9a-f]{16}) (\S.*)", name)
            if not match:
                continue
            duration, timestamp = event.get("dur"), event.get("ts")
            if any(type(value) not in (int, float) for value in (duration, timestamp)):
                continue
            try:
                duration /= 1000
                timestamp /= 1_000_000
            except OverflowError:
                continue
            if not math.isfinite(duration) or not math.isfinite(timestamp):
                continue
            key, operation = match.groups()
            label = operation.split()[0]
            if label in ("GFX", "COMP", "LINK"):
                pipeline_kinds[key] = "compute" if label == "COMP" else "graphics"
            kind = "driver_internal" if label == "META" else pipeline_kinds.get(key, "unknown")
            total = totals.setdefault(kind, Counter())
            total["calls"] += 1
            total["over_100_ms"] += duration >= 100
            total["total_ms"] += duration
            row = dict(pipeline_key=key, operation=operation, duration_ms=round(duration, 3),
                       type=kind, wine_seconds=round(timestamp, 6))
            heapq.heappush(slowest, (duration, index, row))
            if len(slowest) > 20:
                heapq.heappop(slowest)
    for group in totals.values():
        group["total_ms"] = round(group["total_ms"], 3)
    return dict(groups=totals, slowest=[item[2] for item in sorted(slowest, reverse=True)])


@contextmanager
def pipeline_capture(backend, env, log):
    if backend not in ("wine", "proton"):
        raise ValueError("--profile-pipelines requires a Linux Wine or Proton backend with vkd3d-proton")
    directory = Path(mkdtemp(prefix="pipelines-", dir=log.path.parent)).resolve()
    trace = directory / "timeline.json"
    env.update(VKD3D_QUEUE_PROFILE=winpath(trace), VKD3D_QUEUE_PROFILE_ABSOLUTE="1")
    log.status(f"Pipeline timing capture: {trace}")
    try:
        yield
    finally:
        if trace.is_file():
            try:
                result = summarize(trace)
                summary = directory / "summary.json"
                summary.write_text(json.dumps(result, indent=2) + "\n")
                log.status(f"Pipeline timings: {summary}")
            except (OSError, ValueError) as error:
                log.status(f"Could not summarize pipeline timings: {error}")
        else:
            log.status(f"The backend did not produce a pipeline trace at {trace}")


if __name__ == "__main__":
    import argparse

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("trace", type=Path, help="vkd3d-proton queue timeline from the game")
    print(json.dumps(summarize(parser.parse_args().trace), indent=2))
