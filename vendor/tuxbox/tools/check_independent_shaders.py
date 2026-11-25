#!/usr/bin/env python3
"""Compile retail shaders and compare offscreen GPU readbacks with their expected words and pixels."""
import argparse
from collections import Counter
import hashlib
import json
import os
from pathlib import Path
import re
import struct
import subprocess
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import find_dxc
from frosty_gpu_guest import run_gpu_guest
from project_tuxbox.inspect import digest
from project_tuxbox.shader_inventory import ARCHIVES, archive_entries, decoder

SOURCES = ROOT / "tools/reference/independent-shaders"


def run_tool(command):
    run = subprocess.run(command, capture_output=True, text=True)
    if run.returncode:
        raise ValueError(f"{command[0]} failed on {command[-1]}: {run.stdout}{run.stderr}")
    return run.stdout


def vertex_input(dump):
    inputs = dump.split("; Input signature:", 1)[1].split("; Output signature:", 1)[0]
    columns = re.compile(r"^; (\S+)\s+(\d+)\s+([xyzw]+)\s+(\d+)\s+(\S+)\s+(\S+)\s*", re.M)
    fields = []
    # DXC can print semantics in declaration order even when packed in another lane order.
    rows = sorted(columns.findall(inputs), key=lambda row: (int(row[3]), "xyzw".index(row[2][0])))
    for name, index, mask, _, sysval, scalar in rows:
        if sysval == "POS":
            fields.append("float4 pos : SV_Position;")
        if sysval != "NONE":
            continue
        qualifier = "nointerpolation " if scalar in ("int", "uint") else ""
        if scalar not in ("int", "uint", "float"):
            raise ValueError(f"unsupported input type {scalar}")
        typename = scalar + (str(len(mask)) if len(mask) > 1 else "")
        fields.append(f"{qualifier}{typename} f{len(fields)} : {name}{index};")
    if "float4 pos : SV_Position;" not in fields:
        fields.append("float4 pos : SV_Position;")
    return "\n".join(["struct V {", *fields, "};",
                      "V main(uint id : SV_VertexID) { V v = (V)0;",
                      "v.pos = float4(id == 2 ? 3 : -1, id == 1 ? 3 : -1, 0.375, 1); return v; }"])


def compile_cases(trial, cases, native, compiler, dxc):
    lines, draws, cso_hashes = [], [], {}
    for i, case in enumerate(cases):
        key = case["source_sha256"]
        source = trial / "source.dxbc"
        source.write_bytes(native[key])
        name = f"s{i}"
        hlsl = trial / "out.hlsl"
        profile = trial / "profile.txt"
        run_tool([compiler, source, "--hlsl-out", hlsl, "--profile-out", profile])
        blob = trial / f"{name}.cso"
        run_tool([dxc, "-T", profile.read_text().strip(), "-E", "main", "-Fo", blob, hlsl])
        run_tool([dxc.with_name("dxv"), blob])
        cso_hashes[key] = digest(blob)
        if case["mode"] == "compute":
            for view in (["structured"] if case["stride"] else ["uint", "float"]):
                shader = name + view
                (trial / f"{shader}.cso").hardlink_to(blob)
                lines.append(f"{shader} {case['stride']} {view} {case.get('groups', 1)}")
                draws.append((shader, case, view))
        else:
            vs = vertex_input(run_tool([dxc, "-dumpbin", blob]))
            vertex = "v" + hashlib.sha256(vs.encode()).hexdigest()[:16]
            vertex_blob = trial / f"{vertex}.cso"
            if not vertex_blob.exists():
                hlsl.write_text(vs)
                run_tool([dxc, "-T", "vs_6_0", "-E", "main", "-Fo", vertex_blob, hlsl])
                run_tool([dxc.with_name("dxv"), vertex_blob])
            widths = [len(value) for value in case["targets"]]
            lines.append(" ".join(map(str, [name, vertex, len(widths), *widths])))
            draws.append((name, case, "raster"))
    (trial / "cases.txt").write_text("\n".join(lines) + "\n")
    return draws, cso_hashes


