#!/usr/bin/env python3
"""Correlate a launch window with offline Linux kernel GPU fault records.

Input: journalctl -k -b -o json --since ... --until ...
A temporal overlap is evidence, not a causal attribution. No raw messages
or host identifiers are copied to the report.
"""
import argparse
from collections import Counter
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import re

XID = re.compile(r"\bNVRM:\s*Xid\s*\(PCI:([0-9a-fA-F]{4}:[0-9a-fA-F]{2}:[0-9a-fA-F]{2}(?:\.[0-7])?)\):\s*(\d{1,4})\b")
AMD = re.compile(r"\bamdgpu\s+([0-9a-fA-F]{4}:[0-9a-fA-F]{2}:[0-9a-fA-F]{2}\.[0-7]):.*?\b(GPU reset (?:begin|succeeded|failed)|ring [\w.]+ timeout)\b", re.I)
BOOT = re.compile(r"[0-9a-f]{32}")
EPOCH = datetime(1970, 1, 1, tzinfo=timezone.utc)
MAX_LINE = 262144
MAX_BYTES = 128 * 1024 * 1024

def utc_us(value):
    dt = datetime.fromisoformat(value)
    if dt.tzinfo is None:
        raise ValueError("timestamps require an explicit timezone")
    delta = dt.astimezone(timezone.utc) - EPOCH
    return (delta.days * 86400 + delta.seconds) * 1000000 + delta.microseconds

def correlate(path, start, end, boot_id):
    if not isinstance(boot_id, str) or not BOOT.fullmatch(boot_id):
        raise ValueError("provide the capture boot ID (32 lowercase hex digits)")
    lo, hi = utc_us(start), utc_us(end)
    if lo > hi:
        raise ValueError("launch start is after end")
    events, counts = [], Counter()
    digest = hashlib.sha256()
    total = 0
    with path.open("rb") as stream:
        while raw := stream.readline(MAX_LINE + 1):
            total += len(raw)
            digest.update(raw)
            if total > MAX_BYTES or len(raw) > MAX_LINE:
                raise ValueError("journal exceeds input size limit")
            try:
                row = json.loads(raw)
                if not isinstance(row, dict):
                    raise ValueError("not an object")
            except (ValueError, UnicodeError):
                counts["invalid_json"] += 1
                continue
            if row.get("_TRANSPORT") != "kernel":
                counts["not_kernel"] += 1
                continue
            if row.get("_BOOT_ID") != boot_id:
                counts["other_or_unknown_boot"] += 1
                continue
            stamp = row.get("__REALTIME_TIMESTAMP")
            message = row.get("MESSAGE")
            if not isinstance(stamp, str) or not re.fullmatch(r"[0-9]{1,20}", stamp) or not isinstance(message, str):
                counts["invalid_fields"] += 1
                continue
            t = int(stamp)
            if not lo <= t <= hi:
                counts["outside_window"] += 1
                continue
            counts["kernel_records_in_window"] += 1
            nvidia, amd = XID.search(message), AMD.search(message)
            if nvidia:
                events.append({"kind": "nvidia_xid", "code": int(nvidia[2]), "pci": nvidia[1].lower(), "offset_us": t-lo})
            elif amd:
                desc = amd[2].lower()
                kind = "amd_ring_timeout" if desc.startswith("ring ") else desc.replace(" ", "_")
                events.append({"kind": kind, "pci": amd[1].lower(), "offset_us": t-lo})
    events.sort(key=lambda e: e["offset_us"])
    return {"schema": 1, "journal_sha256": digest.hexdigest(),
            "window_us": hi-lo, "counts": dict(counts), "events": events,
            "scope": "Temporal overlap only; absence does not rule out GPU faults. PCI address does not prove process attribution."}

def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("journal", type=Path)
    p.add_argument("--start", required=True)
    p.add_argument("--end", required=True)
    p.add_argument("--boot-id", required=True)
    p.add_argument("--json", required=True, type=Path)
    a = p.parse_args(argv)
    try:
        result = correlate(a.journal, a.start, a.end, a.boot_id)
        with a.json.open("x", encoding="utf-8") as out:
            json.dump(result, out, indent=2)
            out.write("\n")
    except (OSError, ValueError) as exc:
        p.error(str(exc))
    print(f"{len(result['events'])} matching kernel events; temporal overlap only")
    return 0

if __name__ == "__main__":
    raise SystemExit(main())
