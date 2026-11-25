#!/usr/bin/env python3
"""Read back three translated Frosty vertex shaders with padded, split and interleaved inputs."""
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
from build import find_dxc
from frosty_gpu_guest import run_gpu_guest
from project_tuxbox.inspect import digest
from project_tuxbox.shader_prep import FROSTY_PLAN, cooked_shader, run_tool, shader_compiler

ENTRIES = (1007, 2450, 2751)
GUEST = ROOT / "tools/reference/independent-shaders/vertex-fetch.cpp"
# DrawInstanced starts at vertex 1 and consumes three records. The end records catch a bad base.
VERTICES = (
    ((-8, -7, -6, -5), (-4, -3)),
    ((0.125, -0.25, 0.375, 0.875), (-1.125, 2.25)),
    ((0.75, 0.0625, -0.5, 1.25), (3.5, -4.75)),
    ((-0.375, 0.625, 0.25, 1.375), (5.125, -6.25)),
    ((8, 7, 6, 5), (4, 3)),
)


def make_inputs(trial):
    for layout, strides in (("interleaved", (48,)), ("split", (24, 32))):
        views = [bytearray(b"\x55" * (len(VERTICES) * stride)) for stride in strides]
        for index, (attr0, attr1) in enumerate(VERTICES):
            struct.pack_into("<2f", views[0], index * strides[0] + 4, *attr1)
            offset = 8 if layout == "split" else 16
            struct.pack_into("<4f", views[-1], index * strides[-1] + offset, *attr0)
        for slot, view in enumerate(views):
            (trial / f"{layout}{slot}.vb").write_bytes(view)


def expected_readback(entry):
    # From the Xbox exports: 1007 replaces W, 2450 passes two attributes,
    # and 2751 uses two FMAs on ATTRIBUTE0.xy to move from UV to clip space.
    expected = bytearray(b"\xcd" * 128)
    cursor = 16
    for attr0, attr1 in VERTICES[1:4]:
        if entry == 1007:
            values = (*attr0[:3], 1)
        elif entry == 2450:
            values = (*attr0, *attr1)
        elif entry == 2751:
            x, y = attr0[:2]
            values = (2 * x - 1, 1 - 2 * y, 0, 1)
        else:
            raise ValueError(f"no expected output for entry {entry}")
        payload = struct.pack(f"<{len(values)}f", *values)
        expected[cursor:cursor + len(payload)] = payload
        cursor += len(payload)
    struct.pack_into("<Q", expected, 112, cursor - 16)
    return bytes(expected)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--stage", required=True, type=Path, help="prepared Frosty layout with Mount and prefix")
    parser.add_argument("--proton", type=Path, help="defaults to the staged Proton installation")
    parser.add_argument("--compiler", type=Path, help="defaults to the current TuxBox host shader compiler")
    parser.add_argument("--adapter", required=True, help="expected vendor:device PCI ID")
    parser.add_argument("--work", required=True, type=Path, help="directory for result.json and gpu.log")
    args = parser.parse_args()
    stage, work = args.stage.resolve(), args.work.resolve()
    proton = (args.proton or Path((stage / "proton").read_text().strip())).resolve()
    xshader = (args.compiler or shader_compiler()).resolve()
    dxc = Path(find_dxc(None)).resolve()
    dxv = dxc.with_name("dxv")
    plan = json.loads(FROSTY_PLAN.read_text())
    records = {r["entry"]: r for r in [*plan["shaders"], *plan["source_cache"]["additional_shaders"]]
               if r["archive"] == "global" and r["entry"] in ENTRIES}
    if records.keys() != set(ENTRIES):
        raise ValueError("the preparation profile lacks a requested vertex shader")

    work.mkdir(parents=True, exist_ok=True)
    with TemporaryDirectory(dir=work) as tmp:
        trial = Path(tmp)
        make_inputs(trial)
        shaders = {}
        for entry, record in records.items():
            source = trial / f"{entry}.dxbc"
            source.write_bytes(cooked_shader(stage / "Mount/Frosty/Content/Paks/Frosty-XSX.pak", record))
            hlsl, profile, dxil = (source.with_suffix(suffix) for suffix in (".hlsl", ".profile", ".cso"))
            run_tool([xshader, source, "--hlsl-out", hlsl, "--profile-out", profile])
            if profile.read_text().strip() != record["profile"]:
                raise ValueError(f"unexpected output profile for global/{entry}")
            run_tool([dxc, "-T", record["profile"], "-E", "main", "-Fo", dxil, hlsl])
            run_tool([dxv, dxil])
            shaders[entry] = dict(source_sha256=digest(source), hlsl_sha256=digest(hlsl), dxil_sha256=digest(dxil))
        guest_lib = stage / "Mount/Frosty/Binaries/XSX"
        exe, status, observed = run_gpu_guest(GUEST, "vertex-fetch.exe", trial, work, proton, stage / "prefix",
                                              guest_lib / "d3d12_xs.dll", guest_lib)
        draws = []
        for entry in ENTRIES:
            expected = expected_readback(entry)
            for layout in ("interleaved", "split"):
                path = trial / f"{entry}-{layout}.bin"
                actual = path.read_bytes() if path.is_file() else b""
                row = dict(entry=f"global/{entry}", layout=layout, matched=actual == expected,
                           expected_sha256=hashlib.sha256(expected).hexdigest(),
                           readback_sha256=hashlib.sha256(actual).hexdigest(), bytes=len(actual))
                if actual != expected:
                    row.update(actual_hex=actual.hex(), expected_hex=expected.hex())
                draws.append(row)
        passed = status == 0 and observed.get("adapter") == args.adapter and all(row["matched"] for row in draws)
        result = dict(passed=passed, guest_exit=status, guest_result=observed, expected_adapter=args.adapter,
                      proton=(proton / "version").read_text().strip(), compiler_sha256=digest(xshader),
                      dxc_sha256=digest(dxc), dxv_sha256=digest(dxv), guest_sha256=digest(exe),
                      d3d12_dlls={lib: digest(trial / lib) for lib in ("d3d12.dll", "d3d12core.dll")},
                      vkd3d_config=os.environ.get("VKD3D_CONFIG", ""), shaders=shaders, draws=draws)
        (work / "result.json").write_text(json.dumps(result, indent=2) + "\n")
    print(f"{sum(d['matched'] for d in draws)}/{len(draws)} draws matched on {observed.get('adapter')}. {work / 'result.json'}")
    if not passed:
        raise SystemExit(observed.get("error", "GPU result mismatch"))


if __name__ == "__main__":
    main()
