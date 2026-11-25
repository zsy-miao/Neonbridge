#!/usr/bin/env python3
"""Prioritize shader incompatibilities and prepare exact local replay sets.

Consume a runtime log (observations only) or a paired production-compiler
comparison. No title bytes are copied, generated, uploaded or required to
rank a log. Replay manifests contain request names only; the owner must
already have matching recorded inputs. Review reports before sharing.
"""
import argparse
from collections import Counter, defaultdict
import json
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from project_tuxbox.runtime_shaders import REQUEST_PATTERNS
from project_tuxbox.shader_diagnostics import refusal_detail
from tools.triage_scarlett_shaders import LOG_REFUSAL

STATUSES = {"compiled", "refused", "failed"}


def valid_name(name):
    return isinstance(name, str) and any(
        name.endswith(ext) and re.fullmatch(pattern, name[:-len(ext)])
        for ext, pattern in REQUEST_PATTERNS.items()
    )


def key_name(key):
    matches = [key + ext for ext, pattern in REQUEST_PATTERNS.items()
               if re.fullmatch(pattern, key)]
    if len(matches) != 1:
        raise ValueError("ambiguous request key")
    return matches[0]


def group_id(detail):
    # Retain opcode distinctions even when the same normalized reason occurs
    # on different instructions. Sampler words are tracked as a subgroup.
    return (detail.get("reason") or "(no message)", detail.get("opcode") or "")


def log_rows(lines):
    observed = {}
    events = 0
    for line in lines:
        match = LOG_REFUSAL.search(line.rstrip())
        if match:
            key, reason = match.groups()
            observed[key_name(key)] = {"status": "refused", **refusal_detail(reason)}
            events += 1
    return [{"request": name, "after": result} for name, result in sorted(observed.items())], events


def paired_rows(report):
    if not isinstance(report, dict) or not isinstance(report.get("rows"), list):
        raise ValueError("expected paired compiler comparison JSON")
    rows = report["rows"]
    if not rows or report.get("requests") != len(rows):
        raise ValueError("invalid request count")
    seen = set()
    matrix = Counter()
    for row in rows:
        if not isinstance(row, dict) or not valid_name(row.get("request")) or row["request"] in seen:
            raise ValueError("invalid or duplicate request name")
        seen.add(row["request"])
        for side in ("before", "after"):
            detail = row.get(side)
            if not isinstance(detail, dict) or detail.get("status") not in ("compiled", "refused", "failed"):
                raise ValueError("invalid comparison status")
            for field in ("reason", "opcode"):
                if detail.get(field) is not None and not isinstance(detail[field], str):
                    raise ValueError("invalid refusal detail")
            words = detail.get("sampler_words")
            if words is not None and (not isinstance(words, list) or len(words) != 4
                                      or any(not isinstance(word, str) or not re.fullmatch(r"0x[0-9a-f]{1,8}", word) for word in words)):
                raise ValueError("invalid sampler words")
            if detail["status"] == "compiled" and not re.fullmatch(
                r"[0-9a-f]{64}", str(detail.get("dxil_sha256", ""))
            ):
                raise ValueError("missing validated output digest")
        matrix[row["before"]["status"], row["after"]["status"]] += 1
    # Do not rely on a summary that may have gone stale after editing rows.
    expected = {a: {b: matrix[a, b] for b in STATUSES} for a in STATUSES}
    if report.get("transitions") != expected:
        raise ValueError("comparison transition matrix does not match rows")
    return rows


