#!/usr/bin/env python3
"""Compare standalone HLSL node tests directly with a native gfx1036 dump.

Generate the input with the optional --dump argument to the native BVH node
control. The dump contains synthetic inputs and raw instruction outputs; no
game assets are required. This checks only the cases present in that dump.
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import struct
import subprocess
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import Toolchain, find_dxc
from project_tuxbox.host import Host


def run():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dump", type=Path, required=True)
    parser.add_argument("--proton", type=Path, required=True)
    parser.add_argument("--prefix-amd", type=Path, required=True)
    parser.add_argument("--prefix-nvidia", type=Path, required=True)
    parser.add_argument("--amd-vulkan", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--strict", action="store_true", help="compile with DXC strict IEEE math")
    parser.add_argument("--projected-rtz", action="store_true", help="experimental finite triangle control away from edges")
    parser.add_argument("--float-rtz", action="store_true", help="test experimental float32-only RTZ candidate")
    parser.add_argument("--dynamic", action="store_true", default=True,
                        help="prevent constant folding using the runtime thread ID (default)")
    parser.add_argument("--allow-constant-folding", dest="dynamic", action="store_false",
                        help="reproduce older constant-input controls; does not test dynamic GPU arithmetic")
    args = parser.parse_args()
    data = args.dump.read_bytes()
    is_triangle = data[:8] in (b"TXBVT001", b"TXBVT002", b"TXBVP001", b"TXBVF001")
    if is_triangle and len(data) == 8 + 6 * 27 * 4:
        cases = [struct.unpack_from("<27I", data, 8 + i * 27 * 4) for i in range(6)]
    elif data[:8] == b"TXBVP001" and len(data) == 8 + 16 * 27 * 4:
        cases = [struct.unpack_from("<27I", data, 8 + i * 27 * 4) for i in range(16)]
    elif data[:8] == b"TXBVF001" and len(data) == 8 + 27 * 4:
        cases = [struct.unpack_from("<27I", data, 8)]
    elif data[:8] == b"TXBVH001" and len(data) == 8 + 48 * 52 * 4:
        cases = [struct.unpack_from("<52I", data, 8 + i * 52 * 4) for i in range(48)]
    elif data[:8] == b"TXBVR001" and len(data) == 8 + 8 * 52 * 4:
        cases = [struct.unpack_from("<52I", data, 8 + i * 52 * 4) for i in range(8)]
    else:
        raise ValueError("invalid native oracle dump")
    if args.projected_rtz and not is_triangle:
        raise ValueError("projected RTZ control requires triangle input")
    if args.float_rtz and not args.projected_rtz:
        raise ValueError("float-only RTZ requires projected RTZ mode")
    if args.projected_rtz and not args.strict:
        raise ValueError("projected RTZ control requires strict IEEE compilation")
    if args.projected_rtz:
        for i, case in enumerate(cases):
            if any(word & 0x7f800000 == 0x7f800000 for word in (*case[:12], *case[16:22], *case[24:27])):
                raise ValueError(f"projected RTZ case {i} contains nonfinite inputs or outputs")
            if case[24] & 0x7fffffff == 0:
                raise ValueError(f"projected RTZ case {i} has a zero native determinant")
            if all(word & 0x7fffffff == 0 for word in case[19:22]):
                raise ValueError(f"projected RTZ case {i} has a zero ray direction")
            values = struct.unpack("<4f", struct.pack("<4I", *case[23:27]))
            if math.isnan(values[0]):
                raise ValueError(f"projected RTZ case {i} has a NaN native numerator")
            t, u, v = (values[j] / values[1] for j in (0, 2, 3))
            if u == 0 or v == 0 or u + v == 1:
                raise ValueError(f"projected RTZ case {i} is on a triangle edge")
            if math.isfinite(t) and (t < 0 or u < 0 or v < 0 or u + v > 1):
                raise ValueError(f"projected RTZ case {i} is not an interior hit")
    shader = (ROOT / "tools/reference/bvh-node/node.hlsl").read_text()
    if args.projected_rtz:
        shader += "\n" + (ROOT / "tools/reference/bvh-node/rtz_triangle.hlsl").read_text()
    shader += "\nRWBuffer<uint> output : register(u0);\n[numthreads(1,1,1)] void main(uint3 tid : SV_DispatchThreadID) {\n"
    def raw(word):
        return f"(0x{word:08x}u ^ tid.x)" if args.dynamic else f"0x{word:08x}u"
    def float_word(word):
        return f"asfloat({raw(word)})"
    for i, case in enumerate(cases):
        if is_triangle:
            node = ", ".join(raw(word) for word in (*case[:16], *([0] * 16)))
            origin = ", ".join(float_word(word) for word in case[16:19])
            direction = ", ".join(float_word(word) for word in case[19:22])
            if case[22] not in (0, 1):
                raise ValueError(f"invalid triangle side in case {i}")
            if args.projected_rtz:
                shader += f"{{ uint node[32] = {{ {node} }}; float4 r = tb_bvh_triangle_rtz(node, {case[22]}u, float3({origin}), float3({direction}));"
            else:
                shader += f"{{ uint node[32] = {{ {node} }}; float4 r = tb_bvh_paired_triangle(node, {case[22]}u, float3({origin}), float3({direction}));"
            shader += "".join(f" output[{4*i+j}] = asuint(r[{j}]);" for j in range(4)) + " }\n"
        else:
            node = ", ".join(raw(word) for word in case[:32])
            origin = ", ".join(float_word(word) for word in case[32:35])
            inverse = ", ".join(float_word(word) for word in case[35:38])
            extent = float_word(case[38])
            if case[39] not in (0, 1) or case[40] not in (0, 1):
                raise ValueError(f"invalid descriptor in case {i}")
            half = "false" if case[40] else "true"
            sort = "true" if case[39] else "false"
            shader += f"{{ uint node[32] = {{ {node} }}; uint4 r = tb_bvh_boxes(node, {half}, float3({origin}), float3({inverse}), {extent}, {sort});"
            shader += "".join(f" output[{4*i+j}] = r[{j}];" for j in range(4)) + " }\n"
    shader += "}\n"
    scope = ("serialized BLAS paired triangle" if data[:8] == b"TXBVT002" else
             "synthetic fractional-direction triangle" if data[:8] == b"TXBVF001" else
             "synthetic triangle precision" if data[:8] == b"TXBVP001" else
             "synthetic paired triangle") if is_triangle else (
        "serialized BLAS root" if data[:8] == b"TXBVR001" else "synthetic BOX16/BOX32")
    report = {"scope": f"{scope} cases: native gfx1036 instruction output versus HLSL, not a general opcode equivalence proof",
              "projected_rtz": args.projected_rtz, "float_rtz": args.float_rtz, "dynamic": args.dynamic,
              "cases": len(cases), "native_dump_sha256": hashlib.sha256(data).hexdigest(),
              "hlsl_sha256": hashlib.sha256(shader.encode()).hexdigest(), "adapters": {}}
    with TemporaryDirectory(prefix="tuxbox-bvh-native-diff-") as directory:
        work = Path(directory)
        hlsl, cso = work / "node.hlsl", work / "node.cso"
        hlsl.write_text(shader)
        dxc = Path(find_dxc(None))
        dxc_args = [dxc, "-T", "cs_6_0", "-E", "main", hlsl, "-Fo", cso]
        if args.strict:
            dxc_args.append("-Gis")
        if args.float_rtz:
            dxc_args += ["-D", "TUXBOX_RTZ_FLOAT32=1"]
        subprocess.run(dxc_args, check=True)
        subprocess.run([dxc.with_name("dxv"), cso], check=True)
        report["dxil_sha256"] = hashlib.sha256(cso.read_bytes()).hexdigest()
        toolchain = Toolchain()
        obj = toolchain.compile(ROOT / "tools/reference/independent-shaders/compute.cpp",
                                work, f"/I{ROOT / 'runtime/d3d12'}")
        exe = work / "node.exe"
        toolchain.link(exe, obj, "d3d12.lib", "dxgi.lib", "kernel32.lib")
        (work / "cases.txt").write_text("node 0 uint 1\n")
        for name, prefix in (("AMD", args.prefix_amd), ("NVIDIA", args.prefix_nvidia)):
            os.environ.update(VKD3D_FILTER_DEVICE_NAME=name, DXVK_FILTER_DEVICE_NAME=name)
            if name == "AMD" and args.amd_vulkan:
                os.environ["VK_DRIVER_FILES"] = str(args.amd_vulkan)
            elif name == "NVIDIA":
                os.environ.pop("VK_DRIVER_FILES", None)
            result = Host(None, str(args.proton), prefix).run(exe, [], timeout=45, capture=True)
            if result.returncode:
                raise RuntimeError(f"{name} GPU control failed: {result.returncode}")
            receipt = (work / "result.txt").read_text()
            adapter = next((line.removeprefix("adapter=") for line in receipt.splitlines()
                            if line.startswith("adapter=")), None)
            if adapter is None or not adapter.lower().startswith("1002:" if name == "AMD" else "10de:"):
                raise ValueError(f"{name} selected unexpected adapter {adapter!r}")
            raw = (work / "node.bin").read_bytes()
            actual = struct.unpack(f"<{len(raw)//4}I", raw)
            start, end = 96, 96 + 4 * len(cases)
            mismatches = []
            for i, case in enumerate(cases):
                got = actual[start + 4*i:start + 4*i + 4]
                want = case[23:27] if is_triangle else case[48:52]
                if got != want:
                    mismatches.append({"case": i, "actual": [f"{v:08x}" for v in got],
                                       "native": [f"{v:08x}" for v in want]})
            if any(value != 0x76540000 + i for i, value in enumerate(actual) if i < start or i >= end):
                raise ValueError(f"{name} output guard changed")
            report["adapters"][name] = {"adapter": adapter, "cases": len(cases),
                                         "guard_words": len(actual) - 4 * len(cases),
                                         "mismatches": mismatches}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))
    if any(value["mismatches"] for value in report["adapters"].values()):
        raise SystemExit(1)


if __name__ == "__main__":
    run()
