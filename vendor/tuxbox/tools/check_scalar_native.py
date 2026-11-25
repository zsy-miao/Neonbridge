#!/usr/bin/env python3
"""Paired production-compiler validation of synthetic native scalar programs.

The containers have only project-generated RDNA2 ISA and minimal metadata.
Nothing is extracted from a title. The result is compilation, not rendering.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
from pathlib import Path
import shutil
import struct
import subprocess
import sys
from tempfile import TemporaryDirectory
import zlib

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from project_tuxbox.runtime_shaders import compile_request, ShaderRefused


OPS = {
    "s_brev_b64": 0xbe820c02,
    "s_bcnt0_i32_b32": 0xbe820d02,
    "s_bcnt0_i32_b64": 0xbe830e02,
    "s_ff0_i32_b32": 0xbe821102,
    "s_ff0_i32_b64": 0xbe821202,
    "s_flbit_i32_b64": 0xbe831602,
    "s_flbit_i32": 0xbe821702,
    "s_flbit_i32_i64": 0xbe831802,
    "s_bitset0_b64": 0xbe821c04,
    "s_bitset1_b64": 0xbe821e02,
    "s_bitcmp0_b64": 0xbf0e0402,
    "s_bitcmp1_b64": 0xbf0f0402,
}


def pack(words):
    return struct.pack("<" + "I" * len(words), *words)


def native_container(code, resources=()):
    # Synthetic FSSM layout 6, one entry, code immediately after metadata.
    image = bytearray(0x170)

    def qword(offset, value):
        struct.pack_into("<Q", image, offset, value)

    qword(0, 6)
    for slot in range(12):
        qword(8 + 8 * slot, 0x80 if slot == 7 else 0xdeadbeefdeadbeef)
    qword(0x80, 0x170)
    qword(0x88, len(code) * 4)
    qword(0xe0, 0x100)
    struct.pack_into("<II", image, 0xe8, 1, 26)
    struct.pack_into("<H", image, 0x112, 6)
    image[0x114:0x11a] = b"Sprite"
    image[0x134:0x13c] = b"MainFake"
    struct.pack_into("<4I", image, 0x160, 0, 64, 1, 1)
    image.extend(pack(code))
    fssm = b"FSSM" + pack([0x101]) + struct.pack("<Q", len(image)) + image
    compressed = zlib.compress(fssm)
    section = b"XBCH" + pack([3]) + struct.pack("<QQ", len(compressed), len(fssm)) + compressed
    xdat = b"XDAT" + pack([2, 36, 13, 60, len(section), 7,
                           60 + ((len(section) + 3) // 4) * 4, 4]) + section
    xdat += bytes((-len(xdat)) % 4) + b"NOBC"
    body = (pack([0x50060, (24 + len(xdat)) // 4]) + b"DXIL"
            + pack([0x100, 16 + len(xdat) - 4, 4]) + xdat)
    info = bytearray(48)
    info[24] = 5  # compute
    struct.pack_into("<3I", info, 36, 64, 1, 1)
    psv = pack([48]) + info + pack([len(resources)])
    if resources:
        psv += pack([16, *[word for row in resources for word in row]])
    chunks = [b"DXIL" + pack([len(body)]) + body, b"PSV0" + pack([len(psv)]) + psv]
    return (b"DXBC" + bytes(16)
            + pack([1, 40 + sum(map(len, chunks)), 2, 40, 40 + len(chunks[0])])
            + b"".join(chunks))


def cases():
    prefix = [0xbe8203ff, 0x80000000, 0xbe8303ff, 0x00102030,
              0xbe8403ff, 63, 0xbf068004]
    end = [0xbf810000]
    for name, op in OPS.items():
        yield name, native_container(prefix + [op] + end), "refused", "compiled"
    dynamic = [0xf4080200, 0xfa000000, 0xf4240084, 0xfa000000,
               0xbf8cc07f, 0xbe820c02]
    yield "runtime_cbv", native_container(dynamic + end, [(2, 0, 0, 0)]), "refused", "compiled"
    yield "ordinary_control", native_container(end), "compiled", "compiled"
    negatives = {
        "unknown_half": [0xbe8203ff, 123, 0xbe820c02],
        "exec": [0xbe820c7e],
        "vcc": [0xbe820c6a],
        "copied_exec": [0xbe82047e, 0xbe820c02],
        "clobbered_half": [0xbe8203ff, 1, 0xbe8303ff, 2, 0xbe83037e, 0xbe820c02],
        "branch_entry": [0xbf820004, 0xbe8203ff, 1, 0xbe8303ff, 2, 0xbe820c02],
    }
    for name, code in negatives.items():
        yield name, native_container(code + end), "refused", "refused"


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("baseline", "candidate", "dxc", "dxv", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists():
        parser.error("choose a fresh receipt")
    names = ("baseline", "candidate", "dxc", "dxv")
    originals = {name: getattr(args, name).resolve() for name in names}
    fingerprints = {name: digest(path) for name, path in originals.items()}
    with TemporaryDirectory(prefix="scalar-native-") as tmp:
        root = Path(tmp)
        emitters = {}
        for name in names[:2]:
            dest = root / name / "xshader"
            dest.parent.mkdir()
            shutil.copyfile(originals[name], dest)
            dest.chmod(0o700)
            if digest(dest) != fingerprints[name]:
                raise ValueError("emitter changed during snapshot")
            emitters[name] = dest

        def pair(case):
            label, data, before, after = case
            key = hashlib.sha256(data).hexdigest()
            directory = root / label
            directory.mkdir()
            source = directory / (key + ".dxbc")
            source.write_bytes(data)
            row = {"case": label, "request_sha256": key}
            for name, expect in zip(names[:2], (before, after)):
                cache = directory / name
                cache.mkdir()
                tools = (emitters[name], originals["dxc"], originals["dxv"])
                try:
                    if not compile_request(source, key, tools, cache, None):
                        raise ValueError("unexpected cache hit")
                    output = cache / (key + ".cso")
                    observed = {"status": "compiled", "dxil_sha256": digest(output)}
                except ShaderRefused as exc:
                    if exc.cached:
                        raise ValueError("unexpected cached refusal") from exc
                    observed = {"status": "refused"}
                if observed["status"] != expect:
                    raise ValueError(f"{label}/{name}: expected {expect}, got {observed}")
                row[name] = observed
            if before == after == "compiled" and row["baseline"] != row["candidate"]:
                raise ValueError(f"unchanged control differs: {label}")
            return row

        with ThreadPoolExecutor(max_workers=2) as pool:
            rows = list(pool.map(pair, cases()))
    if fingerprints != {name: digest(path) for name, path in originals.items()}:
        raise ValueError("compiler tools changed")
    recovered = sum(row["baseline"]["status"] == "refused"
                    and row["candidate"]["status"] == "compiled" for row in rows)
    receipt = dict(scope="CPU production compiler pipeline, synthetic native DXBC; no GPU or retail coverage",
                   tools_sha256=fingerprints, cases=len(rows), recovered=recovered,
                   regressions=0, rows=rows)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("x") as stream:
        json.dump(receipt, stream, indent=2)
        stream.write("\n")
    print(f"PASS {len(rows)} native cases, {recovered} recovered, zero regressions")


if __name__ == "__main__":
    main()