def build_worklist(rows, paired, events=None):
    if not rows:
        raise ValueError("no observed requests")
    remaining = defaultdict(list)
    origins = defaultdict(Counter)
    moved = defaultdict(Counter)
    variants = defaultdict(Counter)
    transitions = Counter()
    regressions = []
    changed_outputs = []
    for row in rows:
        name = row["request"]
        after = row["after"]
        status = after["status"]
        before = row.get("before")
        if paired:
            old = before["status"]
            transitions[old, status] += 1
            if old == "compiled":
                if status != "compiled":
                    regressions.append(name)
                elif before["dxil_sha256"] != after["dxil_sha256"]:
                    changed_outputs.append(name)
            if old == "refused":
                group = group_id(before)
                outcome = status
                if status == "refused" and group != group_id(after):
                    outcome = "different_refusal"
                    moved[group][group_id(after)] += 1
                elif status == "refused" and before.get("sampler_words") != after.get("sampler_words"):
                    outcome = "different_variant"
                origins[group][outcome] += 1
        if status == "refused":
            group = group_id(after)
            remaining[group].append(name)
            if after.get("sampler_words"):
                variants[group][tuple(after["sampler_words"])] += 1

    def describe(group):
        return {"reason": group[0], "opcode": group[1] or None}

    # Groups are disjoint and rank by observed distinct requests, not an
    # estimated number a patch will recover. Equal counts sort deterministically.
    groups = []
    for group, names in sorted(remaining.items(), key=lambda item: (-len(item[1]), item[0])):
        groups.append({
            "rank": len(groups) + 1, **describe(group), "count": len(names),
            "requests": sorted(names),
            "sampler_variants": [{"words": list(words), "count": count}
                                 for words, count in sorted(variants[group].items(), key=lambda i: (-i[1], i[0]))],
        })
    history = []
    for group, outcomes in sorted(origins.items(), key=lambda i: (-sum(i[1].values()), i[0])):
        history.append({**describe(group), "before": sum(outcomes.values()),
                        "recovered": outcomes["compiled"],
                        "still_same": outcomes["refused"],
                        "different_refusal": outcomes["different_refusal"],
                        "different_variant": outcomes["different_variant"],
                        "inconclusive": outcomes["failed"],
                        "next_blockers": [{**describe(g), "count": n}
                                          for g, n in sorted(moved[group].items(), key=lambda i: (-i[1], i[0]))]})
    counts = Counter(row["after"]["status"] for row in rows)
    return {
        "schema": 1, "mode": "paired" if paired else "log",
        "scope": ("Same-request compilation outcomes, not rendering or gameplay" if paired else
                  "Distinct observed refusals only; missing log entries do not prove recovery"),
        "totals": ({"requests": len(rows), **{s: counts[s] for s in sorted(STATUSES)}} if paired
                   else {"refused": counts["refused"], "refusal_events": events}),
        "groups": groups, "history": history,
        "recovered": transitions["refused", "compiled"] if paired else None,
        "regressions": sorted(regressions) if paired else None,
        "changed_compiled_outputs": sorted(changed_outputs) if paired else None,
        "inconclusive": sum(n for (a, b), n in transitions.items() if "failed" in (a, b)) if paired else None,
    }


def apply_claims(report, claims):
    """Explicit ownership hints, never evidence of recovered compatibility."""
    if not isinstance(claims, list):
        raise ValueError("claims must be a list")
    checked = []
    for claim in claims:
        if not isinstance(claim, dict) or set(claim) - {"ref", "reason", "opcode"}:
            raise ValueError("invalid claim fields")
        ref = claim.get("ref")
        selectors = {k: claim[k] for k in ("reason", "opcode") if k in claim}
        if not isinstance(ref, str) or not ref.strip() or not selectors or any(
            not isinstance(v, str) or not v.strip() for v in selectors.values()
        ):
            raise ValueError("claim needs a reference and an exact reason or opcode")
        checked.append((ref, selectors))
    matched = set()
    for group in report["groups"]:
        refs = set()
        for i, (ref, selectors) in enumerate(checked):
            if all(group.get(k) == v for k, v in selectors.items()):
                refs.add(ref)
                matched.add(i)
        group["claims"] = sorted(refs)
    report["unclaimed_ranks"] = [g["rank"] for g in report["groups"] if not g["claims"]]
    report["overlapping_claim_ranks"] = [g["rank"] for g in report["groups"] if len(g["claims"]) > 1]
    report["unmatched_claims"] = [claims[i] for i in range(len(claims)) if i not in matched]
    return report


