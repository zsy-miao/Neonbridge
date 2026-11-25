#!/usr/bin/env python3
"""Run Frosty's two merged vertex programs and read back their native position and primitive exports."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import Toolchain, find_dxc
from check_frosty_chase_link import extract, SHADERS
from project_tuxbox.host import Host
from project_tuxbox.inspect import digest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for flag in ("mount", "xshader", "proton", "prefix", "output"):
        parser.add_argument("--" + flag, type=Path, required=True)
    args = parser.parse_args()
    host = Host("proton", args.proton, args.prefix)
    host.env["UMU_RUNTIME_UPDATE"] = "0"
    parent = ROOT / "build/frosty-signature/chase-vertex"
    parent.mkdir(parents=True, exist_ok=True)
    patch = ROOT / "tools/reference/tombox-merged-vertex-count.patch"
    with TemporaryDirectory(dir=parent) as folder:
        trial = Path(folder)
        extract(args.mount, trial)
        stages = {}
        for n in (6113, 31788):
            native, hlsl, dxil = (trial / f"mat{n}.{ext}" for ext in ("dxbc", "hlsl", "cso"))
            subprocess.run([args.xshader.resolve(), native, "--hlsl-out", hlsl], check=True, stdout=subprocess.DEVNULL)
            subprocess.run([find_dxc(None), "-T", "vs_6_6", "-E", "main", hlsl, "-Fo", dxil], check=True)
            stages[n] = dict(native_sha256=SHADERS[n], hlsl_sha256=digest(hlsl), dxil_sha256=digest(dxil))
        tc = Toolchain()
        obj = tc.compile(ROOT / "tools/reference/merged-vertex/guest.cpp", trial, "/W4", "/WX",
                         "/D_CRT_SECURE_NO_WARNINGS", f"/I{ROOT / 'runtime/d3d12'}")
        guest = trial / "vertex.exe"
        tc.link(guest, obj, "d3d12.lib", "dxgi.lib", "kernel32.lib")
        observed = host.run(guest, [], timeout=75, capture=True)
        readback = trial / "vertex.txt"
        lines = readback.read_text().splitlines() if readback.exists() else []
        if observed.returncode or len(lines) != 9 or not lines[0].startswith("adapter="):
            raise RuntimeError(f"Vertex probe returned {observed.returncode}: {lines}")
        result = dict(host=host.version, guest_sha256=digest(guest), translator_sha256=digest(args.xshader),
                      patch_sha256=digest(patch), vertex_entry=stages, adapter=lines[0].removeprefix("adapter="), readback=lines[1:],
                      resource_setup="Zero initial inputs except ATTRIBUTE13=3, t2[3,4]=2,3, t1[2,3].x=17,29, and CB2[20] clip-space translation. Two instances per draw. The guest verifies both float4 exports, primitive ID, byte count and trailing guard.",
                      scope="Actual translated retail vertex shaders dispatched through a desktop stream-output PSO with controlled resources.")
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
