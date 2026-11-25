#!/usr/bin/env python3
"""Compile Frosty's ClearCS shaders from the retail archive and verify typed GPU writes."""
import argparse
import ctypes
from ctypes.util import find_library
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
from tempfile import TemporaryDirectory
import time

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import Toolchain, find_dxc
from check_frosty_shaders import GLOBAL_SHA, containers, decompress, global_archive, require
from project_tuxbox.host import Host
from project_tuxbox.inspect import digest
from project_tuxbox.runtime_shaders import shader_session
from project_tuxbox.shader_prep import compiler_tools


def fnv1a(data):
    value = 0xcbf29ce484222325
    for byte in data:
        value = (value ^ byte) * 0x100000001b3 & 0xffffffffffffffff
    return value


def extract_shaders(mount, xshader, work, *, check_source_cache=False):
    payload, entries = global_archive(mount)
    lz4 = ctypes.CDLL(find_library("lz4")).LZ4_decompress_safe
    lz4.argtypes = [ctypes.c_char_p, ctypes.c_void_p, ctypes.c_int, ctypes.c_int]
    hlsl = work / "clear.hlsl"
    shaders = []
    inputs = [(402, 0x967bc3a70a86c7f4), (379, 0x97a201d874e27271)]
    if check_source_cache:
        inputs.append((414, 0x526075503f59f4c3))
    for index, expected in inputs:
        blob = decompress(payload, entries[index], lz4)
        ((_, cooked, _),) = containers(blob)
        require(fnv1a(cooked) == expected, "Shader does not match the retail input")
        native = work / f"{index}.dxbc"
        native.write_bytes(cooked)
        report = subprocess.check_output([xshader, native, "--hlsl-out", hlsl], text=True)
        require('entry=Some("ClearCS")' in report and "consumed 148/148 bytes" in report, "Unexpected shader code")
        shaders.append({"index": index, "fnv1a64": f"{expected:016x}", "cooked_sha256": digest(native),
                        "hlsl_sha256": digest(hlsl)})
    require(len({row["hlsl_sha256"] for row in shaders}) == 1, "ClearCS translations differ")
    return cooked, hlsl, shaders


