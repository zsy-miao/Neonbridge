#!/usr/bin/env python3
"""Check grouped compute register writes, EXEC-mask changes and adjacent UAV accesses on the GPU."""
import argparse
import ctypes
from ctypes.util import find_library
import json
from pathlib import Path
import struct
import subprocess
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import find_dxc
from check_frosty_shaders import containers, decompress, global_archive
from frosty_gpu_guest import run_gpu_guest
from project_tuxbox.inspect import digest


def expected(lane):
    a, b, c = 11 + lane, 81 + lane, 114 + lane
    if lane % 2 == 0:
        a += 17
        b = a ^ 0x9abc
        c = b
        b += 53
    if lane % 5 == 2:
        a = c + 73
        b ^= a
    return a, b, c


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("mount", "provider", "proton", "prefix", "compiler-dir", "translator", "work", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    a = parser.parse_args()
    work = a.work.resolve()
    work.mkdir(parents=True, exist_ok=True)
    with TemporaryDirectory(dir=work) as tmp:
        trial = Path(tmp)
        module = a.translator.resolve() / "crates/tuxbox-xshader/src/hlsl/exec_group.rs"
        # Compile the actual translator pass independently of Wine and the retail package.
        driver = '#[path = ' + json.dumps(str(module)) + '] mod exec_group;\n'
        driver += 'fn main() { print!("{}", exec_group::group(&std::fs::read_to_string(std::env::args().nth(1).unwrap()).unwrap())); }\n'
        (trial / "group.rs").write_text(driver)
        subprocess.run(["rustc", "--edition=2021", trial / "group.rs", "-o", trial / "group"], check=True)
        fixture = ROOT / "tools/reference/exec-group/control.hlsl"
        grouped = subprocess.check_output([trial / "group", fixture], text=True)
        if grouped.count("if (exec_lo & 1u) {") != 3 or "if (exec_lo & 1u) v2 = output[" not in grouped:
            raise ValueError("The groups must end at both the UAV store and the guarded UAV load")
        (trial / "exchange.hlsl").write_text(grouped)
        subprocess.run([find_dxc(None), "-T", "cs_6_0", "-E", "main", "-Fo", trial / "exchange.cso",
                        trial / "exchange.hlsl"], check=True)
        (trial / "expected.bin").write_bytes(b"".join(struct.pack("<3I", *expected(i)) for i in range(256)))
        # Reuse the existing four-table compute guest and its verified root signature.
        archive, records = global_archive(a.mount)
        lz4 = ctypes.CDLL(find_library("lz4")).LZ4_decompress_safe
        lz4.argtypes = [ctypes.c_char_p, ctypes.c_void_p, ctypes.c_int, ctypes.c_int]
        (_, blob, _), = containers(decompress(archive, records[2227], lz4))
        (trial / "retail.dxbc").write_bytes(blob)
        exe, status, actual = run_gpu_guest(ROOT / "tools/reference/compute-swizzle/guest.cpp", "mask.exe", trial,
                                            work, a.proton.resolve(), a.prefix, a.provider, a.compiler_dir)
        if status or actual.get("matched") != "768":
            raise ValueError(f"Mask/UAV regression failed: {status}, {actual}")
        receipt = dict(threads=256, matched_words=768, groups=3, source_sha256=digest(fixture),
                       translated_sha256=digest(trial / "exchange.hlsl"), shader_sha256=digest(trial / "exchange.cso"),
                       shader_group_source_sha256=digest(module), guest_sha256=digest(exe),
                       adapters=[line.split("Found device: ", 1)[1] for line in (work / "gpu.log").read_text().splitlines()
                                 if "Found device: " in line],
                       scope="Both alternating parity and modulo-five predicates, disabled destination preservation, write-after-write register dependencies, a UAV store and a predicated UAV read.")
    a.output.parent.mkdir(parents=True, exist_ok=True)
    a.output.write_text(json.dumps(receipt, indent=2) + "\n")


if __name__ == "__main__":
    main()
