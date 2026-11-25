#!/usr/bin/env python3
"""Exercise Frosty's API-based CBV and UAV descriptor copies on both vkd3d heap backends."""
import argparse
import json
import os
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
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--provider", required=True, type=Path)
    ap.add_argument("--proton", type=Path)
    ap.add_argument("--prefix", type=Path)
    ap.add_argument("--output", required=True, type=Path)
    args = ap.parse_args()
    build = ROOT / "build/frosty-descriptor-api"
    build.mkdir(exist_ok=True, parents=True)
    source = ROOT / "tools/reference/descriptor-api"
    host = Host(None, args.proton, args.prefix)
    toolchain = Toolchain()
    with TemporaryDirectory(dir=build) as directory:
        work = Path(directory)
        subprocess.run([find_dxc(None), "-T", "cs_6_0", "-E", "main", "-Fh", "work.h", "-Vn", "work_dxil",
                        source / "work.hlsl"], cwd=work, check=True)
        obj = toolchain.compile(source / "guest.cpp", work, "/W4", "/WX", f"/I{work}", f"/I{ROOT / 'runtime/d3d12'}")
        exe = work / "descriptor-api.exe"
        toolchain.link(exe, obj, "d3d12.lib", "dxgi.lib", "kernel32.lib")
        shutil.copyfile(args.provider, work / "d3d12_xs.dll")
        result = dict(provider_sha256=digest(args.provider), guest_sha256=digest(exe), host=host.version, runs={})
        for backend in ("default", "sets"):
            if backend == "sets":
                os.environ["VKD3D_DISABLE_EXTENSIONS"] = "VK_EXT_descriptor_buffer"
            receipt = work / "descriptor-api.txt"
            receipt.unlink(missing_ok=True)
            run = host.run(exe, [], timeout=60, capture=True)
            obs = dict(line.split("=", 1) for line in receipt.read_text().splitlines()) if receipt.exists() else {}
            result["runs"][backend] = dict(exit=run.returncode, **obs)
            if run.returncode:
                sys.stderr.buffer.write(run.stdout[-4096:])
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(result, indent=2) + "\n")
        return int(any(run["exit"] or run.get("matching_words") != "43" for run in result["runs"].values())
                   or result["runs"]["sets"].get("dst_low5") != "19")


if __name__ == "__main__":
    sys.exit(main())