def manifest(report, ranks):
    groups = report["groups"]
    selected = [g for g in groups if g["rank"] in ranks]
    if not selected or set(ranks) != {g["rank"] for g in selected}:
        raise ValueError("unknown or empty group selection")
    names = sorted({name for group in selected for name in group["requests"]})
    return {"schema": 1, "scope": "local replay selection, not an expected success claim",
            "groups": [{k: g[k] for k in ("rank", "reason", "opcode", "count")} for g in selected],
            "requests": names}


def selected_sources(sources, selection):
    if not isinstance(selection, dict) or selection.get("schema") != 1:
        raise ValueError("invalid replay manifest schema")
    names = selection.get("requests")
    if not isinstance(names, list) or not names or not all(valid_name(n) for n in names):
        raise ValueError("invalid or empty request selection")
    if len(set(names)) != len(names):
        raise ValueError("duplicate replay request")
    available = {p.name: p for p in sources}
    missing = sorted(set(names) - available.keys())
    if missing:
        raise ValueError(f"{len(missing)} selected requests missing locally (first: {missing[0]})")
    return [available[name] for name in sorted(names)]


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--log", type=Path)
    source.add_argument("--comparison", type=Path)
    parser.add_argument("--json", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, help="write exact replay selection")
    selection_mode = parser.add_mutually_exclusive_group()
    selection_mode.add_argument("--group", type=int, action="append", help="rank to replay (repeatable)")
    selection_mode.add_argument("--protect-compiled", action="store_true",
                                help="freeze all successful candidate requests as regression controls")
    parser.add_argument("--top", type=int, default=20)
    parser.add_argument("--claims", type=Path, help="optional explicit PR/issue ownership hints (JSON)")
    args = parser.parse_args(argv)
    if bool(args.manifest) != bool(args.group or args.protect_compiled):
        parser.error("--manifest requires --group or --protect-compiled, and vice versa")
    if args.protect_compiled and args.log:
        parser.error("regression controls require a paired comparison")
    if args.top < 1:
        parser.error("--top must be positive")
    outputs = [args.json] + ([args.manifest] if args.manifest else [])
    if len({p.resolve() for p in outputs}) != len(outputs) or any(p.exists() for p in outputs):
        parser.error("use distinct fresh output paths")
    try:
        if args.log:
            with args.log.open(encoding="utf-8", errors="replace") as stream:
                rows, events = log_rows(stream)
            report = build_worklist(rows, False, events)
        else:
            source_report = json.loads(args.comparison.read_text())
            rows = paired_rows(source_report)
            report = build_worklist(rows, True)
            report["tool_sha256"] = source_report.get("tool_sha256")
        if args.claims:
            apply_claims(report, json.loads(args.claims.read_text()))
        selection = manifest(report, args.group) if args.group else None
        if args.protect_compiled:
            names = sorted(row["request"] for row in rows if row["after"]["status"] == "compiled")
            if not names:
                raise ValueError("no compiled requests to protect")
            selection = {"schema": 1, "scope": "previously validated compilation successes",
                         "requests": names,
                         "expected": {name: {"status": "compiled"} for name in names}}
    except (OSError, ValueError) as exc:
        parser.error(str(exc))
    for path, obj in zip(outputs, [report, selection]):
        with path.open("x", encoding="utf-8") as stream:
            json.dump(obj, stream, indent=2)
            stream.write("\n")
    print(report["scope"])
    for g in report["groups"][:args.top]:
        owners = ", ".join(g.get("claims", []))
        print(f"{g['rank']:3}. {g['count']:6}  {g['opcode'] or '-'}: {g['reason']}"
              + (f" [in progress: {owners}]" if owners else ""))
    if args.claims:
        print("Unclaimed ranks:", ", ".join(map(str, report["unclaimed_ranks"])) or "none")
        print("Overlapping claims:", ", ".join(map(str, report["overlapping_claim_ranks"])) or "none")
    if report["mode"] == "paired":
        print(f"{report['recovered']} recovered, {len(report['regressions'])} regressions, "
              f"{report['inconclusive']} inconclusive, "
              f"{len(report['changed_compiled_outputs'])} changed compiled outputs")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
