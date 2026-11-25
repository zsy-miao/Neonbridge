#!/usr/bin/env python3
"""Record installed handlers in the TuxBox Xbox D3D12 proxy tables."""
import argparse
import json
from pathlib import Path
import re
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
INVENTORY = ROOT / "evidence/matrix-awakens/d3d12-xs-interfaces.json"
TABLES = {
    "Device": "device::DEVICE",
    "Graphics command list": "commands::LIST_SLOTS",
    "DMA command list": "commands::media::SLOTS",
    "Command queue": "commands::QUEUE_SLOTS",
    "Descriptor heap": "device::HEAP",
    "Command allocator": "device::ALLOCATOR",
    "Resource": "device::RESOURCE",
    "NV12 plane alias": "device::media::ALIAS",
    "Fence": "device::FENCE",
    "Heap": "device::heap::HEAP",
    "Root signature": "device::compute::ROOT",
    "Pipeline state": "device::compute::PIPELINE",
    "Query heap": "device::queries::HEAP",
    "Command signature": "device::indirect::SIGNATURE",
    "State object": "device::state_object::TABLE",
    "State object properties": "device::state_object::inspection::PROPERTIES",
    "Ray shader inspection": "device::state_object::inspection::INSPECTOR",
}


def registered(ir):
    tables = {}
    missing_handler_seen = False
    # The module suffix in Rust's v0 mangling is independent of the crate hash.
    names = {"12tuxbox_d3d12" + "".join(f"{len(part)}{part}" for part in path.split("::")): label
             for label, path in TABLES.items()}
    for line in ir:
        if not line.startswith("@_R"):
            continue
        symbol, _, definition = line.partition(" = ")
        suffix = symbol[symbol.find("12tuxbox_d3d12"):]
        if suffix not in names:
            continue
        label = names[suffix]
        table = re.search(r"constant <\{ (ptr(?:, ptr)*) \}> <\{ (.*?) \}>", definition)
        if not table or label in tables:
            raise ValueError(f"Unrecognized LLVM table definition for {label}")
        slots = table[2].split(", ")
        if len(slots) != len(table[1].split(", ")) or any(not re.fullmatch(r"ptr @_R\w+", s) for s in slots):
            raise ValueError(f"Expected a function pointer at each {label} slot")
        mapped = [i for i, handler in enumerate(slots) if "12tuxbox_d3d125proxy16unsupported_slot" not in handler]
        missing_handler_seen |= len(mapped) < len(slots)
        tables[label] = mapped
    if not missing_handler_seen:
        raise ValueError("Could not locate the Xbox unsupported-slot handlers")
    if tables.keys() != TABLES.keys():
        raise ValueError(f"Missing compiled tables: {', '.join(TABLES.keys() - tables.keys())}")
    return {name: tables[name] for name in TABLES}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=INVENTORY)
    args = parser.parse_args()
    data = json.loads(INVENTORY.read_text())
    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory) / "provider.ll"
        subprocess.run(["cargo", "rustc", "--locked", "--release", "-p", "tuxbox-d3d12",
                        "--target", "x86_64-pc-windows-msvc", "--",
                        "-Csymbol-mangling-version=v0", f"--emit=llvm-ir={path}"], cwd=ROOT, check=True)
        with path.open() as ir:
            data["registered_slots"] = registered(ir)
    args.output.write_text(json.dumps(data, indent=2) + "\n")


if __name__ == "__main__":
    main()
