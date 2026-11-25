#!/usr/bin/env python3
"""GPU-check cooked Nanite visibility pixel shaders against unsigned 64-bit max."""
import argparse
from contextlib import closing
import ctypes
import json
from pathlib import Path
import struct
import subprocess
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
import pefile

from build import Toolchain, find_dxc
from check_frame_profile import rip
from check_frosty_profile import BUILD
from check_frosty_shaders import GLOBAL_SHA, containers, decompress, global_archive, require
from project_tuxbox.host import Host
from project_tuxbox.shader_prep import shader_compiler
from project_tuxbox.inspect import digest

SHADERS = {2989: "6a68ec901fb2697f60bf3cfbda888e7c3653d28ef94b6961e552b05e6231fb7e",
           2853: "33c3a3b5c3045ac280b15ccaf78e951227c87833ec919c0fbb787c6316d970a6",
           3050: "28000052530a61c5eccae78f762383ef4d53d93f4c23a0b12b2c1ea5564d029b"}



def visibility_format(mount):
    executable = mount / "Frosty/Binaries/XSX/Frosty-XSX-Shipping.exe"
    require(digest(executable) == BUILD, "Unknown Frosty executable")
    with closing(pefile.PE(str(executable), fast_load=True)) as pe:
        base = pe.OPTIONAL_HEADER.ImageBase
        for site, prefix, name in [(0x3a34cde, "488d05", "OutVisBuffer64"),
                                   (0x3a34ced, "488d05", "RWTexture2D<UlongType>"),
                                   (0x3a8f47a, "488d15", "Nanite.VisBuffer64")]:
            target = rip(pe, site, bytes.fromhex(prefix))
            encoded = (name + "\0").encode("utf-16le")
            require(pe.get_data(target, len(encoded)) == encoded, f"Unexpected renderer metadata at {site:#x}")
        require(pe.get_data(0x3a8f3f7, 8) == bytes.fromhex("48c7458744000000"), "Unexpected visibility EPixelFormat")
        table = 0x79bd910
        ptr, index, sx, sy, sz, size, channels = struct.unpack("<Q6I", pe.get_data(table, 32))
        name = "PF_R32G32_UINT\0".encode("utf-16le")
        require(pe.get_data(ptr - base, len(name)) == name and (index, sx, sy, sz, size, channels) == (68, 1, 1, 1, 8, 2),
                "Unexpected visibility format entry")
        store = pe.get_data(0x57d1c00, 10)
        delta, value = struct.unpack_from("<iI", store, 2)
        require(store[:2] == b"\xc7\x05" and 0x57d1c00 + 10 + delta == table + 36 and value == 17,
                "Unexpected D3D12 platform format assignment")
    return dict(executable_sha256=BUILD, texture="Nanite.VisBuffer64", parameter="OutVisBuffer64",
                parameter_type="RWTexture2D<UlongType>", engine_format="PF_R32G32_UINT", engine_index=68,
                allocation_format_rva="0x3a8f3f7", format_table_entry_rva=hex(table),
                d3d12_format_store_rva="0x57d1c00", dxgi_format=17)


def check_psv(cooked):
    count, = struct.unpack_from("<I", cooked, 28)
    for offset in struct.unpack_from(f"<{count}I", cooked, 32):
        if cooked[offset:offset + 4] == b"PSV0":
            info_size, = struct.unpack_from("<I", cooked, offset + 8)
            record = struct.unpack_from("<8I", cooked, offset + 12 + info_size)
            require(record == (1, 24, 6, 0, 0, 0, 2, 0), "Expected one typed Texture2D UAV in space 0 at u0")
            return
    raise ValueError("Missing PSV0")

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mount", type=Path)
    parser.add_argument("--xshader", type=Path)
    parser.add_argument("--proton", type=Path)
    parser.add_argument("--prefix", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    xshader = args.xshader.resolve() if args.xshader else shader_compiler()
    allocation = visibility_format(args.mount)
    host = Host(None, args.proton, args.prefix)
    source = ROOT / "tools/reference/visibility"
    build = ROOT / "build/frosty-visibility"
    build.mkdir(parents=True, exist_ok=True)
    payload, entries = global_archive(args.mount)
    lz4 = ctypes.CDLL("liblz4.so.1").LZ4_decompress_safe
    dxc = find_dxc(None)
    with TemporaryDirectory(dir=build) as temp:
        work = Path(temp)
        shaders = []
        blobs = []
        for index, fingerprint in SHADERS.items():
            ((_, cooked, _),) = containers(decompress(payload, entries[index], lz4))
            dxbc, hlsl = work / f"{index}.dxbc", work / f"{index}.hlsl"
            dxbc.write_bytes(cooked)
            check_psv(cooked)
            require(digest(dxbc) == fingerprint, f"Unexpected capture at global archive index {index}")
            subprocess.run([xshader, dxbc, "--hlsl-out", hlsl], check=True, stdout=subprocess.DEVNULL)
            for stage, text, profile in [("vs", source / "vertex.hlsl", "vs_6_0"), ("ps", hlsl, "ps_6_6")]:
                dxil = work / f"{stage}{index}.dxil"
                defines = ["-DMESH_INPUT=1"] if stage == "vs" and index in (2853, 3050) else []
                if stage == "vs" and index == 3050:
                    defines.append("-DCLIP_INPUT=1")
                subprocess.run([dxc, *defines, "-T", profile, "-E", "main", "-Fo", dxil, text], check=True)
                blobs.append(f"const unsigned char {stage}{index}[] = {{" + ",".join(map(str, dxil.read_bytes())) + "};\n")
            shaders.append(dict(entry=index, input_sha256=fingerprint, hlsl_sha256=digest(hlsl), dxil_sha256=digest(dxil)))
        (work / "visibility_shaders.h").write_text("".join(blobs))
        toolchain = Toolchain()
        obj = toolchain.compile(source / "guest.cpp", work, "/W4", "/WX", f"/I{work}", f"/I{ROOT / 'runtime/d3d12'}")
        exe = work / "visibility.exe"
        toolchain.link(exe, obj, "d3d12.lib", "dxgi.lib", "kernel32.lib")
        run = host.run(exe, [], timeout=65, capture=True)
        receipt = work / "visibility.txt"
        observed = dict(line.split("=", 1) for line in receipt.read_text().splitlines()) if receipt.exists() else {}
        result = dict(archive_sha256=GLOBAL_SHA, allocation=allocation, shaders=shaders, guest_sha256=digest(exe), host=host.version,
                      format="R32G32_UINT", extent=[19, 13], scissor=[3, 2, 16, 10], instances=3, exit=run.returncode,
                      scope="Translated retail pixel shaders, controlled vertex input and depth, competing full-width UAV values",
                      readback=observed)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(result, indent=2) + "\n")
        if run.returncode:
            sys.stderr.buffer.write(run.stdout[-4096:])
        require(run.returncode == 0 and all(observed.get(f"entry_{entry}_matched") == "247" for entry in SHADERS),
                str(observed))


if __name__ == "__main__":
    main()
