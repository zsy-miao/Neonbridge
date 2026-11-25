#!/usr/bin/env python3
"""Extend a chase preparation profile with programs and variants from a retail trace."""
import argparse
import json
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from project_tuxbox.inspect import digest
from project_tuxbox.runtime_shaders import LINK_KEY, SOURCE_KEY, STENCIL_KEY
from project_tuxbox.shader_inventory import ARCHIVES, archive_entries, decoder


def observations(log):
    standalone, links, stencils, missing = set(), set(), set(), set()
    with log.open(errors="replace") as stream:
        for line in stream:
            if match := re.search(rf"shader request [a-z ]+ ({SOURCE_KEY})\b", line):
                standalone.add(match[1])
            elif match := re.search(rf"shader missing or invalid, source SHA-256 ({SOURCE_KEY})", line):
                missing.add(match[1])
            elif match := re.match(r"\[TuxBox [^]]+\] Compiled Xbox shader (\S+)\s*$", line):
                key = match[1]
                if re.fullmatch(SOURCE_KEY, key):
                    standalone.add(key)
                elif re.fullmatch(LINK_KEY, key):
                    producer, pixel, _ = key.split("_")
                    links.add((producer, pixel))
                elif re.fullmatch(STENCIL_KEY, key):
                    source, slot = key.split("_rt")
                    stencils.add((source, int(slot)))
    return standalone - missing, links, stencils


def update(mount, profile, log):
    if (digest(mount / "Frosty/Binaries/XSX/Frosty-XSX-Shipping.exe") != profile["title_sha256"]
            or profile["archives"] != {name: meta[2] for name, meta in ARCHIVES.items()}):
        raise ValueError("Preparation profile does not match the package")
    shaders, links, stencils = observations(log)
    wanted = shaders | {s for pair in links for s in pair} | {s for s, _ in stencils}
    if not wanted:
        raise ValueError("No shader observations in trace")
    prior = {name: set(indices) for name, indices in profile["programs"].items()}
    refs = {}
    decode = decoder()
    with (mount / "Frosty/Content/Paks/Frosty-XSX.pak").open("rb") as pak:
        for name in ARCHIVES:
            for row in archive_entries(name, pak, decode, None, {}, include_ray_sources=True):
                if row.source in wanted and (row.source not in refs or row.index in prior[name]):
                    refs[row.source] = row
    if absent := wanted - refs.keys():
        raise ValueError(f"{len(absent)} shader inputs absent from package, first {min(absent)}")
    for name, indices in prior.items():
        indices.update(row.index for row in refs.values() if row.archive == name)
        profile["programs"][name] = sorted(indices)

    def ref(source):
        row = refs[source]
        return f"{row.archive}/{row.index}"

    profile["links"] = sorted({tuple(pair) for pair in profile["links"]}
                              | {(ref(a), ref(b)) for a, b in links})
    profile["stencil_targets"] = sorted({tuple(pair) for pair in profile.get("stencil_targets", ())}
                                       | {(ref(s), slot) for s, slot in stencils})
    print(f"Matched {len(refs)} shader inputs, {len(links)} linked pairs and {len(stencils)} stencil variants.")


def format_profile(profile):
    # Keep large numeric lists readable and each linked pair on a single line.
    text = json.dumps(profile, indent=2)
    def compact_numbers(match):
        values = re.findall(r"\d+", match[0])
        return ",\n".join("      " + ", ".join(values[i:i + 15]) for i in range(0, len(values), 15))
    text = re.sub(r"^      \d+(?:,\n      \d+)+$", compact_numbers, text, flags=re.M)
    return re.sub(r'\[\n      ("[^"]+"),\n      ("[^"]+"|\d+)\n    \]', r'[\1, \2]', text) + "\n"


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mount", type=Path)
    parser.add_argument("log", type=Path, help="launcher's tuxbox.log with shader tracing enabled")
    parser.add_argument("--profile", type=Path, default=ROOT / "tools/reference/frosty-chase-warmup.json")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    profile = json.loads(args.profile.read_text())
    update(args.mount, profile, args.log)
    args.output.write_text(format_profile(profile))
