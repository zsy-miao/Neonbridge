#!/usr/bin/env python3
"""Draw packed Nanite clusters through Xbox mesh and pixel shaders from Frosty's local archive."""
import argparse
import ctypes
from ctypes.util import find_library
import json
from pathlib import Path
import shutil
import struct
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import Toolchain
from check_frosty_shaders import GLOBAL_SHA, containers, decompress, global_archive
from project_tuxbox.host import Host
from project_tuxbox.inspect import digest
from project_tuxbox.runtime_shaders import shader_session
from frosty_pipeline_checks import check_packaged_pipelines, guest

PROGRAMS = {
    3194: "0d4059821b7f492a8b6672e87fc9bdff6e4d826966b6bb5c78de9a37dc42dcb6",
    2886: "c5100d26c3b55cdfc22c3a62ddb8287e6f025eca2f929fea180a30496ecc95dc",
    3130: "0cd12c7f55a779bfd6ec8c234c515c129b9e219c39e75faac3bd7cf11d283442",
    2853: "33c3a3b5c3045ac280b15ccaf78e951227c87833ec919c0fbb787c6316d970a6",
}


def run_draw(host, exe, adapter, cache, entry, cold):
    pixel = 3130 if entry == 2886 else 2853
    target = "R32_UINT" if entry == 2886 else "R32G32_UINT"
    keys = {PROGRAMS[entry], PROGRAMS[pixel]}
    steps = [f"Compiled Xbox shader {key}" for key in cold]
    compiled = []
    with shader_session(cache, host.env, compiled.append):
        session = cache / host.env["TUXBOX_SHADER_SESSION"]
        flag = ["--alternate"] if entry == 2886 else []
        expected = (f"nanite-mesh PASS pci={adapter} ms={entry} ps={pixel} format={target} "
                    "texels=1729/1729 primitive0=69 primitive96=69 empty=0 invalid=0 clipped=23 group0=0 groups01=69")
        line = guest(host, exe, adapter, flag, expected)
        replies = {path.stem: path for path in session.glob("*.cso")}
        if set(replies) != keys or list(session.glob("*.failed")) or sorted(compiled) != sorted(steps):
            raise ValueError(f"Incomplete mesh shader compilation: {compiled}, {list(replies)}")
        if cold:
            for key, name in ((PROGRAMS[3194], "portable-mesh.cso"), (PROGRAMS[2853], "wrong-stage.cso")):
                if key in replies:
                    shutil.copyfile(replies[key], exe.with_name(name))
        return {"receipt": line, "compiled": sorted(cold), "dxil_sha256": {key: digest(path) for key, path in replies.items()}}


def check(mount, host, adapter, provider, trial, all_mesh=False):
    payload, entries = global_archive(mount)
    lz4 = ctypes.CDLL(find_library("lz4") or "liblz4.so.1").LZ4_decompress_safe
    for index, source_hash in PROGRAMS.items():
        ((_, source, _),) = containers(decompress(payload, entries[index], lz4))
        path = trial / f"{index}.dxbc"
        path.write_bytes(source)
        if digest(path) != source_hash:
            raise ValueError(f"Unexpected packaged shader at {index}")
    toolchain = Toolchain()
    obj = toolchain.compile(ROOT / "tools/reference/mesh/nanite.cpp", trial, "/W4", "/WX", f"/I{ROOT / 'runtime/d3d12'}")
    exe = trial / "nanite.exe"
    toolchain.link(exe, obj, str(ROOT / "build/d3d12-smoke/d3d12_xs.lib"), "dxgi.lib", "kernel32.lib")
    shutil.copyfile(provider, trial / "d3d12_xs.dll")
    cache = trial / "temp/tuxbox_shader_cache"
    runs = {}
    for index in (3194, 2886):
        first = {PROGRAMS[index], PROGRAMS[2853]} if index == 3194 else {PROGRAMS[index], PROGRAMS[3130]}
        runs[str(index)] = {
            "cold": run_draw(host, exe, adapter, cache, index, first),
            "warm": run_draw(host, exe, adapter, cache, index, set()),
        }
        if runs[str(index)]["cold"]["dxil_sha256"] != runs[str(index)]["warm"]["dxil_sha256"]:
            raise ValueError("Cold and warm responses differed")
    # Exercise the stripped AS lookup with a synthetic empty shader envelope.
    # Frosty's reviewed archive contains no amplification programs.
    env = trial / "as-envelope.dxbc"
    env.write_bytes(struct.pack("<19I", 0x43425844, 0, 0, 0, 0, 1, 76, 1, 36, 0x4c495844, 32, 0xe0065,
                                8, 0x4c495844, 0x105, 20, 4, 0x54414458, 0x43424f4e))
    by_source = cache / "by_source"
    by_source.mkdir()
    shutil.copyfile(trial / "wrong-stage.cso", by_source / (digest(env) + ".cso"))
    host.env.pop("TUXBOX_SHADER_SESSION", None)
    controls = guest(host, exe, adapter, ["--check-cache"], "nanite-mesh-cache PASS missing_ms=E_NOTIMPL "
                     "wrong_ms=E_NOTIMPL wrong_as=E_NOTIMPL cleared_outputs=3")
    result = {"host": host.version, "adapter": adapter, "guest_sha256": digest(exe), "d3d12_xs_sha256": digest(provider),
            "global_archive_sha256": GLOBAL_SHA, "source_sha256": PROGRAMS, "runs": runs, "cache_controls": controls,
            "fixture": "Seven 19x13 depth or visibility readbacks per pipeline cover seeded visibility values and scissor guards. "
                       "The positive draws use packed vertex and triangle streams with a unit instance transform. "
                       "Primitive 96 references vertices 192 to 194. Additional draws cover a projected triangle, "
                       "an empty cluster, out-of-range triangle indices and two mesh groups selecting different clusters."}
    if all_mesh:
        mesh = [index for index, entry in enumerate(entries) if entry[3] == 1]
        pipelines = check_packaged_pipelines(payload, entries, lz4, host, adapter, trial, exe,
                                             mesh, {"ms_6_5", "ms_6_6"}, "nanite-mesh")
        pipelines["scope"] = ("Each stripped mesh stage created an Xbox graphics pipeline on this adapter "
                              "with the translated shader and no pixel stage.")
        result["mesh_pipelines"] = pipelines
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mount", type=Path)
    parser.add_argument("--provider", type=Path, default=ROOT / "build/d3d12-smoke/d3d12_xs.dll")
    parser.add_argument("--adapter", required=True)
    parser.add_argument("--proton", type=Path)
    parser.add_argument("--prefix", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--all-mesh", action="store_true",
                        help="also compile, validate and create a pipeline for every packaged mesh program")
    args = parser.parse_args()
    with TemporaryDirectory(prefix="nanite-mesh-", dir=ROOT / "build") as tmp:
        result = check(args.mount, Host("proton", args.proton, args.prefix), args.adapter.lower(), args.provider,
                       Path(tmp), args.all_mesh)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
