#!/usr/bin/env python3
"""Measure GPU work in a chosen range of VulkanProfiler Event Trace frames."""
import argparse
from collections import Counter, defaultdict
import json
import math
from pathlib import Path
import re


# VulkanProfiler writes each complete event on one line, and may still be
# writing the current frame. Event Trace Format timestamps are microseconds.
def trace_events(path):
    with path.open(encoding="utf-8") as stream:
        for line in stream:
            if not line.lstrip().startswith('{'):
                continue
            try:
                yield json.loads(line.rstrip().removesuffix("]}").removesuffix(","))
            except json.JSONDecodeError:
                if '"displayTimeUnit"' in line or '"traceEvents"' in line:
                    continue
                if stream.read(1):
                    raise
                break


def union_duration(ranges):
    elapsed, end = 0, -math.inf
    for start, stop in sorted(ranges):
        elapsed += max(0, stop - max(end, start))
        end = max(end, stop)
    return elapsed


def frames(path, first, last):
    frame_id = None
    categories = ("Pipelines", "Render passes", "Command buffers")
    for event in trace_events(path):
        category, kind, label, tick = (event.get(k) for k in ("cat", "ph", "name", "ts"))
        if category == "Frames" and kind == "B" and (match := re.fullmatch(r"Frame #(\d+)", label)):
            frame_id = int(match[1])
            if frame_id > last:
                break
            start = tick
            pending = defaultdict(list)
            totals = {c: Counter() for c in categories}
            ranges = []
            reversed_ranges = 0
        elif frame_id is not None and first <= frame_id <= last:
            if category == "Frames" and kind == "E" and label == f"Frame #{frame_id}":
                if tick < start or any(pending.values()):
                    raise ValueError(f"Incomplete GPU timestamp range in frame {frame_id}")
                yield frame_id, (tick - start) / 1000, union_duration(ranges) / 1000, totals, reversed_ranges
                frame_id = None
            elif category in categories and kind in ("B", "E"):
                key = (category, event["tid"], label)
                if kind == "B":
                    pending[key].append(tick)
                elif not pending[key]:
                    raise ValueError(f"Unpaired GPU timestamp in frame {frame_id} for {label}")
                else:
                    begin = pending[key].pop()
                    if begin > tick:
                        # Vulkan timestamp stages can overlap. A backwards pair
                        # cannot supply a duration; count it in the receipt.
                        reversed_ranges += 1
                        continue
                    totals[category][label] += (tick - begin) / 1000
                    if category == "Command buffers":
                        ranges.append((begin, tick))


def summarize(path, first, last):
    if first < 0 or last < first:
        raise ValueError("Expected a nonnegative inclusive frame interval")
    samples = list(frames(path, first, last))
    if not samples or [sample[0] for sample in samples] != list(range(first, last + 1)):
        raise ValueError(f"Trace does not contain every completed frame from {first} through {last}")
    top = {}
    for category in ("Pipelines", "Render passes"):
        combined = Counter()
        for _, _, _, totals, _ in samples:
            combined.update(totals[category])
        top[category] = [
            {"name": name, "mean_ms_per_frame": round(total / len(samples), 3),
             "max_ms_per_frame": round(max(totals[category][name] for _, _, _, totals, _ in samples), 3)}
            for name, total in combined.most_common(12)
        ]
    return {
        "scope": "VulkanProfiler GPU timestamp pairs. Pipeline and render-pass totals sum work across queues; queues can overlap. Command-buffer active time is the union across queues.",
        "frame_interval_inclusive": [first, last],
        "frame_count": len(samples),
        "frames": [
            {"frame": frame, "first_to_last_gpu_timestamp_ms": round(span, 3),
             "command_buffer_active_ms": round(busy, 3), "reversed_ranges_skipped": reversed_count,
             "top_pipelines_ms": {name: round(ms, 3) for name, ms in totals["Pipelines"].most_common(3)}}
            for frame, span, busy, totals, reversed_count in samples
        ],
        "pipeline_timings": top["Pipelines"],
        "render_pass_timings": top["Render passes"],
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("trace", type=Path)
    parser.add_argument("--first", type=int, required=True)
    parser.add_argument("--last", type=int, required=True)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    try:
        report = json.dumps(summarize(args.trace, args.first, args.last), indent=2) + "\n"
        if args.output:
            args.output.write_text(report)
        else:
            print(report, end="")
    except (OSError, KeyError, ValueError) as error:
        parser.exit(1, f"GPU timing trace: {error}\n")
