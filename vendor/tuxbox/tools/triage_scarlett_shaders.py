#!/usr/bin/env python3
"""Rank the reasons xshader refuses ProjectScarlett's recorded shader requests.

Preparation and the runtime worker keep only a count of refused requests. This
tool runs the HLSL emitter alone (no DXC) on each recorded request and groups
the refusal messages. Program counters and register numbers are masked, so
identical gaps in different shaders share one row.

Example:
    python tools/triage_scarlett_shaders.py \\
        "$WINEPREFIX/drive_c/users/steamuser/AppData/Local/Temp/tuxbox_shader_cache/projectscarlett/<exe sha256>/requests" \\
        --json /tmp/scarlett-refusals.json

The cache is read only. The report holds xshader messages and request digests,
not game shader bytes. A runtime log can be ranked without a compiler or
recorded request files:
    python tools/triage_scarlett_shaders.py --log scarlett.log --json refusals.json

Log mode reports observed refusals, not all requests or current coverage.
"""
import argparse
from collections import Counter, defaultdict
from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import re
import subprocess
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from project_tuxbox.runtime_shaders import REQUEST_PATTERNS, XSHADER_UNSUPPORTED_HLSL, ShaderRefused
from project_tuxbox.scarlett_report import refusal_bucket, failure_bucket

from project_tuxbox.shader_diagnostics import normalize_refusal as normalize
from tools.scarlett_replay_integrity import snapshot_request, pin_emitter
from project_tuxbox.inspect import digest


def construct(reason):
    """Coarse group: the instruction or construct named before ' at 0x'."""
    head, found, _ = reason.partition(" at 0x_")
    return head if found else reason.split(":", 1)[0]


def requests(directory):
    return sorted(
        path for path in directory.iterdir()
        if path.suffix in REQUEST_PATTERNS and re.fullmatch(REQUEST_PATTERNS[path.suffix], path.stem)
    )


def command(xshader, source, work):
    """The emitter invocation runtime_shaders.compile_request uses for this request."""
    flags = []
    cooked = source
    if source.suffix == ".link":
        cooked = source / "producer.dxbc"
        flags = ["--link-pixel", source / "pixel.dxbc", "--pixel-order", source / "pixel.cso"]
    elif source.suffix == ".stencil":
        stem, sep, target = source.stem.rpartition("_rt")
        if not sep:
            raise ValueError(f"stencil request without a render-target suffix: {source.name}")
        flags = ["--stencil-target", target]
    return [xshader, cooked, *flags, "--hlsl-out", work / "shader.hlsl", "--profile-out", work / "profile"]


def probe(xshader, source, timeout, *, aggregate_only=False):
    with TemporaryDirectory(prefix="tuxbox-triage-") as tmp:
        try:
            run = subprocess.run(command(xshader, source, Path(tmp)), capture_output=True, text=True,
                                 errors="replace", timeout=timeout)
        except subprocess.TimeoutExpired as exc:
            return source, "failed", failure_bucket(exc) if aggregate_only else f"timed out after {timeout:g}s"
        except (OSError, ValueError) as exc:
            return source, "failed", failure_bucket(exc) if aggregate_only else str(exc)
    if run.returncode == 0:
        return source, "translated", None
    status = "refused" if run.returncode == XSHADER_UNSUPPORTED_HLSL else "failed"
    if aggregate_only:
        reason = (refusal_bucket(ShaderRefused(run.stderr or run.stdout, cached=False))
                  if status == "refused" else "emitter_failure")
        return source, status, reason
    return source, status, normalize(run.stderr or run.stdout)


def triage(xshader, sources, jobs, timeout, examples, *, aggregate_only=False):
    totals = Counter()
    reasons = Counter()
    samples = defaultdict(list)
    failures = Counter()
    with ThreadPoolExecutor(max_workers=jobs) as pool:
        for source, status, reason in pool.map(lambda s: probe(xshader, s, timeout, aggregate_only=aggregate_only), sources):
            totals[status] += 1
            if status == "refused":
                reasons[reason] += 1
                if not aggregate_only and len(samples[reason]) < examples:
                    samples[reason].append(source.name)
            elif status == "failed":
                failures[reason] += 1
    groups = Counter()
    for reason, count in reasons.items():
        groups[construct(reason)] += count
    return {
        **({"aggregate_only": True} if aggregate_only else {"xshader": str(xshader)}),
        "totals": {"requests": len(sources), **{k: totals[k] for k in ("translated", "refused", "failed")}},
        "constructs": [{"construct": name, "count": count} for name, count in groups.most_common()],
        "reasons": [{"reason": reason, "count": count, "examples": samples[reason]}
                    for reason, count in reasons.most_common()],
        "failures": [{"reason": reason, "count": count} for reason, count in failures.most_common()],
    }


