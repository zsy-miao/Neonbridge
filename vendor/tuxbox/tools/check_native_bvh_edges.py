#!/usr/bin/env python3
"""Compare dynamic RTZ triangle edge results with native gfx1036 outputs.

Consumes the synthetic `--dump-edges` output of the native node control. Six
records at a time are read by GPU lanes from a seeded UAV, avoiding constant
folding. Mismatches are reported and cause a nonzero exit status.
"""
import argparse
import hashlib
import json
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


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--dump", type=Path, required=True)
    p.add_argument("--proton", type=Path, required=True)
    p.add_argument("--prefix-amd", type=Path, required=True)
    p.add_argument("--prefix-nvidia", type=Path, required=True)
    p.add_argument("--amd-vulkan", type=Path)
    p.add_argument("--output", type=Path, required=True)
    p.add_argument("--offset", type=int, default=0)
    p.add_argument("--limit", type=int)
    p.add_argument("--wide-fallback", action="store_true", help="test experimental exceptional-range fallback")
    args = p.parse_args()
    raw = args.dump.read_bytes()
    if raw[:8] not in (b"TXBVE001", b"TXBVD001", b"TXBVS001", b"TXBVM001", b"TXBVU001", b"TXBVT002") or (len(raw) - 8) % (27 * 4):
        raise ValueError("invalid native edge dump")
    total = (len(raw) - 8) // (27 * 4)
    stop = min(total, args.offset + args.limit) if args.limit is not None else total
    if args.offset < 0 or args.offset >= stop:
        raise ValueError("empty or invalid case range")
    cases = [struct.unpack_from("<27I", raw, 8 + i * 27 * 4) for i in range(args.offset, stop)]
    source = (ROOT / "tools/reference/bvh-node/rtz_triangle.hlsl").read_text()
    source += '''\nRWBuffer<uint> data : register(u0);
[numthreads(8,1,1)] void main(uint3 tid : SV_DispatchThreadID) {
    uint id=tid.x; if (id>=data[190]) return; uint base=id*27;
    uint node[32]; [unroll] for (uint i=0;i<16;i++) node[i]=data[base+i];
    [unroll] for (uint i=16;i<32;i++) node[i]=0;
    float3 origin=asfloat(uint3(data[base+16],data[base+17],data[base+18]));
    float3 direction=asfloat(uint3(data[base+19],data[base+20],data[base+21]));
    float4 result=tb_bvh_triangle_rtz(node,data[base+22],origin,direction);
    [unroll] for(uint i=0;i<4;i++) data[162+id*4+i]=asuint(result[i]);
}\n'''
    scope = {b"TXBVT002": "serialized-leaf edges", b"TXBVD001": "parallel and degenerate triangles",
             b"TXBVS001": "extreme triangle scales", b"TXBVM001": "mixed-exponent triangle scales",
             b"TXBVU001": "underflow-boundary triangle scales",
             b"TXBVE001": "synthetic triangle edges"}[raw[:8]]
    report = {"scope": f"Dynamic seeded-UAV {scope}: native gfx1036 versus float32 RTZ candidate; no general equivalence claim",
              "native_dump_sha256": hashlib.sha256(raw).hexdigest(), "offset": args.offset,
              "wide_fallback": args.wide_fallback, "cases": len(cases),
              "hlsl_sha256": hashlib.sha256(source.encode()).hexdigest(), "adapters": {}}
    with TemporaryDirectory(prefix="tuxbox-bvh-edges-") as directory:
        root = Path(directory)
        hlsl, cso = root / "edge.hlsl", root / "edge.cso"
        hlsl.write_text(source)
        dxc = Path(find_dxc(None))
        command = [dxc, "-T", "cs_6_0", "-E", "main", "-Gis", "-D", "TUXBOX_RTZ_FLOAT32=1",
                   hlsl, "-Fo", cso]
        if args.wide_fallback:
            command += ["-D", "TUXBOX_RTZ_WIDE_FALLBACK=1"]
        subprocess.run(command, check=True)
        subprocess.run([dxc.with_name("dxv"), cso], check=True)
        report["dxil_sha256"] = hashlib.sha256(cso.read_bytes()).hexdigest()
        toolchain = Toolchain()
        obj = toolchain.compile(ROOT / "tools/reference/independent-shaders/compute.cpp", root,
                                f"/I{ROOT / 'runtime/d3d12'}")
        exe = root / "edge.exe"
        toolchain.link(exe, obj, "d3d12.lib", "dxgi.lib", "kernel32.lib")
        chunks = [cases[i:i+6] for i in range(0, len(cases), 6)]
        seeds = []
        for batch, chunk in enumerate(chunks):
            seed = [0x76540000 + i for i in range(384)]
            for i, case in enumerate(chunk):
                seed[96+i*27:96+(i+1)*27] = case
            seed[96+190] = len(chunk)
            seeds.append(seed)
            (root / f"edge{batch}.seed").write_bytes(struct.pack("<384I", *seed))
            (root / f"edge{batch}.cso").write_bytes(cso.read_bytes())
        (root / "cases.txt").write_text("".join(f"edge{i} 0 uint 1\n" for i in range(len(chunks))))
        for name, prefix in (("AMD", args.prefix_amd), ("NVIDIA", args.prefix_nvidia)):
            os.environ.update(VKD3D_FILTER_DEVICE_NAME=name, DXVK_FILTER_DEVICE_NAME=name)
            if name == "AMD" and args.amd_vulkan:
                os.environ["VK_DRIVER_FILES"] = str(args.amd_vulkan)
            elif name == "NVIDIA":
                os.environ.pop("VK_DRIVER_FILES", None)
            result = Host(None, str(args.proton), prefix).run(exe, [], timeout=180, capture=True)
            if result.returncode:
                raise RuntimeError(f"{name} GPU edge control failed: {result.returncode}")
            receipt = (root / "result.txt").read_text()
            adapter = next((line.removeprefix("adapter=") for line in receipt.splitlines()
                            if line.startswith("adapter=")), None)
            if adapter is None or not adapter.lower().startswith("1002:" if name == "AMD" else "10de:"):
                raise ValueError(f"unexpected {name} adapter {adapter!r}")
            mismatches = []
            for batch, chunk in enumerate(chunks):
                got = struct.unpack("<384I", (root / f"edge{batch}.bin").read_bytes())
                start, end = 96 + 162, 96 + 162 + len(chunk) * 4
                for index, case in enumerate(chunk):
                    actual = got[start+4*index:start+4*index+4]
                    expected = case[23:27]
                    if actual != expected:
                        mismatches.append({"case": args.offset+batch*6+index,
                                           "actual": [f"{v:08x}" for v in actual],
                                           "native": [f"{v:08x}" for v in expected]})
                if any(value != seeds[batch][i] for i, value in enumerate(got) if not start <= i < end):
                    raise ValueError(f"{name} batch {batch} input or guard changed")
            report["adapters"][name] = {"adapter": adapter, "cases": len(cases),
                                        "mismatches": mismatches}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(" ".join(f"{name}={len(item['mismatches'])}/{len(cases)}" for name, item in report["adapters"].items()))
    if any(item["mismatches"] for item in report["adapters"].values()):
        raise SystemExit(1)


if __name__ == "__main__":
    main()
