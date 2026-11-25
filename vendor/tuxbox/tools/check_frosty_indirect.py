#!/usr/bin/env python3
"""Compare Frosty's command signatures with desktop D3D12 on the same GPU.

python tools/check_frosty_indirect.py --proton PATH --prefix PATH --provider PATH/to/d3d12_xs.dll --output receipt.json

The x64 guest executes a compute signature and compares all 81 readback words.
It also checks mesh and ray signature creation. The original TomBox provider
is expected to exit with status 1 and write a receipt for the rejected calls.
"""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import Toolchain, find_dxc
from project_tuxbox.host import Host
from project_tuxbox.inspect import digest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--provider", type=Path, help="also test this Xbox-facing d3d12_xs.dll")
    parser.add_argument("--proton", type=Path)
    parser.add_argument("--prefix", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    host = Host(None, args.proton, args.prefix)
    src = ROOT / "tools/reference/indirect"
    build = ROOT / "build/frosty-indirect"
    build.mkdir(exist_ok=True, parents=True)
    with TemporaryDirectory(dir=build) as temp:
        work = Path(temp)
        subprocess.run([find_dxc(None), "-T", "cs_6_0", "-E", "main", "-O3", "-Fh", "groups_dxil.h",
                        "-Vn", "groups_dxil", str(src / "groups.hlsl")], cwd=work, check=True)
        tools = Toolchain()
        obj = tools.compile(src / "guest.cpp", work, "/W4", "/WX", f"/I{work}", f"/I{ROOT / 'runtime/d3d12'}")
        exe = work / "indirect.exe"
        tools.link(exe, obj, "d3d12.lib", "dxgi.lib", "kernel32.lib")
        result = {"guest_sha256": digest(exe), "host": host.version, "runs": {}}
        modes = ["desktop"]
        if args.provider:
            shutil.copyfile(args.provider, work / "d3d12_xs.dll")
            result["d3d12_xs_sha256"] = digest(work / "d3d12_xs.dll")
            modes.append("xbox")
        for mode in modes:
            receipt = work / (mode + ".txt")
            run = host.run(exe, [mode, receipt.name], timeout=60, capture=True)
            observation = {}
            if receipt.exists():
                observation = dict(line.split("=", 1) for line in receipt.read_text().splitlines())
            else:
                observation["error"] = "Guest did not write a receipt"
                sys.stderr.buffer.write(run.stdout)
            result["runs"][mode] = {"exit": run.returncode, **observation}
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    return int(any(run["exit"] or run.get("verified_words") != "81" for run in result["runs"].values()))


if __name__ == "__main__":
    sys.exit(main())
