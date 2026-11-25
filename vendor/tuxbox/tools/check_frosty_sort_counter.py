#!/usr/bin/env python3
"""Check the elected counter address and VCC population in Frosty's translated wave64 tail."""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import find_dxc
from frosty_gpu_guest import run_gpu_guest
from project_tuxbox.inspect import digest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("hlsl", "prefix", "provider", "compiler-dir", "proton", "work", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--control-container", type=Path, help="Frosty global 2227 four-table control",
                        default=ROOT/"build/frosty-signature/chase-swizzle/2227.dxbc")
    args = parser.parse_args()
    work = args.work.resolve()
    work.mkdir(parents=True, exist_ok=True)
    text = args.hlsl.read_text()
    declaration = "groupshared uint sort_first, sort_count;"
    tail = text[text.index("    if (gtid.x == 0u) {\n        sort_first = 64u;"):]
    # Each possible issuer has a different address. Only the elected lane is allowed
    # to update its counter. All 768 initial words are explicitly initialized.
    hlsl = declaration + """
RWBuffer<uint> sort_counter : register(u0);
[numthreads(64,1,1)]
void main(uint3 gid:SV_GroupID, uint3 gtid:SV_GroupThreadID) {
    uint lane = gtid.x, group = gid.x, index = (64u * group + lane) * 3u;
    uint exec_lo = group == 0u ? 0u : group == 1u ? lane >= 34u : group == 2u ? (lane % 5u == 0u) : 1u;
    uint v2 = group == 0u ? 0u : group == 1u ? lane % 3u : group == 2u ? (lane != 35u) : (lane != 34u);
    uint4 cb0[4]; cb0[3].y = index + 1u;
    sort_counter[index] = 0x5a5a3210u;
    sort_counter[index+1] = 0xfffffffeu;
    sort_counter[index+2] = 0x1234abcd;
    AllMemoryBarrierWithGroupSync();
""" + tail
    # Active and contributing lane sets are independent reference inputs.
    cases = [(set(), set(range(64))), (set(range(34, 64)), set(range(0, 64, 3))),
             (set(range(0, 64, 5)), {35}), (set(range(64)), {34})]
    expected = bytearray()
    for active, qualifying in cases:
        first = min(active, default=None)
        for lane in range(64):
            value = 0xfffffffe + (len(active & qualifying) if lane == first else 0)
            expected.extend(struct.pack("<III", 0x5a5a3210, value & 0xffffffff, 0x1234abcd))
    with tempfile.TemporaryDirectory(dir=work) as path:
        trial = Path(path)
        (trial / "exchange.hlsl").write_text(hlsl)
        subprocess.run([find_dxc(None), "-T", "cs_6_0", "-E", "main", "-Fo", trial/"exchange.cso",
                        trial/"exchange.hlsl"], check=True)
        (trial / "expected.bin").write_bytes(expected)
        # Existing Xbox compute control obtains the four-table root from global 2227.
        # Use the compiled host shader to test the terminal block independently of its prefix.
        original = args.control_container
        if digest(original) != "d6ce4f1984b92b4106c31b052c92e1e5dccd60943a06da7a250fcb132c28d12b":
            raise ValueError("Unexpected four-table control shader")
        (trial / "retail.dxbc").write_bytes(original.read_bytes())
        exe, status, result = run_gpu_guest(ROOT/"tools/reference/compute-swizzle/guest.cpp", "count.exe",
                                            trial, work, args.proton.resolve(), args.prefix,
                                            args.provider, args.compiler_dir)
        if status or result.get("matched") != "768":
            raise ValueError(f"Wave64 counter readback failed: {status}, {result}")
        receipt = dict(provider_sha256=digest(args.provider), translated_hlsl_sha256=digest(args.hlsl),
                       terminal_hlsl_sha256=hashlib.sha256(tail.encode()).hexdigest(), guest_sha256=digest(exe),
                       matching_uints=768, groups=4, elected_lanes=[None, 34, 0, 0],
                       increments=[0, 10, 1, 1],
                       adapters=[line.split("Found device: ")[1] for line in (work/"gpu.log").read_text().splitlines()
                                 if "Found device: " in line])
    args.output.write_text(json.dumps(receipt, indent=2)+"\n")


if __name__ == "__main__":
    main()
