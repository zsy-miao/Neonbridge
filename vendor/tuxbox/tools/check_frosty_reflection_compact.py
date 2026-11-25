#!/usr/bin/env python3
"""Check Frosty's reflection compaction using real shader resources and GPU readback."""
import argparse
import json
from pathlib import Path
import struct
import subprocess
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import Toolchain
from check_frosty_shaders import containers, decoder, decompress, global_archive
from project_tuxbox.host import Host
from project_tuxbox.inspect import digest
from project_tuxbox.shader_prep import compiler_tools

GUARD = 0xdeadbeef
GROUP_SIZES = {580: 64, 582: 64, 576: 128, 579: 256, 589: 1024, 601: 512, 587: 128, 592: 512, 600: 1024, 606: 256}
CASES = ("none", "full", "mixed", "lane31", "lane32", "lane63", "one_tile", "no_tiles")


def f32(n):
    return struct.unpack("<I", struct.pack("<f", n))[0]


def xy(lane):
    return (((lane >> 1) & 2) | (lane & 1) | ((lane >> 2) & 4),
            ((lane >> 1) & 1) | ((lane >> 3) & 4) | ((lane >> 2) & 2))


def active(case, lane):
    x, y = xy(lane)
    return (case == "full" or (case == "one_tile" and lane < 64) or (case == "mixed" and (y * 8 + x) % 3 != 0)
            or (case.startswith("lane") and (x, y) == xy(int(case[4:]))))


def words(path, values):
    path.write_bytes(struct.pack(f"<{len(values)}I", *values))


def run(mount, compiler, host, work, entry):
    size = GROUP_SIZES[entry]
    small = entry in (582, 587, 592, 600, 606)
    payload, entries = global_archive(mount)
    ((_, native, _),) = containers(decompress(payload, entries[entry], decoder()))
    (work / f"{entry}.dxbc").write_bytes(native)
    hlsl, profile, dxil = (work / f"{entry}.{suffix}" for suffix in ("hlsl", "profile", "dxil"))
    _, dxc, dxv = compiler_tools()
    subprocess.run([compiler, work / f"{entry}.dxbc", "--hlsl-out", hlsl, "--profile-out", profile], check=True)
    subprocess.run([dxc, "-E", "main", "-T", profile.read_text().strip(), "-O3", "-Fo", dxil, hlsl], check=True)
    subprocess.run([dxv, dxil], check=True, stdout=subprocess.DEVNULL)
    for case in CASES:
        cb0, cb1 = [0] * 128, [0] * 768
        cb0[64:68] = [1, 0, 8, 8]
        cb0[104:106] = [f32(1), f32(1)]
        words(work / f"{case}.b0", cb0)
        words(work / f"{case}.b1", cb1)
        # The two source tiles refer to the same 8x8 image. Its swizzled
        # coordinates make individual pixels exercise both wave boundaries.
        def lane_for_xy(x, y):
            return ((x & 1) | ((y & 1) << 1) | ((x & 2) << 1)
                    | ((y & 2) << 2) | ((x & 4) << 2) | ((y & 4) << 3))
        depths = [f32(.5 if active(case, lane_for_xy(x, y)) else 0)
                  for y in range(8) for x in range(8)]
        words(work / f"{case}.t0", depths)
        words(work / f"{case}.t1", [f32(.25 if small else 0)] * 64)
        words(work / f"{case}.t2", [0] * 16 if small else [f32(.25)] * 64)
        words(work / f"{case}.t3", [0 if case == "no_tiles" else 1 if case == "one_tile" else size // 64] * 8 if small else [0] * 16)
        if not small:
            words(work / f"{case}.t4", [0 if case == "no_tiles" else 1 if case == "one_tile" else size // 64] * 8)
        words(work / f"{case}.u0", [5] + [GUARD] * 15)
        words(work / f"{case}.u1", [GUARD] * (size * 2 + 64))
    (work / "vsm.inputs").write_text("".join(f"{case} {entry} 1 1 4 2 4 8\n" for case in CASES))
    toolchain = Toolchain()
    obj = toolchain.compile(ROOT / "tools/reference/reflection-compact/guest.cpp", work, "/W4", "/WX", f"/I{ROOT / 'runtime/d3d12'}")
    exe = work / "vsm.exe"
    toolchain.link(exe, obj, "d3d12.lib", "dxgi.lib", "kernel32.lib")
    process = host.run(exe, [], timeout=60, capture=True)
    receipt = dict(line.split("=", 1) for line in (work / "vsm.txt").read_text().splitlines())
    if process.returncode:
        raise RuntimeError(f"reflection fixture failed: {receipt}")
    results = {}
    for case in CASES:
        def read(slot):
            data = (work / f"{case}.out{slot}.bin").read_bytes()
            return list(struct.unpack(f"<{len(data) // 4}I", data))
        actual_counter, actual_records = read(0), read(1)
        records = []
        for lane in range(size):
            if active(case, lane):
                x, y = xy(lane)
                records.extend([x | (y << 16), 0x3c003400 if small else 0x34003400])
        # The guest reserves one extra entry if its last lane is inactive.
        expected_counter = 5 + len(records) // 2 + int(not active(case, size - 1))
        expected_records = [GUARD] * (size * 2 + 64)
        expected_records[10:10 + len(records)] = records
        if len(actual_records) != len(expected_records) or len(actual_counter) != 16:
            raise ValueError(f"{case}: unexpected reflection readback length")
        differences = sum(a != b for a, b in zip(actual_records, expected_records))
        differences += sum(a != b for a, b in zip(actual_counter, [expected_counter] + [GUARD] * 15))
        if differences:
            raise ValueError(f"{case}: {differences} reflection readback mismatches; counter={actual_counter[0]:#x}/{expected_counter:#x}; records={[(i,hex(a),hex(b)) for i,(a,b) in enumerate(zip(actual_records,expected_records)) if a!=b][:8]}")
        results[case] = {"selected": len(records) // 2, "counter": expected_counter,
                         "checked_words": len(actual_records) + len(actual_counter), "mismatches": differences}
    return {"gpu": receipt, "host": host.version, "source_sha256": digest(work / f"{entry}.dxbc"),
            "dxil_sha256": digest(dxil), "cases": results}


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("mount", type=Path)
    p.add_argument("--prefix", type=Path, required=True)
    p.add_argument("--proton", type=Path, required=True)
    p.add_argument("--xshader", type=Path)
    p.add_argument("--entry", type=int, choices=GROUP_SIZES, default=576)
    p.add_argument("--output", type=Path, required=True)
    args = p.parse_args()
    compiler = args.xshader.resolve() if args.xshader else compiler_tools()[0]
    with TemporaryDirectory(dir=ROOT / "build") as temp:
        result = run(args.mount, compiler, Host(None, args.proton, args.prefix), Path(temp), args.entry)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
