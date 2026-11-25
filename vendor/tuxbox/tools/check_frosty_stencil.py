#!/usr/bin/env python3
"""Compare both retail depth/stencil exports with independently predicted visibility and stencil values."""
import argparse
import json
import shutil
import struct
import subprocess
import sys
from pathlib import Path
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import Toolchain
from check_frosty_shaders import GLOBAL_SHA, containers, decompress, global_archive
from project_tuxbox.host import Host
from project_tuxbox.inspect import digest
from project_tuxbox.runtime_shaders import shader_session
from project_tuxbox.shader_inventory import decoder
from project_tuxbox.shader_prep import compiler_tools

SHADERS = {
    1748: "078b54eab6d1e676a891bc7239cfb0500898c648d1414d4a53b5731ee01b0e42",
    1763: "4dbeb7e58ba2610862b819e631b4128bde9cfeda006a2f93b44bd681ee073e7e",
}


def expected_pixels():
    pixels = bytearray()
    for y in range(8):
        for x in range(16):
            # Empty visibility discards. The nearest two rows fail reversed-Z.
            survives = x >= 4 and y >= 2
            depth = 0.75 if survives else 0.25
            stencil = (0x29 if x < 10 else 0x4a) if survives else 0x6c
            pixels.extend(struct.pack("<fB", depth, stencil))
    return pixels


def run(mount, host, adapter, provider, work):
    archive, entries = global_archive(mount)
    ref = ROOT / "tools/reference/stencil-export"
    build = Toolchain()
    compiler, dxc, _ = compiler_tools()
    obj = build.compile(ref / "guest.cpp", work, "/W4", "/WX", f"/I{ROOT / 'runtime/d3d12'}")
    exe = work / "stencil.exe"
    build.link(exe, obj, str(ROOT / "build/d3d12-smoke/d3d12_xs.lib"), "d3d12.lib", "dxgi.lib", "kernel32.lib")
    subprocess.run([dxc, "-T", "vs_6_0", "-E", "main", "-Fo", work / "triangle.cso", ref / "triangle.hlsl"], check=True)
    receipt = dict(adapter=adapter, host=host.version, global_archive_sha256=GLOBAL_SHA, compiler_sha256=digest(compiler),
                   provider_sha256=digest(provider), guest_sha256=digest(exe), shaders=[])
    expected = expected_pixels()
    for index, fingerprint in SHADERS.items():
        trial = work / str(index)
        trial.mkdir()
        for source in (exe, work / "triangle.cso"):
            shutil.copyfile(source, trial / source.name)
        shutil.copyfile(provider, trial / "d3d12_xs.dll")
        if index == 1748:
            (trial / "velocity").touch()
        ((_, shader, _),) = containers(decompress(archive, entries[index], decoder()))
        source = trial / "pixel.dxbc"
        source.write_bytes(shader)
        if digest(source) != fingerprint:
            raise ValueError(f"Unexpected shader for global entry {index}")
        compiled = []
        cache = trial / "temp/tuxbox_shader_cache"
        with shader_session(cache, host.env, compiled.append):
            session = cache / host.env["TUXBOX_SHADER_SESSION"]
            result = host.run(trial / exe.name, ["--adapter", adapter, "--report", "result.txt"], capture=True, timeout=150)
            report = trial / "result.txt"
            line = report.read_text().strip() if report.exists() else ""
            if result.returncode or line != "stencil-export readback complete":
                sys.stderr.buffer.write(result.stdout[-4000:])
                raise ValueError(f"Stencil export {index} failed: {line}, compilation: {compiled}")
            native = f"Compiled Xbox shader {fingerprint}"
            mirror = f"{fingerprint}_rt2"
            if compiled == [native]:
                path, key = "SV_StencilRef", fingerprint
            elif compiled == [native, f"Compiled Xbox shader {mirror}"]:
                path, key = "R8_UINT stencil-plane copy", mirror
            else:
                raise ValueError(f"Unexpected compilation: {compiled}")
            dxil = digest(session / (key + ".cso"))
        pixels = trial / "pixels.bin"
        actual = pixels.read_bytes()
        if actual != expected:
            diffs = [(i // 5, actual[i:i + 5].hex(), expected[i:i + 5].hex())
                     for i in range(0, len(expected), 5) if expected[i:i + 5] != actual[i:i + 5]]
            raise ValueError(f"Entry {index}: {len(actual)} bytes, mismatching depth/stencil texels {diffs[:8]}")
        receipt["shaders"].append(dict(entry=index, source_sha256=fingerprint, path=path, dxil_sha256=dxil,
                                       matching_texels=len(expected) // 5, readback_sha256=digest(pixels)))
    return receipt


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mount", type=Path)
    parser.add_argument("--proton", type=Path)
    parser.add_argument("--prefix", type=Path)
    parser.add_argument("--adapter", required=True)
    parser.add_argument("--provider", type=Path, default=ROOT / "build/d3d12-smoke/d3d12_xs.dll")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    with TemporaryDirectory(prefix="stencil-export-", dir=ROOT / "build") as tmp:
        report = run(args.mount, Host("proton", args.proton, args.prefix), args.adapter.lower(), args.provider, Path(tmp))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
