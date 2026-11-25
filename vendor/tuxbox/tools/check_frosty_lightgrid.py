#!/usr/bin/env python3
"""Check both wave allocations in the original Frosty LightGridInjectionCS with sparse cell coordinates and typed UAVs."""
import argparse
import ctypes
from ctypes.util import find_library
import json
from pathlib import Path
import subprocess
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import Toolchain, find_dxc
from check_frosty_clear import fnv1a
from check_frosty_shaders import GLOBAL_SHA, containers, decompress, global_archive, require
from project_tuxbox.host import Host
from project_tuxbox.shader_prep import shader_compiler
from project_tuxbox.inspect import digest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mount", type=Path)
    parser.add_argument("--prefix", type=Path)
    parser.add_argument("--proton", type=Path)
    parser.add_argument("--xshader", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    xshader = args.xshader.resolve() if args.xshader else shader_compiler()
    host = Host(None, args.proton, args.prefix)
    build = ROOT / "build/frosty-ff1"
    build.mkdir(exist_ok=True, parents=True)
    data, entries = global_archive(args.mount)
    lz4 = ctypes.CDLL(find_library("lz4")).LZ4_decompress_safe
    lz4.argtypes = [ctypes.c_char_p, ctypes.c_void_p, ctypes.c_int, ctypes.c_int]
    ((_, cooked, _),) = containers(decompress(data, entries[3260], lz4))
    require(fnv1a(cooked) == 0xd8d1d5c254cb81c3, "Unexpected cooked shader")
    with TemporaryDirectory(dir=build) as temp:
        work = Path(temp)
        native, source, desktop = (work / name for name in ("native.dxbc", "converted.hlsl", "converted.dxil"))
        native.write_bytes(cooked)
        report = subprocess.check_output([xshader, native, "--hlsl-out", source, "--profile-out", work / "profile"], text=True)
        require('entry=Some("LightGridInjectionCS")' in report, "Unexpected shader entry")
        dxc = find_dxc(None)
        subprocess.run([dxc, "-E", "main", "-T", (work / "profile").read_text().strip(), "-O3", "-Fo", desktop, source], check=True)
        dump = subprocess.check_output([dxc, "-dumpbin", desktop], text=True)
        require("NumThreads=(4,4,4)" in dump and "atomicBinOp" in dump and "dx.op.barrier" in dump and "WaveSize=" not in dump,
                "Expected a 4x4x4 group with barriers, typed UAV atomics and a portable subgroup width")
        header = work / "lightgrid_blobs.h"
        header.write_text("".join(f"const unsigned char {name}[] = {{" + ",".join(map(str, value)) + "};\n"
                                  for name, value in (("cooked_shader", cooked), ("desktop_shader", desktop.read_bytes()))))
        tools = Toolchain()
        obj = tools.compile(ROOT / "tools/reference/lightgrid/guest.cpp", work, "/W4", "/WX",
                            f"/I{work}", f"/I{ROOT / 'runtime/d3d12'}")
        exe = work / "lightgrid.exe"
        tools.link(exe, obj, "d3d12.lib", "dxgi.lib", "kernel32.lib")
        run = host.run(exe, [], timeout=90, capture=True)
        receipt = work / "readback.txt"
        observed = dict(line.split("=", 1) for line in receipt.read_text().splitlines()) if receipt.exists() else {}
        result = dict(archive_sha256=GLOBAL_SHA, entry=3260, shader="LightGridInjectionCS", native_sha256=digest(native),
                      hlsl_sha256=digest(source), dxil_sha256=digest(desktop), dxil_bytes=desktop.stat().st_size,
                      guest_sha256=digest(exe), host=host.version, groups=[2, 2, 2],
                      scope="Eight 4x4x4 groups. One or two groups cover 24 or 48 active cells. Broad and lane-34 lights, repeated reflection captures, linked chains, capacity overflow, each empty list and both empty.",
                      readback=observed, exit=run.returncode)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(result, indent=2) + "\n")
        if run.returncode or not observed:
            sys.stderr.buffer.write(run.stdout)
        cases = ("two_productive_groups", "broad", "small", "chained", "limited", "captures", "lights", "empty")
        require(not run.returncode and all(observed.get(f"{case}_matched_words") == "1024" for case in cases),
                str(observed))


if __name__ == "__main__":
    main()
