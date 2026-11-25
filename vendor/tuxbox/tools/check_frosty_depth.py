#!/usr/bin/env python3
"""Check a single-sample D32_FLOAT_S8X24_UINT to R32_FLOAT UAV round trip through GPU staging."""
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
    parser.add_argument("--provider", type=Path, help="also verify the Xbox-facing resource, UAV and barrier calls")
    parser.add_argument("--placed", action="store_true", help="also check the chase depth texture at its retail heap offset")
    parser.add_argument("--tuxbox", action="store_true", help="use the TuxBox guest interfaces for compute and readback")
    parser.add_argument("--proton", type=Path)
    parser.add_argument("--prefix", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    if (args.placed or args.tuxbox) and not args.provider:
        parser.error("--placed and --tuxbox require --provider")
    host = Host(None, args.proton, args.prefix)
    source = ROOT / "tools/reference/depth"
    build = ROOT / "build/frosty-depth"
    build.mkdir(exist_ok=True, parents=True)
    with TemporaryDirectory(dir=build) as temp:
        work = Path(temp)
        subprocess.run([find_dxc(None), "-T", "cs_6_0", "-E", "main", "-O3", "-Fh", "modify_dxil.h",
                        "-Vn", "modify_dxil", source / "modify.hlsl"], cwd=work, check=True)
        tools = Toolchain()
        obj = tools.compile(source / "guest.cpp", work, "/W4", "/WX", f"/I{work}", f"/I{ROOT / 'runtime/d3d12'}")
        exe = work / "depth.exe"
        tools.link(exe, obj, "d3d12.lib", "dxgi.lib", "kernel32.lib")
        run = host.run(exe, [], timeout=60, capture=True)
        result = dict(guest_sha256=digest(exe), host=host.version, samples=1,
                      method="whole-plane CopyTextureRegion via default-heap buffer and compute RWTexture2D<float>",
                      cases={}, exit=run.returncode)
        success = not run.returncode
        cases = [("typed", 73 * 19), ("typeless", 1024 * 1024)]
        if args.provider:
            shutil.copyfile(args.provider, work / "d3d12_xs.dll")
            extra = ["--tuxbox"] if args.tuxbox else []
            wrapped = host.run(exe, ["wrapped", *extra], timeout=60, capture=True)
            result.update(provider_sha256=digest(args.provider), wrapped_exit=wrapped.returncode)
            success &= not wrapped.returncode
            if wrapped.returncode:
                sys.stderr.buffer.write(wrapped.stdout)
            cases.append(("wrapped", 1024 * 1024))
            if args.placed:
                placed = host.run(exe, ["placed", *extra], timeout=90, capture=True)
                result["placed_exit"] = placed.returncode
                success &= not placed.returncode
                cases.append(("placed", 2976 * 1676))
        if args.tuxbox:
            result["guest_compute_slot"] = 62
            result["guest_copy_slot"] = 83
        for case, pixels in cases:
            receipt = work / f"{case}.txt"
            observed = dict(line.split("=", 1) for line in receipt.read_text().splitlines()) if receipt.exists() else {}
            result["cases"][case] = observed
            success &= all(observed.get(key) == str(pixels) for key in ("matched_depth", "matched_stencil"))
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(result, indent=2) + "\n")
        if not success:
            sys.stderr.buffer.write(run.stdout)
            print(result["cases"], file=sys.stderr)
        return int(not success)


if __name__ == "__main__":
    sys.exit(main())
