#!/usr/bin/env python3
"""Map VulkanProfiler pipeline shader IDs to Frosty archive entries using vkd3d shader dumps.

Requires pyfarmhash. Supply the runtime cache directory for the measured compiler.
The optional family map comes from map_frosty_tsr.py --all-families.
"""
import argparse
from collections import defaultdict
import json
from pathlib import Path
import re
import struct
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from project_tuxbox.inspect import digest
from project_tuxbox.shader_inventory import ARCHIVES, archive_entries, decoder


def entry_points(module):
    if len(module) < 20 or len(module) % 4 or module[:4] != b"\x03\x02\x23\x07":
        raise ValueError("Invalid SPIR-V module header")
    pos = 20
    while pos < len(module):
        operation, words = struct.unpack_from("<HH", module, pos)
        end = pos + words * 4
        if not words or end > len(module):
            raise ValueError("Invalid SPIR-V instruction length")
        if operation == 15:  # OpEntryPoint: execution model, ID, literal string, interface
            if words < 4:
                raise ValueError("Truncated SPIR-V entry point")
            name = module[pos + 12:end]
            terminator = name.find(b"\0")
            if terminator <= 0:
                raise ValueError("Invalid SPIR-V entry point name")
            yield name[:terminator]
        pos = end


def build_map(pak, dumps, runtime, families, fingerprint):
    if not runtime.is_dir() or runtime.parent.name != "runtime":
        raise ValueError("Expected the measured cache's runtime/<compiler fingerprint> directory")
    sources = defaultdict(set)
    source_root = runtime.parent.parent
    for directory in (runtime, *(source_root / name for name in ("by_source", "by_vs_ps", "by_gs_ps"))):
        for path in directory.glob("*.cso"):
            # Linked programs name the native producer first.
            source = re.match(r"^([0-9a-f]{64})(?:_|$)", path.stem)
            if source:
                sources[digest(path)].add(source[1])
    archives = defaultdict(set)
    with pak.open("rb") as stream:
        for archive in ARCHIVES:
            for entry in archive_entries(archive, stream, decoder(), None, {}, include_ray_sources=True):
                archives[entry.source].add((archive, entry.index))
    names = defaultdict(set)
    if families:
        for row in json.loads(families.read_text())["maps"]:
            for index in row["entries"]:
                names[index].update(row["names"])
    result = defaultdict(list)
    for path in sorted(dumps.glob("*.spv")):
        # Libraries can have multiple entry-point modules for one .lib.dxil dump.
        stem = path.name.split(".", 1)[0]
        paired = dumps / (stem + (".lib.dxil" if ".lib." in path.name else ".dxil"))
        if not paired.is_file():
            continue
        dxil = digest(paired)
        refs = sorted({ref for source in sources[dxil] for ref in archives[source]})
        module = path.read_bytes()
        module_id = fingerprint(module)
        for name in entry_points(module):
            key = f"{module_id ^ fingerprint(name):08X}"
            result[key].append({"spv": path.name, "entry_point": name.decode(), "dxil_sha256": dxil,
                                "entries": refs, "maps": sorted({label for archive, index in refs
                                                                if archive == "global" for label in names[index]})})
    if not result:
        raise ValueError("No paired SPIR-V and DXIL dumps found")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ("pak", "dumps", "runtime", "output"):
        parser.add_argument(arg, type=Path)
    parser.add_argument("--families", type=Path)
    args = parser.parse_args()
    try:
        from farmhash import fingerprint32
        mapping = build_map(args.pak, args.dumps, args.runtime, args.families, fingerprint32)
    except (ImportError, OSError, ValueError) as error:
        parser.exit(1, f"Shader identification failed: {error}\n")
    args.output.write_text(json.dumps(mapping, sort_keys=True, indent=2) + "\n")
    print(f"Matched {sum(any(row['entries'] for row in matches) for matches in mapping.values())}"
          f" of {len(mapping)} profiler shader IDs to archive entries")


if __name__ == "__main__":
    main()