def expected_readback(case):
    if case["mode"] == "compute":
        words = [0x76540000 + index for index in range(384)]
        start = 96 + case["first_word"]
        values = case["words"] * case.get("repeat", 1)
        words[start:start + len(values)] = values
        return struct.pack("<384I", *words)
    output = bytearray()
    for plane, color in enumerate([*case["targets"], [0.375]]):
        depth = plane == len(case["targets"])
        guard = [0.875] if depth else [0.25, -0.5, 0.75, 1.5][:len(color)]
        for y in range(16):
            for x in range(16):
                covered = 3 <= x < 12 and 2 <= y < 11 and case["mode"] != "discard"
                value = color if covered else guard
                output.extend(struct.pack(f"<{len(value)}f", *value))
            output.extend(bytes(256 - 16 * len(color) * 4))
    return bytes(output)


def validate(trial, draws):
    results = []
    for name, case, view in draws:
        path = trial / f"{name}.bin"
        actual = path.read_bytes() if path.exists() else b""
        expected = expected_readback(case)
        row = dict(source_sha256=case["source_sha256"], entry=case["entry"], view=view,
                   readback_sha256=hashlib.sha256(actual).hexdigest(), matched=actual == expected)
        if actual != expected:
            common = min(len(actual), len(expected)) // 4
            row["different_or_missing_words"] = abs(len(actual) - len(expected)) // 4 + sum(
                actual[i * 4:i * 4 + 4] != expected[i * 4:i * 4 + 4] for i in range(common))
            row["actual_bytes"] = len(actual)
            row["expected_bytes"] = len(expected)
        results.append(row)
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("stage", "compiler", "cases", "work", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--adapter", required=True, help="PCI vendor:device ID to verify in the guest")
    args = parser.parse_args()
    stage, work, compiler = args.stage.resolve(), args.work.resolve(), args.compiler.resolve()
    proton = Path((stage / "proton").read_text().strip())
    cases = json.loads(args.cases.read_text())
    modes = {c["mode"] == "compute" for c in cases}
    if len(modes) != 1 or len({c["source_sha256"] for c in cases}) != len(cases):
        parser.error("use distinct programs from a single shader stage in each run")
    native = {}
    wanted = {c["source_sha256"] for c in cases}

    def collect(key, blob):
        if key in wanted:
            native[key] = blob

    labels = {c["entry"]: c["source_sha256"] for c in cases}
    found = set()
    with (stage / "Mount/Frosty/Content/Paks/Frosty-XSX.pak").open("rb") as stream:
        decode = decoder()
        for archive in ARCHIVES:
            for entry in archive_entries(archive, stream, decode, collect, {}):
                label = f"{entry.archive}/{entry.index}"
                if label in labels and labels[label] == entry.source:
                    found.add(label)
    if native.keys() != wanted or found != labels.keys():
        raise ValueError("one or more requested shader hashes are absent from the archives")
    dxc = Path(find_dxc(None)).resolve()
    work.mkdir(parents=True, exist_ok=True)
    guest = SOURCES / ("compute.cpp" if modes.pop() else "pixel.cpp")
    with TemporaryDirectory(dir=work) as tmp:
        trial = Path(tmp)
        draws, shaders = compile_cases(trial, cases, native, compiler, dxc)
        xsx = stage / "Mount/Frosty/Binaries/XSX"
        exe, status, fields = run_gpu_guest(guest, "shader-control.exe", trial, work, proton,
                                            stage / "prefix", xsx / "d3d12_xs.dll", xsx)
        rows = validate(trial, draws)
        if fields.get("error"):
            print(fields["error"], file=sys.stderr)
        passed = status == 0 and fields.get("adapter") == args.adapter and all(r["matched"] for r in rows)
        adapters = [s.split("Found device: ", 1)[1] for s in (work / "gpu.log").read_text().splitlines()
                    if "Found device: " in s]
        result = dict(passed=passed, guest_exit=status, adapter=fields.get("adapter"), devices=adapters,
                      proton=(proton / "version").read_text().strip(), cases_sha256=digest(args.cases),
                      compiler_sha256=digest(compiler), dxc_sha256=digest(dxc), dxv_sha256=digest(dxc.with_name("dxv")),
                      guest_sha256=digest(exe), d3d12_dlls={name: digest(trial / name) for name in ("d3d12.dll", "d3d12core.dll")},
                      config=os.getenv("VKD3D_CONFIG", ""), disabled_extensions=os.getenv("VKD3D_DISABLE_EXTENSIONS", ""),
                      counts=dict(Counter(c["mode"] for c in cases)), dxil=shaders, draws=rows)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(result, indent=2) + "\n")
    if not passed:
        raise SystemExit(f"GPU result mismatch; see {args.output}")


if __name__ == "__main__":
    main()
