#!/usr/bin/env python3
"""Check the published Xbox export inventory against a locally supplied package."""
import argparse
import json
from pathlib import Path
import sys

import pefile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from project_tuxbox.inspect import digest, export_table


def check(path, coverage):
    if digest(path) != coverage["binary_sha256"]:
        raise ValueError("reference SHA-256 differs")
    slots, data = export_table(path)
    if len(slots) != coverage["total"]:
        raise ValueError(f"expected {coverage['total']} occupied ordinals, found {len(slots)}")
    names = {name: ordinal for ordinal, aliases in slots.items()
             for name in (*aliases, f"#{ordinal}")}
    verified = [names[name] for name in coverage["verified"]]
    if len(verified) != len(set(verified)):
        raise ValueError("two verified entries identify the same ordinal")
    reported_data = [names[name] for name in coverage["data_exports"]]
    if set(reported_data) != data or len(reported_data) != len(data):
        raise ValueError("data exports differ")
    if any(names[name] in data and name not in coverage["data_exports"] for name in coverage["verified"]):
        raise ValueError("verified data must use the same identifier in both lists")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("package", type=Path, help="root containing EmbeddedXvd and Mount")
    args = parser.parse_args()
    status = json.loads((ROOT / "evidence/library-status.json").read_text())
    for library in status["libraries"]:
        row = library["export_coverage"]
        try:
            check(args.package / row["reference"], row)
        except (KeyError, OSError, ValueError, pefile.PEFormatError) as error:
            parser.exit(1, f"{library['name']}: {error}\n")
        print(f"{library['name']:<22} {len(row['verified']):>3} / {row['total']:<3} verified exports")


if __name__ == "__main__":
    main()
