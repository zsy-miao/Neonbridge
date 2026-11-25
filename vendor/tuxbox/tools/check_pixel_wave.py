#!/usr/bin/env python3
"""Render a triangle and inspect pixel wave allocation, helpers and overflow.

This is a host D3D12 capability control. It does not execute an Xbox shader or
prove that host wave32 allocation reproduces Xbox wave64 ordering.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import find_dxc
from frosty_gpu_guest import run_gpu_guest


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    for name in ("provider", "proton", "prefix", "compiler-dir", "work", "output"):
        ap.add_argument("--" + name, type=Path, required=True)
    ap.add_argument("--adapter", required=True)
    ap.add_argument("--icd", type=Path)
    a = ap.parse_args()
    if a.icd:
        os.environ["VK_DRIVER_FILES"] = str(a.icd.resolve())
        os.environ["VKD3D_FILTER_DEVICE_NAME"] = "AMD"
        os.environ["DXVK_FILTER_DEVICE_NAME"] = "AMD"
    source = ROOT / "tools/reference/pixel-wave"
    a.work.mkdir(parents=True, exist_ok=True)
    with TemporaryDirectory(dir=a.work.resolve()) as td:
        trial = Path(td)
        for name, profile in (("vertex", "vs_6_0"), ("pixel", "ps_6_6")):
            subprocess.run([find_dxc(None), "-T", profile, "-E", "main", "-Fo", trial / (name + ".cso"),
                            source / (name + ".hlsl")], check=True)
            subprocess.run([Path(find_dxc(None)).with_name("dxv"), trial / (name + ".cso")], check=True)
        exe, status, report = run_gpu_guest(source / "guest.cpp", "pixel-wave.exe", trial, a.work.resolve(),
                                            a.proton.resolve(), a.prefix, a.provider, a.compiler_dir)
        if status or report.get("adapter", "").lower() != a.adapter.lower():
            raise ValueError(f"Unexpected guest status or adapter: {status}, {report}")
        pixels = list(struct.iter_unpack("<4I", (trial / "out0.bin").read_bytes()))
        (counter,) = struct.unpack("<I", (trial / "out1.bin").read_bytes())
        records = list(struct.unpack("<176I", (trial / "out2.bin").read_bytes()))
        covered = {i: p for i, p in enumerate(pixels) if p != (0xdeadbeef,) * 4}
        active = {i for i in covered if (i % 32 + 3 * (i // 32)) % 5 != 0}
        if len(covered) not in (496, 528) or counter != len(active) or counter <= 160:
            raise ValueError(f"Unexpected raster coverage or count: {len(covered)}, {counter}, {len(active)}")
        slots = {i: p[3] for i, p in covered.items() if i in active and p[3] < 160}
        if len(slots) != 160 or set(slots.values()) != set(range(160)):
            raise ValueError("Wave reservations did not generate 160 unique in-capacity slots")
        if any(records[slot] != pixel for pixel, slot in slots.items()) or any(x != 0xdeadbeef for x in records[160:]):
            raise ValueError("Pixel list content or guard region differs")
        if any(i not in active and p[3] != 0xffffffff for i, p in covered.items()):
            raise ValueError("Inactive pixels unexpectedly reserved a slot")
        wave_sizes = sorted({p[0] for p in covered.values()})
        if any(p[0] not in (4, 8, 16, 32, 64, 128) or p[1] >= p[0] or (p[2] & 1) for p in covered.values()):
            raise ValueError("Invalid lane width/index or helper-lane side effect")
        result = {"scope": "Host pixel-wave characterization using a synthetic triangle, not original Xbox shader execution. Overflow is checked for valid unique slots, not equality with the Xbox wave64 subset.",
                  "adapter": report["adapter"], "wave_sizes": wave_sizes, "covered_pixels": len(covered),
                  "active_pixels": len(active), "reserved": counter, "capacity": 160,
                  "helper_neighbor_pixels": sum(bool(p[2] & 14) for p in covered.values()),
                  "slots_checked": 160, "guard_words": 16,
                  "guest_sha256": hashlib.sha256(exe.read_bytes()).hexdigest(),
                  "pixel_source_sha256": hashlib.sha256((source / "pixel.hlsl").read_bytes()).hexdigest()}
    a.output.parent.mkdir(parents=True, exist_ok=True)
    a.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
