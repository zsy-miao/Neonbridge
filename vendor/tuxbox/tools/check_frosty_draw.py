#!/usr/bin/env python3
"""Draw Frosty's two observed Slate shader pairs offscreen with controlled red texture, white vertices, and a blue target."""
import argparse
import ctypes
from ctypes.util import find_library
import json
from pathlib import Path
import shutil
import subprocess
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import Toolchain
from check_frosty_clear import fnv1a
from check_frosty_shaders import GLOBAL_SHA, containers, decompress, global_archive, require
from project_tuxbox.host import Host
from project_tuxbox.inspect import digest
from project_tuxbox.shader_prep import FROSTY_PLAN, SOURCE_CACHE, shader_compiler


def run_guest(host, work, exe):
    receipt = work / "readback.txt"
    receipt.unlink(missing_ok=True)
    run = host.run(exe, [], timeout=60, capture=True)
    require(receipt.exists(), "No guest receipt")
    return run.returncode, dict(line.split("=", 1) for line in receipt.read_text().splitlines())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mount", type=Path)
    parser.add_argument("--prefix", type=Path, required=True, help="prefix containing the retail shader cache")
    parser.add_argument("--probe-prefix", type=Path, required=True)
    parser.add_argument("--proton", type=Path)
    parser.add_argument("--xshader", type=Path)
    parser.add_argument("--tuxbox-dll", type=Path, help="submit the cooked graphics stream to TuxBox using its prepared cache")
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    xshader = args.xshader.resolve() if args.xshader else shader_compiler()
    work_root = ROOT / "build/frosty-draw"
    work_root.mkdir(parents=True, exist_ok=True)
    host = Host(None, args.proton, args.probe_prefix)
    payload, entries = global_archive(args.mount)
    lz4 = ctypes.CDLL(find_library("lz4")).LZ4_decompress_safe
    lz4.argtypes = [ctypes.c_char_p, ctypes.c_void_p, ctypes.c_int, ctypes.c_int]
    cache = args.prefix / SOURCE_CACHE
    plan = json.loads(FROSTY_PLAN.read_text())
    profiles = {row["entry"]: row for row in plan["shaders"] if row["archive"] == "global"}
    compiled = plan["source_cache"]["compiled_dxil_sha256"]
    active = [(3643, 0x8e9e3b832c7dd2ae, "vs_6_0", "vertex_shader"),
              (785, 0x01dadaebf074d085, "ps_6_1", "pixel_a"),
              (792, 0xf6d1b9eafc4e1bd3, "ps_6_1", "pixel_b")]
    shaders = []
    with TemporaryDirectory(dir=work_root) as temp:
        work = Path(temp)
        blobs = {}
        for index, input_hash, profile, name in active:
            ((_, cooked, _),) = containers(decompress(payload, entries[index], lz4))
            require(fnv1a(cooked) == input_hash, f"Runtime shader hash mismatch at entry {index}")
            native, hlsl = work / "shader.dxbc", work / "shader.hlsl"
            native.write_bytes(cooked)
            subprocess.run([xshader, native, "--hlsl-out", hlsl], stdout=subprocess.DEVNULL, check=True)
            p = fnv1a(profile.encode())
            key = fnv1a(hlsl.read_bytes()) ^ (((p << 17) | (p >> 47)) & 0xffffffffffffffff)
            source_key = digest(native)
            row = profiles[index]
            require(row["dxbc_sha256"] == source_key and row["cache_key"] == f"{key:016x}",
                    f"Unexpected cooked input or translation at entry {index}")
            cso = cache / (source_key + ".cso")
            if args.tuxbox_dll:
                allowed = {row["observed_dxil_sha256"], compiled[row["cache_key"]]}
                require(digest(cso) in allowed, f"Unexpected first-boot DXIL at entry {index}")
            blobs[name] = cooked if args.tuxbox_dll else cso.read_bytes()
            if name == "vertex_shader":
                blobs["cooked_shader"] = cooked
            shaders.append(dict(entry=index, profile=profile, cooked_fnv1a64=f"{input_hash:016x}", cooked_sha256=digest(native),
                                hlsl_sha256=digest(hlsl), cache_file=cso.name, desktop_sha256=digest(cso)))
        (work / "draw_blobs.h").write_text("".join(
            f"const unsigned char {key}[] = {{" + ",".join(map(str, value)) + "};\n" for key, value in blobs.items()))
        tools = Toolchain()
        flags = ["/DTUXBOX_COOKED_DRAW"] if args.tuxbox_dll else []
        obj = tools.compile(ROOT / "tools/reference/draw/guest.cpp", work, "/W4", "/WX", *flags,
                            f"/I{work}", f"/I{ROOT / 'runtime/d3d12'}")
        exe = work / "draw.exe"
        library = "d3d12.lib"
        details = {}
        scope = "Title-cached DXIL, original cooked root signature, controlled vertex, texture and raster state"
        if args.tuxbox_dll:
            provider = work / "d3d12_xs.dll"
            shutil.copyfile(args.tuxbox_dll, provider)
            library = args.tuxbox_dll.with_suffix(".lib").resolve()
            scope = "Original cooked vertex and pixel shaders submitted to Xbox slot 86 with first-boot DXIL"
            details["provider_sha256"] = digest(provider)
        tools.link(exe, obj, library, "dxgi.lib", "kernel32.lib")
        if args.tuxbox_dll:
            local = work / "temp/tuxbox_shader_cache/by_source"
            local.mkdir(parents=True)
            vertex, pixel_a, pixel_b = (row["cache_file"] for row in shaders)
            for name in (vertex, pixel_b):
                shutil.copyfile(cache / name, local / name)
            controls = {}
            for case in ("missing_pixel", "vertex_filed_as_pixel"):
                if case == "vertex_filed_as_pixel":
                    shutil.copyfile(cache / vertex, local / pixel_a)
                code, observed = run_guest(host, work, exe)
                require(code == 1 and observed.get("entry_785_pipeline") == "0x80004005"
                        and "entry_785_drawn_red" not in observed, f"{case}: {observed}")
                controls[case] = {"exit": code, **observed}
            details["negative_controls"] = controls
            shutil.copyfile(cache / pixel_a, local / pixel_a)
        code, observed = run_guest(host, work, exe)
        result = dict(archive_sha256=GLOBAL_SHA, shaders=shaders, host=host.version, guest_sha256=digest(exe),
                      extent=[16, 16], scissor=[3, 2, 12, 11], vertex_count=3,
                      scope=scope, readback=observed, exit=code, **details)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(result, indent=2) + "\n")
        require(not code and all(observed.get(f"entry_{entry}_drawn_red") == "81"
                                          and observed.get(f"entry_{entry}_guard_blue") == "175"
                                          for entry in (785, 792)), str(observed))


if __name__ == "__main__":
    main()