# The runtime logs each newly refused key; cache hits only appear in aggregate.
# Restrict the parser to the exact reserved emitter failure status and key forms.
LOG_REFUSAL = re.compile(
    r"Xbox shader (" + "|".join(REQUEST_PATTERNS.values())
    + r"): xshader(?:\.exe)? failed \(65\): (.*)$"
)
SAMPLER_WORDS = re.compile(
    r"not a sampler descriptor .* words \["
    r"(0x[0-9a-fA-F]+), (0x[0-9a-fA-F]+), (0x[0-9a-fA-F]+), (0x[0-9a-fA-F]+)\]$"
)


def triage_log(lines, examples):
    """Rank distinct refused keys, using the last observed refusal for each."""
    observed = {}
    events = 0
    for line in lines:
        match = LOG_REFUSAL.search(line.rstrip())
        if match is None:
            continue
        key, message = match.groups()
        observed[key] = message
        events += 1
    reasons = Counter()
    groups = Counter()
    states = Counter()
    samples = defaultdict(list)
    for key, message in sorted(observed.items()):
        reason = normalize(message)
        reasons[reason] += 1
        groups[construct(reason)] += 1
        if len(samples[reason]) < examples:
            samples[reason].append(key)
        if match := SAMPLER_WORDS.search(message):
            states[tuple(int(word, 16) for word in match.groups())] += 1

    return {
        "source": "log",
        "totals": {"refused": len(observed), "refusal_events": events},
        "constructs": [{"construct": name, "count": count} for name, count in groups.most_common()],
        "reasons": [{"reason": reason, "count": count, "examples": samples[reason]}
                    for reason, count in reasons.most_common()],
        "sampler_states": [{"words": [f"{word:#x}" for word in state], "count": count}
                           for state, count in states.most_common()],
        "failures": [],
    }


def paired_aggregate(baseline, candidate, sources, jobs, timeout):
    """Same enumerated requests, emitter only, fixed labels and no identifiers."""
    totals = {"baseline": Counter(), "candidate": Counter()}
    transitions = Counter()
    buckets = defaultdict(Counter)

    with TemporaryDirectory(prefix="tuxbox-triage-tools-") as temp:
        old_tool, old_hash = pin_emitter(baseline, Path(temp) / "baseline")
        new_tool, new_hash = pin_emitter(candidate, Path(temp) / "candidate")

        def compare(source):
            with TemporaryDirectory(prefix="tuxbox-triage-input-") as tmp:
                try:
                    snap = snapshot_request(source, Path(tmp))
                except (OSError, ValueError):
                    return "failed", "invalid_input", "failed", "invalid_input"
                _, old, old_reason = probe(old_tool, snap, timeout, aggregate_only=True)
                _, new, new_reason = probe(new_tool, snap, timeout, aggregate_only=True)
                return old, old_reason, new, new_reason

        failures = {"baseline": Counter(), "candidate": Counter()}
        with ThreadPoolExecutor(max_workers=jobs) as pool:
            for old, old_reason, new, new_reason in pool.map(compare, sources):
                totals["baseline"][old] += 1
                totals["candidate"][new] += 1
                transitions[(old, new)] += 1
                if old == "refused":
                    buckets[old_reason][new] += 1
                if old == "failed":
                    failures["baseline"][old_reason] += 1
                if new == "failed":
                    failures["candidate"][new_reason] += 1
        if digest(old_tool) != old_hash or digest(new_tool) != new_hash:
            raise ValueError("pinned emitter changed during comparison")
    return {
        "schema": 1,
        "aggregate_only": True,
        "scope": "Digest-verified per-request snapshots; emitter only, not DXC, GPU or gameplay.",
        "tool_sha256": {"baseline": old_hash, "candidate": new_hash},
        "failure_buckets": {side: dict(sorted(counts.items())) for side, counts in failures.items()},
        "requests": len(sources),
        "totals": {side: {state: counts[state] for state in ("translated", "refused", "failed")}
                   for side, counts in totals.items()},
        "transitions": [{"baseline": a, "candidate": b, "count": n}
                        for (a,b), n in sorted(transitions.items())],
        "baseline_refusal_buckets": {label: {state: counts[state] for state in ("translated", "refused", "failed")}
                                    for label, counts in sorted(buckets.items())},
        "recovered": transitions["refused", "translated"],
        "regressed": transitions["translated", "refused"],
        "inconclusive": sum(n for (a,b),n in transitions.items() if "failed" in (a,b)),
    }


