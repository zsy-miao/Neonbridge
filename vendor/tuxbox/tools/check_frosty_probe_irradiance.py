#!/usr/bin/env python3
"""Check all output texels of Frosty's screen probe irradiance shader on a real GPU."""
import argparse
import json
import math
from pathlib import Path
import struct
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import Toolchain
from check_frosty_shaders import containers, decompress, global_archive
from project_tuxbox.host import Host
from project_tuxbox.inspect import digest
from project_tuxbox.shader_inventory import decoder
from project_tuxbox.shader_prep import compiler_tools, run_tool


def measure(path):
    data = path.read_bytes()
    if len(data) != 256 * 256 * 16:
        raise ValueError(f"Unexpected readback size {len(data)}")
    nonfinite = unexpected = 0
    peak = 0.0
    for red, green, blue, _ in struct.iter_unpack("<4f", data):
        if not all(map(math.isfinite, (red, green, blue))):
            nonfinite += 1
            continue
        peak = max(peak, red, green, blue)
        if not (0.1 < red < 1.5 and abs(green - 2 * red) < 0.00001 and abs(blue - 3 * red) < 0.00001):
            unexpected += 1
    return {"pixels": 65536, "nonfinite_rgb": nonfinite, "unexpected_rgb": unexpected, "max_channel": peak}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mount", type=Path)
    parser.add_argument("--adapter", required=True, help="expected PCI vendor:device")
    parser.add_argument("--proton", type=Path)
    parser.add_argument("--prefix", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    payload, entries = global_archive(args.mount)
    ((_, program, _),) = containers(decompress(payload, entries[481], decoder()))
    compiler, dxc, validator = compiler_tools()
    host = Host("proton", args.proton, args.prefix)
    host.env["UMU_RUNTIME_UPDATE"] = "0"
    with TemporaryDirectory(dir=ROOT / "build", prefix="probe-irradiance-") as folder:
        work = Path(folder)
        source, hlsl, dxil = (work / f"481.{ext}" for ext in ("dxbc", "hlsl", "cso"))
        source.write_bytes(program)
        if digest(source) != "132e1dc4de5d3d5682ed436ed7d19fda09c8930180c462f73630dc828ec30309":
            raise ValueError("Unexpected global/481 shader")
        run_tool([compiler, source, "--hlsl-out", hlsl])
        run_tool([dxc, "-T", "cs_6_0", "-E", "main", "-Fo", dxil, hlsl])
        run_tool([validator, dxil])
        tc = Toolchain()
        obj = tc.compile(ROOT / "tools/reference/independent-shaders/probe-irradiance.cpp", work,
                         f"/I{ROOT / 'runtime/d3d12'}")
        exe = work / "probe-irradiance.exe"
        tc.link(exe, obj, "d3d12.lib", "dxgi.lib", "kernel32.lib")
        run = host.run(exe, [], capture=True, timeout=60)
        if run.returncode:
            log = work / "adapter.txt"
            detail = log.read_text(errors="replace") if log.exists() else run.stdout.decode(errors="replace")
            raise RuntimeError(detail[-4000:])
        adapter = (work / "adapter.txt").read_text().strip().removeprefix("adapter=")
        results = {resolution: measure(work / f"{resolution}.bin") for resolution in (8, 6, 4)}
        passed = adapter.lower() == args.adapter.lower() and all(
            report["unexpected_rgb"] == report["nonfinite_rgb"] == 0 for report in results.values()
        )
        report = {"shader": "global/481", "source_sha256": digest(source), "dxil_sha256": digest(dxil),
                  "adapter": adapter, "groups": [32, 32, 1], "gather_resolutions": results, "passed": passed}
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2) + "\n")
        if not passed:
            raise SystemExit(f"Irradiance readbacks failed on {adapter}, see {args.output}")
        print(f"196,608 irradiance pixels passed on {adapter}.")


if __name__ == "__main__":
    main()
