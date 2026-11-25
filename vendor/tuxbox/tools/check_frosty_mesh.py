#!/usr/bin/env python3
"""Check the observed Xbox 0x2a8 mesh PSO layout against a desktop mesh-pipeline GPU control."""
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
    parser.add_argument("--provider", type=Path, required=True)
    parser.add_argument("--proton", type=Path)
    parser.add_argument("--prefix", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    host = Host(None, args.proton, args.prefix)
    src = ROOT / "tools/reference/mesh"
    build = ROOT / "build/frosty-signature/stream680"
    build.mkdir(parents=True, exist_ok=True)
    with TemporaryDirectory(dir=build) as tmp:
        work = Path(tmp)
        compiler = find_dxc(None)
        for name, target, entry in (("mesh", "ms", "mesh"), ("mesh_as", "ms", "mesh"),
                                    ("amplify", "as", "amplify"), ("pixel", "ps", "pixel")):
            defines = ["-DWITH_PAYLOAD=1"] if name == "mesh_as" else []
            subprocess.run([compiler, "-T", target + "_6_5", "-E", entry, "-O3", *defines,
                            "-Fh", name + ".h", "-Vn", name + "_dxil", str(src / "stages.hlsl")],
                           cwd=work, check=True)
        tools = Toolchain()
        obj = tools.compile(src / "guest.cpp", work, "/W4", "/WX", f"/I{work}", f"/I{ROOT / 'runtime/d3d12'}")
        exe = work / "mesh.exe"
        tools.link(exe, obj, "d3d12.lib", "dxgi.lib", "kernel32.lib")
        shutil.copyfile(args.provider, work / "d3d12_xs.dll")
        result = dict(host=host.version, d3d12_xs_sha256=digest(args.provider), guest_sha256=digest(exe),
                      scope="Desktop SM6.5 DXIL mesh and pixel shaders with optional amplification payload; 8x8 UAV readback",
                      runs={})
        for mode in ("desktop", "xbox"):
            receipt = work / (mode + ".txt")
            run = host.run(exe, [mode, receipt.name], timeout=60, capture=True)
            observed = dict(x.split("=", 1) for x in receipt.read_text().splitlines()) if receipt.exists() else {}
            result["runs"][mode] = dict(exit=run.returncode, **observed)
            if run.returncode or not observed:
                sys.stderr.buffer.write(run.stdout[-4096:])
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    return int(any(x["exit"] or x.get("ms_ps_matching_pixels") != "64" or x.get("as_ms_ps_matching_pixels") != "64"
                   for x in result["runs"].values()))


if __name__ == "__main__":
    sys.exit(main())
