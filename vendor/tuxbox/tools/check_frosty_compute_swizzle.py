#!/usr/bin/env python3
"""Dispatch the retail 2227 shader and check each of its lane exchanges with known data and EXEC masks."""
import argparse
import ctypes
from ctypes.util import find_library
import hashlib
import json
from pathlib import Path
import re
import struct
import subprocess
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import find_dxc
from check_frosty_shaders import GLOBAL_SHA, containers, decompress, global_archive
from frosty_gpu_guest import run_gpu_guest
from project_tuxbox.inspect import digest


def create_exchange_control(trial, report, hlsl):
    ops = re.findall(r"^  0x(\w+): (\w{8}) \w{8} +ds_swizzle_b32 +v(\d+), v(\d+),", report, re.M)
    blocks = dict(re.findall(r"    // 0x(\w+) ds_swizzle_b32\n(.*?)(?=    //)", hlsl, re.S))
    if len(ops) != 33 or len(ops) != len(blocks):
        raise ValueError("Mismatch between disassembled and emitted swizzles")
    shader = ["RWBuffer<uint> output : register(u0);", "groupshared uint tb_swizzle[64];",
              "[numthreads(8,8,1)] void main(uint3 tid : SV_GroupThreadID, uint3 gid : SV_GroupID) {",
              "uint tb_lane = tid.x + 8u * tid.y;",
              "uint exec_lo = gid.x == 0u || (gid.x == 1u && tb_lane%5u != 1u) || "
              "(gid.x == 2u && (tb_lane==3u || tb_lane==34u || tb_lane==42u));"]
    expected = []
    for step, (offset, instruction, dst, src) in enumerate(ops):
        mode = int(instruction, 16) & 0xffff
        if mode not in (0x41f, 0x201f):
            raise ValueError(f"Unexpected swizzle {mode:#x} at {offset}")
        shader.append("{")
        shader.append(f"uint v{src} = 0x91800000u + {step}u*1024u + gid.x*64u + tb_lane;")
        if src != dst:
            shader.append(f"uint v{dst} = 0xdefaced0u;")
        shader.extend([blocks[offset], f"output[{step*256}u+gid.x*64u+tb_lane] = v{dst};", "}"])
        for group in range(4):
            enabled = [group == 0 or (group == 1 and lane % 5 != 1) or (group == 2 and lane in (3, 34, 42))
                       for lane in range(64)]
            seed = 0x91800000 + step * 1024 + group * 64
            for lane in range(64):
                neighbor = lane ^ (1 if mode == 0x41f else 8)
                old = seed + lane if src == dst else 0xdefaced0
                expected.append((seed + neighbor if enabled[neighbor] else 0) if enabled[lane] else old)
    (trial / "expected.bin").write_bytes(struct.pack(f"<{len(expected)}I", *expected))
    (trial / "exchange.hlsl").write_text("\n".join(shader + ["}"]))
    return len(ops), len(expected)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("mount", "provider", "proton", "prefix", "compiler-dir", "xshader", "work", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    work, proton = args.work.resolve(), args.proton.resolve()
    work.mkdir(parents=True, exist_ok=True)
    with TemporaryDirectory(dir=work) as tmp:
        trial = Path(tmp)
        archive, entries = global_archive(args.mount)
        lz4 = ctypes.CDLL(find_library("lz4")).LZ4_decompress_safe
        lz4.argtypes = [ctypes.c_char_p, ctypes.c_void_p, ctypes.c_int, ctypes.c_int]
        (_, native, _), = containers(decompress(archive, entries[2227], lz4))
        cooked_hash = hashlib.sha256(native).hexdigest()
        if cooked_hash != "d6ce4f1984b92b4106c31b052c92e1e5dccd60943a06da7a250fcb132c28d12b":
            raise ValueError("Unexpected entry 2227")
        (trial / "retail.dxbc").write_bytes(native)
        report = subprocess.check_output([args.xshader.resolve(), trial / "retail.dxbc", "--hlsl-out",
                                          trial / "retail.hlsl"], text=True)
        count, words = create_exchange_control(trial, report, (trial / "retail.hlsl").read_text())
        subprocess.run([find_dxc(None), "-T", "cs_6_0", "-E", "main", "-Fo", trial / "exchange.cso",
                        trial / "exchange.hlsl"], check=True)
        exe, status, actual = run_gpu_guest(ROOT / "tools/reference/compute-swizzle/guest.cpp", "swizzle.exe",
                                            trial, work, proton, args.prefix, args.provider, args.compiler_dir)
        if status or actual != {"retail_dispatch_groups": "4", "matched": str(words)}:
            raise ValueError(f"GPU control failed: {status}, {actual}. See {work / 'gpu.log'}")
        adapters = [line.split("Found device: ", 1)[1] for line in (work / "gpu.log").read_text().splitlines()
                    if "Found device: " in line]
        receipt = dict(archive_entry=2227, global_archive_sha256=GLOBAL_SHA, cooked_sha256=cooked_hash,
                       native_instructions=769, group=[8, 8, 1], swizzles=count, op="DS_SWIZZLE_B32",
                       modes=["0x041f", "0x201f"], readback=actual, adapters=adapters,
                       scope="Full retail compute PSO created via Xbox slot 62 and dispatched with zero constants, null texture views and valid sampler. Each original emitted exchange separately executed with distinct lane words and full, sparse, partial and zero EXEC masks; compared to CPU, including unchanged destinations.",
                       guest_sha256=digest(exe), provider_sha256=digest(args.provider),
                       patch_sha256=digest(ROOT / "tools/reference/tombox-compute-swizzle.patch"),
                       proton=(proton / "version").read_text().strip())
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(receipt, indent=2) + "\n")


if __name__ == "__main__":
    main()
