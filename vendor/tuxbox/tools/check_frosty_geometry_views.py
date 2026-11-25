#!/usr/bin/env python3
"""Read the three packed view records selected by Frosty's retail Nanite vertex shader."""
import argparse
import json
from pathlib import Path
import struct
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import Toolchain, find_dxc
from check_frosty_shaders import containers, decompress, global_archive
from project_tuxbox.host import Host
from project_tuxbox.inspect import digest
from project_tuxbox.shader_inventory import decoder
from project_tuxbox.shader_prep import run_tool, shader_compiler

SOURCE = "e97681a916368067bb6a90022ef3d192b79cd20cf285f23b4b039eb6eea5f5a8"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mount", type=Path)
    parser.add_argument("--adapter", required=True, help="expected PCI vendor:device")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--compiler", type=Path)
    parser.add_argument("--proton", type=Path)
    parser.add_argument("--prefix", type=Path)
    args = parser.parse_args()
    payload, entries = global_archive(args.mount)
    ((_, source, _),) = containers(decompress(payload, entries[2927], decoder()))
    host = Host("proton", args.proton, args.prefix)
    host.env["UMU_RUNTIME_UPDATE"] = "0"
    compiler = args.compiler.resolve() if args.compiler else shader_compiler()
    dxc = Path(find_dxc(None))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with TemporaryDirectory(dir=ROOT / "build", prefix="geometry-views-") as folder:
        work = Path(folder)
        cooked, hlsl, dxil = (work / ("2927." + ext) for ext in ("dxbc", "hlsl", "cso"))
        cooked.write_bytes(source)
        if digest(cooked) != SOURCE:
            raise ValueError("Unknown global/2927 program")
        run_tool([compiler, cooked, "--hlsl-out", hlsl])
        run_tool([dxc, "-T", "vs_6_0", "-E", "main", "-Fo", dxil, hlsl])
        run_tool([dxc.with_name("dxv"), dxil])
        toolchain = Toolchain()
        obj = toolchain.compile(ROOT / "tools/reference/independent-shaders/geometry-views.cpp", work,
                                "/W4", "/WX", f"/I{ROOT / 'runtime/d3d12'}")
        exe = work / "geometry-views.exe"
        toolchain.link(exe, obj, "d3d12.lib", "dxgi.lib", "kernel32.lib")
        run = host.run(exe, [], capture=True, timeout=60)
        if run.returncode:
            raise RuntimeError((work / "result.txt").read_text() if (work / "result.txt").exists()
                               else run.stdout.decode(errors="replace")[-1000:])
        adapter = (work / "result.txt").read_text().strip().removeprefix("adapter=")
        data = (work / "views.bin").read_bytes()
        values = [struct.unpack_from("<II", data, i * 16 + 8) for i in range(3)]
        dimensions_match = values == [(257 + i, 513 + i) for i in range(3)]
        tail = bytes(16) + struct.pack("<Q", 48) + bytes(8)
        result = dict(passed=dimensions_match and data[48:] == tail and adapter == args.adapter.lower(), adapter=adapter,
                      host=host.version, source_sha256=SOURCE, dxil_sha256=digest(dxil),
                      compiler_sha256=digest(compiler), dimensions=values,
                      bytes_written=struct.unpack_from("<Q", data, 64)[0])
        args.output.write_text(json.dumps(result, indent=2) + "\n")
        if not result["passed"]:
            raise SystemExit(f"Unexpected view readbacks on {adapter}: {result['dimensions']}")
        print(f"Three 800-byte Nanite view records matched on {adapter}.")


if __name__ == "__main__":
    main()
