#!/usr/bin/env python3
"""Verify Frosty's Lumen visibility dispatch arguments through the Xbox provider."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import struct
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
import pefile
from check_frosty_profile import BUILD
from check_frosty_shaders import GLOBAL_SHA, containers, decoder, decompress, global_archive
from frosty_gpu_guest import run_gpu_guest
from project_tuxbox.inspect import digest


def view_evidence(mount):
    exe = mount / "Frosty/Binaries/XSX/Frosty-XSX-Shipping.exe"
    if digest(exe) != BUILD:
        raise ValueError("unexpected Frosty executable")
    checks = {0x3a0fb84: "ba01000000", 0x3a0fb95: "c744246004000000", 0x3a0fb9d: "89542464",
              0x3a0fc6d: "48894d70", 0x3a0fca8: "48894580", 0x3a101a9: "c745301c000000",
              0x3a101b6: "4c8b6580", 0x3a0fbcb: "c745900c000000", 0x3a0fbd2: "895594",
              0x3a0fcf0: "4c89a580000000", 0x3a0fd2c: "488945e0",
              0x3a104b5: "c745101c000000", 0x3a104c2: "488b45e0"}
    with pefile.PE(str(exe), fast_load=True) as pe:
        for rva, raw in checks.items():
            if pe.get_data(rva, len(bytes.fromhex(raw))) != bytes.fromhex(raw):
                raise ValueError(f"unexpected Lumen buffer instruction at {rva:#x}")
        lea = pe.get_data(0x3a0fc22, 7)
        target = 0x3a0fc22 + 7 + struct.unpack_from("<i", lea, 3)[0]
        name = "Lumen.CompactedVisBufferIndirectArguments\0".encode("utf-16le")
        if lea[:3] != bytes.fromhex("4c8d25") or pe.get_data(target, len(name)) != name:
            raise ValueError("unexpected indirect-buffer name")
        lea = pe.get_data(0x3a0fc14, 7)
        target = 0x3a0fc14 + 7 + struct.unpack_from("<i", lea, 3)[0]
        name = "Lumen.CompactedVisBufferAllocator\0".encode("utf-16le")
        if lea[:3] != bytes.fromhex("488d0d") or pe.get_data(target, len(name)) != name:
            raise ValueError("unexpected allocator-buffer name")
    return {"executable_sha256": BUILD, "buffer_name": "Lumen.CompactedVisBufferIndirectArguments",
            "allocation_element_bytes": 12, "allocation_elements": 1, "allocation_rva": "0x3a0fb84",
            "view_buffer_pointer_rva": "0x3a104c2", "view_format_rva": "0x3a104b5",
            "engine_pixel_format": "PF_R32_UINT", "engine_pixel_format_value": 28,
            "input_buffer_name": "Lumen.CompactedVisBufferAllocator", "input_bytes": 4,
            "input_view_format_rva": "0x3a101a9",
            "evidence": "The named 12-byte buffer allocation is retained in a stack slot; the view descriptor sets pixel format 28 and loads that same slot."}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("mount", "xshader", "provider", "proton", "prefix", "compiler-dir", "work", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    work, proton = args.work.resolve(), args.proton.resolve()
    work.mkdir(parents=True, exist_ok=True)
    view = view_evidence(args.mount)
    archive, entries = global_archive(args.mount)
    ((_, native, _),) = containers(decompress(archive, entries[2140], decoder()))
    expected_hash = "08a948c5de3dabde63a207e37f47b3ce1ec624289f74a16b1854b50fff837e62"
    if hashlib.sha256(native).hexdigest() != expected_hash:
        raise ValueError("unexpected global 2140 shader")
    os.environ["TUXBOX_XSHADER"] = str(args.xshader.resolve())
    with TemporaryDirectory(dir=work) as temp:
        trial = Path(temp)
        (trial / "retail.dxbc").write_bytes(native)
        exe, status, actual = run_gpu_guest(ROOT / "tools/reference/voxel-indirect-setup/guest.cpp", "voxel.exe",
                                            trial, work, proton, args.prefix, args.provider, args.compiler_dir)
        expected = {f"count_{value}": "16" for value in (0, 1, 7, 65537, 0xffffffff)}
        if status or actual != expected:
            raise ValueError(f"indirect setup control returned {status}: {actual}. See {work / 'gpu.log'}")
        adapters = sorted({line.split("Found device: ", 1)[1] for line in (work / "gpu.log").read_text().splitlines()
                           if "Found device: " in line})
        result = {"scope": "Packaged Lumen global 2140 through the TuxBox provider and runtime shader compiler. Five counts check all 16 output and guard words; not an end-to-end lighting check.",
                  "archive_sha256": GLOBAL_SHA, "shader_sha256": expected_hash, "view": view,
                  "adapters": adapters, "readback": actual, "guest_sha256": digest(exe),
                  "xshader_sha256": digest(args.xshader), "provider_sha256": digest(args.provider),
                  "proton": (proton / "version").read_text().strip()}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
