#!/usr/bin/env python3
"""Compare two xshader builds against identical recorded requests, CPU only.

Each request passes through the production HLSL -> DXC -> DXV pipeline in
a fresh temporary cache. No GPU, live cache changes or shader exports.
An accepted result is validated DXIL, not proof of correct rendering.
"""
import argparse
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path
import subprocess
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from project_tuxbox.inspect import digest
from project_tuxbox.runtime_shaders import compile_request, ShaderRefused, ToolFailure
from project_tuxbox.shader_diagnostics import normalize_refusal, refusal_detail
from tools.triage_scarlett_shaders import requests
from tools.scarlett_replay_integrity import snapshot_request, pin_emitter
from project_tuxbox.scarlett_report import refusal_bucket, failure_bucket


def compile_one(source, tools, *, aggregate_only=False):
    with TemporaryDirectory(prefix="tuxbox-compare-") as temp:
        cache = Path(temp)
        try:
            if not compile_request(source, source.stem, tools, cache, None):
                raise ValueError("unexpected hit in a fresh cache")
            return {"status": "compiled", "dxil_sha256": digest(cache / (source.stem + ".cso"))}
        except ShaderRefused as exc:
            return {"status": "refused", "bucket": refusal_bucket(exc)} if aggregate_only else {"status": "refused", **refusal_detail(str(exc))}
        except (OSError, ValueError, ToolFailure, subprocess.SubprocessError) as exc:
            if aggregate_only:
                bucket = (exc.tool + "_failure" if isinstance(exc, ToolFailure)
                          and exc.tool in ("xshader", "dxc", "dxv") else failure_bucket(exc))
                return {"status": "failed", "bucket": bucket}
            return {"status": "failed", "reason": normalize_refusal(str(exc))}


