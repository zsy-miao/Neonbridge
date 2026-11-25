#!/usr/bin/env python3
"""Time a synthetic dynamic float32 triangle workload after native comparison.

The control generates 65,536 native gfx1036 results and checks every GPU output
word before recording timestamps. It is not a game benchmark or a general
accuracy proof. Requires the native HSA runtime and a gfx1036 agent.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import Toolchain, find_dxc
from project_tuxbox.host import Host


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--hsa-root", type=Path, default=Path("/opt/rocm"))
    parser.add_argument("--proton", type=Path, required=True)
    parser.add_argument("--prefix-amd", type=Path, required=True)
    parser.add_argument("--prefix-nvidia", type=Path, required=True)
    parser.add_argument("--amd-vulkan", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--wide-fallback", action="store_true", help="test experimental exceptional-range fallback")
    args = parser.parse_args()
    source = ROOT / "tools/reference/bvh-node/perf"
    with TemporaryDirectory(prefix="tuxbox-bvh-perf-") as directory:
        work = Path(directory)
        native_obj, native_code, native_host = (work / "native.o", work / "native.hsaco", work / "native-host")
        subprocess.run(["clang", "-target", "amdgcn-amd-amdhsa", "-mcpu=gfx1036", "-nogpulib",
                        "-x", "cl", "-cl-std=CL2.0", "-O2", "-c", source / "native.cl", "-o", native_obj], check=True)
        subprocess.run(["ld.lld", "-shared", native_obj, "-o", native_code], check=True)
        subprocess.run(["clang", "-Wall", "-Wextra", "-Wno-unused-parameter", "-I" + str(args.hsa_root / "include"),
                        source / "native.c", "-L" + str(args.hsa_root / "lib"), "-lhsa-runtime64",
                        "-lrocprofiler-register", "-o", native_host], check=True)
        env = os.environ.copy()
        env["LD_LIBRARY_PATH"] = str(args.hsa_root / "lib") + ":" + env.get("LD_LIBRARY_PATH", "")
        native = subprocess.run([native_host, native_code, work / "native.bin"], check=True,
                                capture_output=True, text=True, env=env)
        dxc = Path(find_dxc(None))
        cso = work / "f32.cso"
        command = [dxc, "-T", "cs_6_0", "-E", "main", "-Gis", "-D", "TUXBOX_RTZ_FLOAT32=1",
                   source / "compute.hlsl", "-Fo", cso]
        if args.wide_fallback:
            command += ["-D", "TUXBOX_RTZ_WIDE_FALLBACK=1"]
        subprocess.run(command, check=True)
        subprocess.run([dxc.with_name("dxv"), cso], check=True)
        toolchain = Toolchain()
        obj = toolchain.compile(source / "guest.cpp", work,
                                f"/I{ROOT / 'tools/reference/independent-shaders'}",
                                f"/I{ROOT / 'runtime/d3d12'}")
        exe = work / "perf.exe"
        toolchain.link(exe, obj, "d3d12.lib", "dxgi.lib", "kernel32.lib")
        report = {"scope": "Synthetic single triangle, 65536 dynamic rays with 1024 distinct origins; strict float32 candidate, GPU timestamps include UAV barriers. Not a game benchmark or a general accuracy proof.",
                  "wide_fallback": args.wide_fallback, "native_agent": native.stdout.strip(), "native_sha256": hashlib.sha256((work / "native.bin").read_bytes()).hexdigest(),
                  "dxil_sha256": hashlib.sha256(cso.read_bytes()).hexdigest(), "adapters": {}}
        for name, prefix in (("AMD", args.prefix_amd), ("NVIDIA", args.prefix_nvidia)):
            os.environ.update(VKD3D_FILTER_DEVICE_NAME=name, DXVK_FILTER_DEVICE_NAME=name)
            if name == "AMD" and args.amd_vulkan:
                os.environ["VK_DRIVER_FILES"] = str(args.amd_vulkan)
            elif name == "NVIDIA":
                os.environ.pop("VK_DRIVER_FILES", None)
            samples = []
            for _ in range(3):
                result = Host(None, str(args.proton), prefix).run(exe, [], timeout=60, capture=True)
                receipt = dict(line.split("=", 1) for line in (work / "result.txt").read_text().splitlines() if "=" in line)
                if result.returncode:
                    raise RuntimeError(f"{name} performance control failed: {receipt}")
                if not receipt.get("adapter", "").lower().startswith("1002:" if name == "AMD" else "10de:"):
                    raise ValueError(f"unexpected {name} adapter {receipt.get('adapter')!r}")
                if receipt.get("mismatched_words") != "0" or receipt.get("threads") != "65536":
                    raise ValueError(f"{name} output not equivalent to native: {receipt}")
                samples.append(float(receipt["gpu_ms"]))
            report["adapters"][name] = {"adapter": receipt["adapter"], "runs_ms_per_dispatch": samples,
                                        "median_ms_per_dispatch": statistics.median(samples),
                                        "threads_per_dispatch": 65536, "native_mismatched_words": 0}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