def markdown(report, top):
    refused = report["totals"]["refused"] or 1
    cell = lambda text: text.replace("|", "\\|")
    if report.get("source") == "log":
        lines = [
            "Observed refusals: {refused:,} distinct keys, {refusal_events:,} log events.".format(**report["totals"]),
            "Not a coverage estimate; cached refusals and unlogged requests are excluded.",
        ]
    else:
        lines = [
            "| Requests | Translated | Refused | Failed |",
            "| ---: | ---: | ---: | ---: |",
            "| {requests:,} | {translated:,} | {refused:,} | {failed:,} |".format(**report["totals"]),
        ]
    lines += [
        "",
        "| Construct | Refused | Share |",
        "| --- | ---: | ---: |",
        *(f"| `{cell(row['construct'])}` | {row['count']:,} | {100 * row['count'] / refused:.1f}% |"
          for row in report["constructs"][:top]),
        "",
        "| Refusal | Requests | Share |",
        "| --- | ---: | ---: |",
        *(f"| `{cell(row['reason'])}` | {row['count']:,} | {100 * row['count'] / refused:.1f}% |"
          for row in report["reasons"][:top]),
    ]
    if report.get("sampler_states"):
        lines += ["", "| Literal sampler words | Refused keys |", "| --- | ---: |",
                  *(f"| `{', '.join(row['words'])}` | {row['count']:,} |"
                    for row in report["sampler_states"][:top])]
    if report["failures"]:
        lines += ["", "Failures other than refusals (input or tool errors):", "",
                  *(f"- {row['count']:,} × `{row['reason']}`" for row in report["failures"][:top])]
    return "\n".join(lines)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n", 1)[0])
    parser.add_argument("requests", type=Path, help="recorded request directory, or runtime log with --log")
    parser.add_argument("--log", action="store_true", help="rank recorded refusals without running a compiler")
    parser.add_argument("--xshader", type=Path, help="host xshader (default: build it like shader preparation)")
    parser.add_argument("--baseline", type=Path, help="compare a baseline emitter with --xshader using --aggregate-only")
    parser.add_argument("--jobs", type=int, default=os.cpu_count() or 1)
    parser.add_argument("--timeout", type=float, default=30, help="seconds per request, as at runtime")
    parser.add_argument("--top", type=int, default=25, help="rows per table")
    parser.add_argument("--examples", type=int, default=3, help="request names kept per refusal in --json")
    parser.add_argument("--limit", type=int, default=0, help="only the first N requests")
    parser.add_argument("--json", type=Path, help="write the full report here")
    parser.add_argument("--aggregate-only", action="store_true", help="fixed reason buckets only; omit paths, request names and raw error text")
    args = parser.parse_args(argv)

    if args.json and (args.json.exists() or (not args.log and
            args.json.resolve().is_relative_to(args.requests.resolve()))):
        parser.error("choose a fresh report outside the request archive")
    if args.limit < 0:
        parser.error("--limit must be 0 (all) or positive")
    if args.baseline is not None and (not args.aggregate_only or args.xshader is None):
        parser.error("--baseline requires --aggregate-only and an explicit --xshader")
    if not 1 <= args.jobs <= 256 or not 0 < args.timeout <= 3600:
        parser.error("jobs must be 1..256 and timeout in (0, 3600]")
    if args.log:
        if args.xshader is not None or args.limit or args.aggregate_only or args.baseline is not None:
            parser.error("--log cannot be combined with --xshader, --limit, --baseline or --aggregate-only")
        if not args.requests.is_file():
            parser.error(f"not a log file: {args.requests}")
        with args.requests.open(encoding="utf-8", errors="replace") as stream:
            report = triage_log(stream, args.examples)
    else:
        if not args.requests.is_dir():
            parser.error(f"not a directory: {args.requests}")
        sources = requests(args.requests)[:args.limit or None]
        if not sources:
            parser.error(f"no recorded shader requests in {args.requests}")
        if args.xshader is None:
            from project_tuxbox.shader_prep import shader_compiler
            args.xshader = shader_compiler()

        if args.baseline is not None:
            try:
                report = paired_aggregate(args.baseline, args.xshader, sources, args.jobs, args.timeout)
            except (OSError, ValueError):
                parser.error("comparison integrity or tool setup failed")
            # Refuse overwrites and export only the fixed aggregate schema.
            if args.json:
                with args.json.open("x", encoding="utf-8") as output:
                    json.dump(report, output, indent=2)
                    output.write("\n")
            print(json.dumps(report, indent=2))
            return int(bool(report["regressed"] or report["inconclusive"]))
        report = triage(args.xshader, sources, max(1, args.jobs), args.timeout, args.examples, aggregate_only=args.aggregate_only)
    print(markdown(report, args.top))
    if args.json:
        with args.json.open("x", encoding="utf-8") as output:
            json.dump(report, output, indent=2)
            output.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