def compare(sources, baseline, candidate, dxc, dxv, jobs, *, aggregate_only=False):
    # Pin both emitters before the first request so a concurrent build cannot
    # change the executable underneath the comparison.
    with TemporaryDirectory(prefix="tuxbox-compare-tools-") as temp:
        pinned, fingerprints = [], {}
        for name, source in (("baseline", baseline), ("candidate", candidate)):
            dest, fingerprints[name] = pin_emitter(source, Path(temp) / name)
            pinned.append(dest)
        fingerprints.update(dxc=digest(dxc), dxv=digest(dxv))

        def pair(source):
            # Keep one verified private snapshot per active pair. Do not
            # stage the entire title archive or retain any source bytes.
            with TemporaryDirectory(prefix="tuxbox-compare-input-") as input_dir:
                try:
                    snap = snapshot_request(source, Path(input_dir))
                except (OSError, ValueError) as exc:
                    failed = {"status": "failed", "bucket": "invalid_input"} if aggregate_only else {
                        "status": "failed", "reason": normalize_refusal(str(exc))}
                    return {"request": source.name, "before": failed, "after": failed}
                kwargs = {"aggregate_only": True} if aggregate_only else {}
                before = compile_one(snap, (pinned[0], dxc, dxv), **kwargs)
                after = compile_one(snap, (pinned[1], dxc, dxv), **kwargs)
                return {"request": source.name, "before": before, "after": after}

        with ThreadPoolExecutor(max_workers=jobs) as pool:
            rows = list(pool.map(pair, sources))
        if any(fingerprints[name] != digest(path) for name,path in
               zip(("baseline","candidate","dxc","dxv"), (*pinned,dxc,dxv))):
            raise ValueError("compiler executable changed during comparison")

    counts = Counter((row["before"]["status"], row["after"]["status"]) for row in rows)
    statuses = ("compiled", "refused", "failed")
    report = {
        "scope": "CPU-only production compiler pipeline; no rendering or retail gameplay claim",
        "tool_sha256": fingerprints,
        "requests": len(rows),
        "before": {s: sum(n for (a, b), n in counts.items() if a == s) for s in statuses},
        "after": {s: sum(n for (a, b), n in counts.items() if b == s) for s in statuses},
        "recovered": counts["refused", "compiled"],
        "regressed": counts["compiled", "refused"],
        "candidate_failures_after_success": counts["compiled", "failed"],
        "inconclusive": sum(n for (a, b), n in counts.items() if "failed" in (a, b)),
        "transitions": {a: {b: counts[a, b] for b in statuses} for a in statuses},
        "changed_compiled_outputs": sum(
            row["before"]["status"] == row["after"]["status"] == "compiled"
            and row["before"].get("dxil_sha256") != row["after"].get("dxil_sha256")
            for row in rows),
    }
    if aggregate_only:
        # All dynamic text and shader identifiers stay in the private process.
        report["aggregate_only"] = True
        report["schema"] = 1
        report["buckets"] = {}
        for side in ("before", "after"):
            report["buckets"][side] = {}
            for state in ("refused", "failed"):
                report["buckets"][side][state] = dict(sorted(Counter(
                    row[side]["bucket"] for row in rows if row[side]["status"] == state).items()))
        by_bucket = {}
        for row in rows:
            if row["before"]["status"] == "refused":
                bucket = row["before"]["bucket"]
                by_bucket.setdefault(bucket, dict.fromkeys(statuses, 0))[row["after"]["status"]] += 1
        report["baseline_refusal_buckets"] = dict(sorted(by_bucket.items()))
    else:
        report["rows"] = rows
    return report


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("requests", type=Path)
    for name in ("baseline", "candidate", "dxc", "dxv", "json"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--expect-candidate", type=Path, help="require exact candidate status for every selected request")
    parser.add_argument("--require-no-failures", action="store_true", help="fail if either compiler has an inconclusive request")
    parser.add_argument("--jobs", type=int, default=2)
    parser.add_argument("--aggregate-only", action="store_true", help="omit request identifiers and raw errors; retain fixed counts and tool hashes")
    parser.add_argument("--require-identical-dxil", action="store_true", help="fail if both compilers succeed but output bytes differ")
    parser.add_argument("--limit", type=int, default=0, help="first N requests, zero for all")
    parser.add_argument("--select", type=Path, help="exact local request names from a worklist manifest")
    args = parser.parse_args(argv)
    if not 1 <= args.jobs <= 8 or args.limit < 0:
        parser.error("--jobs must be 1..8 and --limit nonnegative")
    if not args.requests.is_dir():
        parser.error("recorded request directory required")
    if args.json.resolve().is_relative_to(args.requests.resolve()):
        parser.error("write the report outside the recorded request directory")
    if args.json.exists():
        parser.error("report already exists; choose a new output path")
    sources = requests(args.requests)
    selection = None
    if args.select:
        if args.limit:
            parser.error("--select cannot be combined with --limit")
        try:
            from tools.scarlett_worklist import selected_sources
            selection = json.loads(args.select.read_text())
            sources = selected_sources(sources, selection)
        except (OSError, ValueError) as exc:
            parser.error(str(exc))
    else:
        sources = sources[:args.limit or None]
    if not sources:
        parser.error("no recorded requests")
    expected = None
    if args.expect_candidate or (selection is not None and "expected" in selection):
        try:
            if args.expect_candidate and selection is not None and "expected" in selection:
                raise ValueError("selection already contains regression expectations")
            expected = json.loads(args.expect_candidate.read_text()) if args.expect_candidate else selection["expected"]
            if not isinstance(expected, dict) or set(expected) != {source.name for source in sources}:
                raise ValueError("expectations must cover exactly the selected requests")
            if any(not isinstance(row, dict) or row.get("status") not in ("compiled", "refused", "failed")
                   for row in expected.values()):
                raise ValueError("invalid expected candidate status")
        except (OSError, ValueError) as exc:
            parser.error(str(exc))
    if args.aggregate_only and expected is not None:
        parser.error("aggregate-only cannot export per-request expectations")
    try:
        report = compare(sources, args.baseline.resolve(), args.candidate.resolve(),
                         args.dxc.resolve(), args.dxv.resolve(), args.jobs,
                         aggregate_only=args.aggregate_only)
    except (OSError, ValueError):
        if args.aggregate_only:
            parser.error("comparison integrity or tool setup failed")
        raise
    if expected is not None:
        report["expectation_mismatches"] = [
            {"request": row["request"], "expected": expected[row["request"]]["status"],
             "actual": row["after"]["status"]}
            for row in report["rows"] if row["after"]["status"] != expected[row["request"]]["status"]
        ]
    with args.json.open("x", encoding="utf-8") as output:
        json.dump(report, output, indent=2)
        output.write("\n")
    print(f"{report['requests']} paired requests: {report['before']['compiled']} -> "
          f"{report['after']['compiled']} compiled; {report['recovered']} recovered, "
          f"{report['regressed']} regressed, {report['inconclusive']} inconclusive.")
    # Fail a regression gate even if an earlier success now has a retryable
    # error; distinguish that from a confirmed emitter refusal in the report.
    return int(bool(report["regressed"] or report["candidate_failures_after_success"]
                    or report.get("expectation_mismatches")
                    or (args.require_no_failures and report["inconclusive"])
                    or (args.require_identical_dxil and report["changed_compiled_outputs"])))


if __name__ == "__main__":
    raise SystemExit(main())
