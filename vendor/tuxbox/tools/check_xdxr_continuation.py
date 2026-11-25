#!/usr/bin/env python3
"""Compile a translated Frosty miss shader with both providers and cross-link on the console compiler."""
import argparse
import json
from pathlib import Path
import shutil
import struct
import sys
from tempfile import TemporaryDirectory

import pefile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import Toolchain
from project_tuxbox.host import Host
from project_tuxbox.inspect import digest

DRIVER_SHA256 = "036b9a0d8569ba662e52e2f7454c3f0e6506415b3b4f7423d51f3c7e31a3a288"


def extract_driver(path, stage):
    if digest(path) != DRIVER_SHA256:
        raise ValueError("These embedded resource addresses belong to the identified Frosty xgs12_10.dll")
    with pefile.PE(str(path), fast_load=True) as pe:
        def string_at(pointer):
            return pe.get_string_at_rva(pointer - pe.OPTIONAL_HEADER.ImageBase)
        chunks = struct.unpack("<11Q", pe.get_data(0x144E30, 88))
        (stage / "runtime.ll").write_bytes(b"".join(string_at(p) for p in chunks))
        fields = struct.iter_unpack("<QQ", pe.get_data(0x144B90, 42 * 16))
        (stage / "fields.tsv").write_text("".join(f"{string_at(p).decode()}\t{code}\n" for p, code in fields))
        for name, address in [("callbacks.dxil", 0x18F320), ("scheduler.dxil", 0x200650)]:
            header = pe.get_data(address, 32)
            if header[:4] != b"DXBC":
                raise ValueError(f"Missing driver library {name}")
            size, = struct.unpack_from("<I", header, 24)
            (stage / name).write_bytes(pe.get_data(address, size))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--system32", type=Path, required=True)
    parser.add_argument("--dxc-dir", type=Path, default=ROOT / "build/dxc-utils")
    parser.add_argument("--shader", type=Path, required=True)
    parser.add_argument("--entry", required=True)
    parser.add_argument("--proton", type=Path)
    parser.add_argument("--prefix", type=Path)
    args = parser.parse_args()
    host = Host(None, args.proton, args.prefix)
    with TemporaryDirectory(prefix="tuxbox-xdxr-leaf-") as folder:
        stage = Path(folder)
        extract_driver(args.system32 / "xgs12_10.dll", stage)
        shutil.copyfile(args.shader, stage / "shader.dxil")
        for name in ("dxcompiler.dll", "dxcompiler_xs.dll"):
            shutil.copyfile(args.dxc_dir / name, stage / name)
        for name in ("scdxil.dll", "xbsc_xs.dll", "newbe_xs.dll"):
            shutil.copyfile(args.system32 / name, stage / name)
        shutil.copyfile(args.system32 / "dxcompiler_xs.dll", stage / "control-xs.dll")
        toolchain = Toolchain()
        obj = toolchain.compile(ROOT / "tools/reference/xdxr-factory/continuation.cpp", stage, "/W4", "/WX")
        exe = stage / "xdxr-continuation.exe"
        toolchain.link(exe, obj, "kernel32.lib")
        run = host.run(exe, [args.entry], timeout=60, capture=True)
        if run.returncode:
            raise RuntimeError(run.stdout.decode(errors="replace")[-2500:])
        result = {"shader": args.entry, "shader_sha256": digest(args.shader), "host": host.version}
        for line in (stage / "report.txt").read_text().splitlines():
            name, state, stack, collection, scheduler = line.split()
            result[name] = dict(state=int(state), stack_bytes=int(stack), collection_bytes=int(collection),
                                native_linked_scheduler_bytes=int(scheduler), payload_minus_one=True)
        print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
