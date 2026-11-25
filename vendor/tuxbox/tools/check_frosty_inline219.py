#!/usr/bin/env python3
"""Compare a packaged inline compute shader against its ray-generation permutation."""
import argparse
import json
import os
import shutil
from pathlib import Path
import subprocess
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import Toolchain, find_dxc
from check_frosty_shaders import containers, decoder, decompress, global_archive, serialized_state
from project_tuxbox.host import Host
from project_tuxbox.inspect import digest
from project_tuxbox.runtime_shaders import runtime_shader_compiler

SOURCE = ROOT / "tools/reference/blas"
CASES = ("empty_queue", "geometry_miss", "primary_mask_20", "fallback_mask_01",
         "fallback_excludes_mask_20", "primary_excludes_mask_01")


def build_control(work, dxc, compute=219):
    subprocess.run([dxc, "-T", "cs_6_5", "-E", "main", "-Fh", work / "query_dxil.h",
                    "-Vn", "query_dxil", SOURCE / "query.hlsl"], check=True)
    source = "occlusion_observer.hlsl"
    observer = "occlusion-observer.dxil"
    defines = []
    if compute in (58, 114):
        source, observer = "screen_gather_observer.hlsl", "screen-gather-observer.dxil"
    elif compute in (306, 313, 318, 426, 434):
        observer, defines = "ray-input-observer.dxil", ["-DTRACE_PROBE_INPUTS"]
    subprocess.run([dxc, "-T", "lib_6_3", *defines, "-Fo", work / observer, SOURCE / source], check=True)
    toolchain = Toolchain()
    obj = toolchain.compile(SOURCE / "guest.cpp", work, "/W4", "/WX", f"/DTUXBOX_INLINE{compute}_REPLAY",
                            f"/I{work}", f"/I{ROOT / 'runtime/d3d12'}")
    toolchain.link(work / f"inline{compute}.exe", obj, "d3d12.lib", "dxgi.lib", "kernel32.lib")


def extract_shaders(mount, work, library=218, compute=219):
    archive, entries = global_archive(mount)
    lz4 = decoder()
    _, state = serialized_state(decompress(archive, entries[library], lz4))
    (work / f"{library}.sptr").write_bytes(state)
    if compute == 58:
        _, near = serialized_state(decompress(archive, entries[71], lz4))
        (work / "71.sptr").write_bytes(near)
        ((_, producer, _),) = containers(decompress(archive, entries[114], lz4))
        (work / "114.dxbc").write_bytes(producer)
    if compute in (306, 313):
        _, retrace = serialized_state(decompress(archive, entries[311], lz4))
        (work / "311.sptr").write_bytes(retrace)
    if compute == 313:
        ((_, producer, _),) = containers(decompress(archive, entries[306], lz4))
        (work / "306.dxbc").write_bytes(producer)
    ((_, bytecode, _),) = containers(decompress(archive, entries[compute], lz4))
    (work / f"{compute}.dxbc").write_bytes(bytecode)


def compile_shaders(xshader, dxc, work, library=218, compute=219):
    hashes = {}
    shaders = [(library, "sptr", "lib_6_3"), (compute, "dxbc", "cs_6_5")]
    if compute == 58:
        shaders.extend(((71, "sptr", "lib_6_3"), (114, "dxbc", "cs_6_5")))
    if compute == 313:
        shaders.append((306, "dxbc", "cs_6_5"))
    if compute in (306, 313):
        shaders.append((311, "sptr", "lib_6_3"))
    for entry, suffix, expected in shaders:
        source = work / f"{entry}.{suffix}"
        hlsl, profile, dxil = (work / f"{entry}.{ext}" for ext in ("hlsl", "profile", "dxil"))
        subprocess.run([xshader, source, "--hlsl-out", hlsl, "--profile-out", profile], check=True)
        if profile.read_text().strip() != expected:
            raise ValueError(f"Unexpected desktop shader profile for {entry}")
        flags = [] if suffix == "sptr" else ["-E", "main"]
        subprocess.run([dxc, "-T", expected, *flags, "-Fo", dxil, hlsl], check=True)
        hashes[entry] = {"source_sha256": digest(source), "dxil_sha256": digest(dxil)}
    return hashes


def collect(work, host, compute=219):
    receipt = work / "results.txt"
    executable = work / f"inline{compute}.exe"
    with runtime_shader_compiler("proton", host.env, executable, lambda _: None):
        result = host.run(executable, ["desktop", receipt.name], capture=True, timeout=180)
    if not receipt.exists():
        raise RuntimeError(f"Inline {compute} guest exited {result.returncode}: {result.stdout[-1500:]!r}")
    fields = dict(line.split("=", 1) for line in receipt.read_text().splitlines() if "=" in line)
    if result.returncode:
        raise RuntimeError(f"{receipt.read_text()[-1500:]}\n{result.stdout[-1500:]!r}")
    cases = CASES if compute == 219 else ("empty_queue", "sky_cube", "triangle_hits", "lit_card", "reversed_winding")
    indices = list(enumerate(cases))
    if compute in (58, 114):
        indices = list(enumerate(("empty_queue", "miss", "triangle", "reversed_triangle")))
        if compute == 58:
            indices = [*indices[:2], (5, "near_triangle"), (6, "far_triangle")]
        if compute == 114:
            indices.append((4, "reversed_second_interval"))
    if compute in (426, 434):
        indices = list(enumerate(("reordered_sky", "triangle_hit")))
        if compute == 434:
            indices.append((2, "excluded_near_mask"))
    if compute == 313:
        indices = [*indices[:3], (5, "far_geometry")]
    run = {"adapter": fields["adapter"], "host": host.version,
           "cases": {case: fields[f"inline{compute}_{index}"] for index, case in indices}}
    if compute in (306, 313):
        run["retrace"] = {case: fields[f"retrace_{compute}_case_{index}"] for index, case in indices}
        run["chained_311_hits"] = {case: int(fields[f"radiance_311_{case}_triangle_hits"])
                                   for case in (("empty", "tile", "geometry", "far_geometry") if compute == 313 else ("empty", "tile", "geometry"))}
    return run


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for option in ("mount", "xshader", "provider", "proton", "prefix", "output"):
        parser.add_argument(f"--{option}", required=True, type=Path)
    parser.add_argument("--entry", type=int, choices=(58, 114, 219, 306, 313, 318, 426, 434), default=219)
    args = parser.parse_args()
    library = {58: 64, 114: 71, 219: 218, 306: 299, 313: 299, 318: 323, 426: 421, 434: 432}[args.entry]
    (ROOT / "build").mkdir(exist_ok=True)
    with TemporaryDirectory(dir=ROOT / "build") as tmp:
        work = Path(tmp)
        dxc = find_dxc(None)
        os.environ["TUXBOX_XSHADER"] = str(args.xshader.resolve())
        shutil.copyfile(args.provider, work / "d3d12_xs.dll")
        extract_shaders(args.mount, work, library, args.entry)
        hashes = compile_shaders(args.xshader.resolve(), dxc, work, library, args.entry)
        build_control(work, dxc, args.entry)
        run = collect(work, Host(None, args.proton, args.prefix), args.entry)
        args.output.write_text(json.dumps({"shaders": hashes, "provider_sha256": digest(args.provider), **run}, indent=2) + "\n")


if __name__ == "__main__":
    main()
