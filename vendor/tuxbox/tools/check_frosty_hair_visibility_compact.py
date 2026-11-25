#!/usr/bin/env python3
"""GPU-check the packaged hair visibility compaction shaders and all five outputs."""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import Toolchain
from check_frosty_shaders import GLOBAL_SHA, containers, decoder, decompress, global_archive
from project_tuxbox.host import Host
from project_tuxbox.inspect import digest
from project_tuxbox.shader_prep import compiler_tools

SHADERS = {744: "7b50efe4e1af24ee493d1fdcf898f4f63a2bc648896e49be42365d54982e1272",
           745: "6279fec42a171d61f708225a65d3f023eed1ec79031cbe0e8c97ef5b4a7e7cde"}
GUARD = 0xdeadbeef
CASES = {"none": set(), "all": set(range(64)), "sparse": {1, 7, 31, 32, 47, 60},
         "last": {63}, "multi": {1, 7, 31, 32, 47, 60}}


def words(path, values):
    path.write_bytes(struct.pack(f"<{len(values)}I", *values))


def inputs(work, case, pixels):
    counts = {pixel: pixel % 4 + 1 if case == "multi" else 1 for pixel in pixels}
    cb = [0] * 64
    cb[:4] = [8, 8, 512, 4 if case == "multi" else 1]
    cb[4], cb[6], cb[8] = 0x3f800000, 1, 8
    words(work / "cb.bin", cb)
    words(work / "t0.bin", [0] * 64)
    words(work / "t5.bin", [0x3f000000] * 64)
    for sample in range(4):
        words(work / f"t{sample + 1}.bin", [word for pixel in range(64) for word in
              ((pixel + 1 + sample * 100, 0x800000ff)
               if pixel in pixels and sample < counts[pixel] else (0, 0))])
    words(work / "t6.bin", [1])
    words(work / "t7.bin", [0, 0])
    words(work / "u0.bin", [5])
    words(work / "u1.bin", [GUARD] * 64)
    words(work / "u2.bin", [GUARD] * 1024)
    words(work / "u3.bin", [GUARD] * 1024)
    words(work / "u4.bin", [GUARD] * 64)
    return counts


def verify(work, counts):
    def read(slot):
        data = (work / f"out{slot}.bin").read_bytes()
        return struct.unpack(f"<{len(data) // 4}I", data)

    counter, index, record, coords, coverage = (read(slot) for slot in range(5))
    total = sum(counts.values())
    if counter[0] != 5 + total:
        raise ValueError(f"counter: {counter[0]} instead of {5 + total}")
    seen = set()
    for pixel in range(64):
        x, y = pixel % 8, pixel // 8
        packed, weight = index[y * 64 + x], coverage[y * 64 + x]
        if pixel not in counts:
            if (packed, weight) != (0, 0):
                raise ValueError(f"empty pixel {pixel}: {packed:#x}, {weight:#x}")
            continue
        count, base = counts[pixel], packed & 0x3ffffff
        if packed >> 26 != count or not 5 <= base < 5 + total or weight != 0x3f000000:
            raise ValueError(f"pixel {pixel}: {packed:#x}, {weight:#x}")
        for sample in range(count):
            dst = base + sample
            if dst in seen or dst >= 5 + total:
                raise ValueError(f"overlapping or out-of-range sample {dst}")
            seen.add(dst)
            expected = (0x80000000 + min(255, 256 // count), pixel + 1 + sample * 100)
            if record[2 * dst:2 * dst + 2] != expected or coords[2 * dst:2 * dst + 2] != (x, y):
                raise ValueError(f"pixel {pixel} sample {sample} incorrect")
    for dst in [*range(5), *range(5 + total, 512)]:
        if record[2 * dst:2 * dst + 2] != (GUARD, GUARD) or coords[2 * dst:2 * dst + 2] != (GUARD, GUARD):
            raise ValueError(f"guard overwritten at {dst}")
    if len(seen) != total:
        raise ValueError("not all compacted samples were found")
    return {"pixels": len(counts), "samples": total, "outputs_checked": 5, "buffer_slots_checked": 512}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("mount", "xshader", "proton", "prefix", "work", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    work = args.work.resolve()
    work.mkdir(parents=True, exist_ok=True)
    archive, entries = global_archive(args.mount)
    dxc, dxv = compiler_tools()[1:]
    with TemporaryDirectory(dir=work) as temp:
        trial = Path(temp)
        # Keep the control isolated from the supplied prefix, including its settings.
        subprocess.run(["cp", "-a", "--reflink=auto", args.prefix.resolve(), trial / "prefix"], check=True)
        toolchain = Toolchain()
        source = ROOT / "tools/reference/hair-visibility-compact/guest.cpp"
        obj = toolchain.compile(source, trial, "/W4", "/WX", f"/I{ROOT / 'runtime/d3d12'}")
        exe = trial / "guest.exe"
        toolchain.link(exe, obj, "d3d12.lib", "dxgi.lib", "kernel32.lib")
        host = Host(None, args.proton, trial / "prefix")
        results = {}
        adapter = None
        for entry, expected in SHADERS.items():
            ((_, native, _),) = containers(decompress(archive, entries[entry], decoder()))
            if hashlib.sha256(native).hexdigest() != expected:
                raise ValueError(f"unexpected global shader {entry}")
            (trial / "retail.dxbc").write_bytes(native)
            hlsl, profile, dxil = (trial / name for name in ("translated.hlsl", "translated.profile", "translated.dxil"))
            subprocess.run([args.xshader.resolve(), trial / "retail.dxbc", "--hlsl-out", hlsl, "--profile-out", profile], check=True)
            subprocess.run([dxc, "-T", profile.read_text().strip(), "-E", "main", "-Fo", dxil, hlsl], check=True)
            subprocess.run([dxv, dxil], check=True, stdout=subprocess.DEVNULL)
            cases = {}
            for name, pixels in CASES.items():
                counts = inputs(trial, name, pixels)
                process = host.run(exe, [], timeout=60, capture=True)
                receipt = (trial / "receipt.txt").read_text()
                if process.returncode:
                    raise RuntimeError(f"shader {entry} {name}: {receipt}")
                this_adapter = next(line.split("=", 1)[1] for line in receipt.splitlines() if line.startswith("adapter="))
                if adapter is not None and adapter != this_adapter:
                    raise ValueError("adapter changed during controls")
                adapter = this_adapter
                cases[name] = verify(trial, counts)
            results[entry] = {"cooked_sha256": expected, "hlsl_sha256": digest(hlsl),
                              "dxil_sha256": digest(dxil), "profile": profile.read_text().strip(), "cases": cases}
        result = {"scope": "Two packaged hair visibility compaction shaders, translated DXIL and CPU-modeled GPU readback. Does not validate rendered hair.",
                  "archive_sha256": GLOBAL_SHA, "adapter": adapter, "proton": host.version,
                  "xshader_sha256": digest(args.xshader), "guest_sha256": digest(exe), "shaders": results}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