def check_tuxbox_cache(dll, host, work, shaders, portable):
    """Exercise the source-keyed Xbox pipeline factory with an isolated guest TEMP."""
    tools = Toolchain()
    provider = work / "d3d12_xs.dll"
    shutil.copyfile(dll, provider)
    obj = tools.compile(ROOT / "tools/reference/clear/guest.cpp", work, "/W4", "/WX", "/DTUXBOX_CACHE_CONTROL",
                        f"/I{ROOT / 'runtime/d3d12'}")
    exe = work / "clear-cache.exe"
    tools.link(exe, obj, dll.with_suffix(".lib").resolve(), "dxgi.lib", "kernel32.lib")
    sources = [(work / f"{row['index']}.dxbc").read_bytes() for row in shaders]
    cache = work / "temp/tuxbox_shader_cache/by_source"
    cache.mkdir(parents=True)
    keys = [cache / (row["cooked_sha256"] + ".cso") for row in shaders]
    (work / "root.dxbc").write_bytes(sources[0])
    observations = {}

    def run(name, source, *, success):
        (work / "source.dxbc").write_bytes(source)
        receipt = work / "readback.txt"
        receipt.unlink(missing_ok=True)
        result = host.run(exe, [], timeout=60, capture=True)
        require(receipt.is_file(), f"{name} produced no GPU receipt")
        readback = dict(line.split("=", 1) for line in receipt.read_text().splitlines())
        observations[name] = {"exit": result.returncode, **readback}
        require("pipeline_hr" in readback, f"{name} did not reach slot 62: {readback}")
        if success:
            require(result.returncode == 0 and readback["pipeline_hr"] == "0x00000000"
                    and all(readback.get(f"{view}_view_{size}_matched_words") == str(size * 140)
                            for view in ("float", "uint") for size in (1, 4)), f"{name}: {readback}")
        else:
            require(result.returncode == 1 and int(readback["pipeline_hr"], 16) & 0x80000000
                    and "float_view_1_matched_words" not in readback, f"{name}: {readback}")

    run("portable_uncached", portable.read_bytes(), success=True)
    run("402_missing", sources[0], success=False)
    shutil.copyfile(portable, keys[1])
    run("402_only_379_cached", sources[0], success=False)
    keys[0].write_bytes(b"DXBC-truncated")
    run("402_corrupt", sources[0], success=False)
    messages = []
    for phase in ("cold", "warm_worker", "warm"):
        with shader_session(cache.parent, host.env, messages.append):
            session = cache.parent / host.env["TUXBOX_SHADER_SESSION"]
            marker = session / "compiler-id"
            if phase == "warm_worker":
                # Keep the worker's pinned cache, but exercise the original
                # path used by guests that cannot read that cache directly.
                deadline = time.monotonic() + 15
                while not marker.exists() and time.monotonic() < deadline:
                    time.sleep(.01)
                marker.unlink()
            run(f"402_on_demand_{phase}", sources[0], success=True)
            if phase == "warm":
                compiler_id = marker.read_text()
            require(digest(session / keys[0].name) == digest(portable), "Unexpected shader session reply")
    require(sum(message.startswith("Compiled Xbox shader ") for message in messages) == 1,
            f"Expected one cold compilation and a warm cache hit: {messages}")
    # With no worker and no requests directory, only a verified persistent
    # result can create this pipeline. A bad checksum must fail immediately.
    with TemporaryDirectory(prefix="run-", dir=cache.parent) as offline:
        session = Path(offline)
        host.env["TUXBOX_SHADER_SESSION"] = session.name
        try:
            (session / "compiler-id").write_text(compiler_id)
            run("402_direct_no_worker", sources[0], success=True)
            (session / keys[0].name).unlink()
            stamp = cache.parent / "runtime" / compiler_id / keys[0].with_suffix(".sha256").name
            stamp.write_text("0" * 64)
            run("402_bad_persistent_checksum", sources[0], success=False)
        finally:
            host.env.pop("TUXBOX_SHADER_SESSION", None)
    for shader, key, source in zip(shaders, keys, sources):
        shutil.copyfile(portable, key)
        run(f"{shader['index']}_warm", source, success=True)
    return {"provider_sha256": digest(provider), "guest_sha256": digest(exe), "archive_sha256": GLOBAL_SHA,
            "backend": host.backend, "host_provider": host.version, "slot": 62, "shaders": shaders,
            "dxil_sha256": digest(portable), "start": 5, "end_exclusive": 102,
            "elements_per_buffer": 140, "dispatch_groups": 2, "cases": observations}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mount", type=Path)
    parser.add_argument("--prefix", type=Path, required=True)
    parser.add_argument("--probe-prefix", type=Path, help="use a separate prefix for the GPU readback guest")
    parser.add_argument("--proton", type=Path)
    parser.add_argument("--xshader", type=Path)
    parser.add_argument("--tuxbox-dll", type=Path,
                        help="test TuxBox slot 62 with cooked input, missing and populated source caches")
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    local_shader, dxc, dxv = compiler_tools()
    xshader = args.xshader.resolve() if args.xshader else local_shader
    work_root = ROOT / "build/frosty-clear"
    work_root.mkdir(parents=True, exist_ok=True)
    host = Host(None, args.proton, args.probe_prefix or args.prefix)
    with TemporaryDirectory(dir=work_root) as temp:
        work = Path(temp)
        cooked, hlsl, shaders = extract_shaders(args.mount, xshader, work, check_source_cache=bool(args.tuxbox_dll))
        cache = work / "clear.cso"
        subprocess.run([dxc, "-T", "cs_6_0", "-E", "main", "-Fo", cache, hlsl], check=True)
        subprocess.run([dxv, cache], check=True)
        original = cache.read_bytes()
        dump = subprocess.check_output([find_dxc(None), "-dumpbin", cache], text=True)
        require("NumThreads=(64,1,1)" in dump and "call void @dx.op.bufferStore.f32" in dump
                and re.search(r"UAV\s+f32\s+buf\s+U0\s+u0\s+1", dump), "Unexpected desktop ClearCS DXIL")
        if args.tuxbox_dll:
            result = check_tuxbox_cache(args.tuxbox_dll.resolve(), host, work, shaders, cache)
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
            return
        header = work / "clear_blobs.h"
        header.write_text("".join(f"const unsigned char {name}[] = {{" + ",".join(map(str, value)) + "};\n"
                                  for name, value in (("cooked_shader", cooked), ("desktop_shader", original))))
        tools = Toolchain()
        obj = tools.compile(ROOT / "tools/reference/clear/guest.cpp", work, "/W4", "/WX",
                            f"/I{work}", f"/I{ROOT / 'runtime/d3d12'}")
        exe = work / "clear.exe"
        tools.link(exe, obj, "d3d12.lib", "dxgi.lib", "kernel32.lib")
        run = host.run(exe, [], timeout=60, capture=True)
        receipt = work / "readback.txt"
        require(receipt.exists(), "GPU guest exited without a receipt")
        observations = dict(line.split("=", 1) for line in receipt.read_text().splitlines())
        result = {"archive_sha256": GLOBAL_SHA, "inputs": shaders, "compiler_sha256": digest(xshader), "dxil_sha256": digest(cache),
                  "numthreads": [64, 1, 1], "cbv_table": 1, "uav_table": 3, "start": 5, "end_exclusive": 102,
                  "unsigned_view_control": "Observed behavior of the cached float-typed DXIL on UINT views",
                  "elements_per_buffer": 140, "dispatch_groups": 2, "host": host.version, "guest_sha256": digest(exe),
                  "readback": observations, "exit": run.returncode}
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
        require(not run.returncode and observations.get("float_view_1_matched_words") == "140"
                and observations.get("float_view_4_matched_words") == "560", str(observations))


if __name__ == "__main__":
    main()
