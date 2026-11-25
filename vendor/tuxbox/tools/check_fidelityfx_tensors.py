#!/usr/bin/env python3
"""Compare packaged FidelityFX compute passes with their native RDNA2 tensor outputs."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import Toolchain
from project_tuxbox.host import Host
from project_tuxbox.inspect import digest
from project_tuxbox.shader_prep import compiler_tools


def main():
    reference = json.loads((ROOT / "evidence/providers/fidelityfx-tensors.json").read_text())
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("dll", type=Path)
    parser.add_argument("--proton", type=Path)
    parser.add_argument("--prefix", type=Path)
    parser.add_argument("--entry", choices=reference["cases"], action="append", help="Packaged shader offset in hex")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    payload = args.dll.read_bytes()
    if hashlib.sha256(payload).hexdigest() != reference["package_sha256"]:
        parser.error("the upscaler DLL differs from the measured package")
    compiler, dxc, dxv = compiler_tools()
    host = Host(None, args.proton, args.prefix)
    results = {}
    (ROOT / "build").mkdir(exist_ok=True)
    with TemporaryDirectory(dir=ROOT / "build") as temp:
        work = Path(temp)
        toolchain = Toolchain()
        obj = toolchain.compile(ROOT / "tools/reference/fidelityfx-tensor/guest.cpp", work,
                                f"/I{ROOT / 'runtime/d3d12'}")
        guest = work / "tensor.exe"
        toolchain.link(guest, obj, "d3d12.lib", "dxgi.lib", "kernel32.lib")
        entries = list(dict.fromkeys(args.entry or reference["cases"]))
        for entry in entries:
            case = work / entry
            case.mkdir()
            original, dxil = case / "original.cso", case / "translated.cso"
            hlsl, profile = case / "tensor.hlsl", case / "profile"
            expected = reference["cases"][entry]
            offset = int(entry, 16)
            original.write_bytes(payload[offset:offset + expected["container_bytes"]])
            if digest(original) != expected["source_sha256"]:
                raise ValueError(f"shader mismatch at {entry}")
            subprocess.run([compiler, original, "--hlsl-out", hlsl, "--profile-out", profile], check=True)
            subprocess.run([dxc, "-T", profile.read_text().strip(), "-E", "main", "-Fo", dxil, hlsl], check=True)
            subprocess.run([dxv, dxil], check=True)
        run = host.run(guest, entries, capture=True, timeout=600)
        if run.returncode:
            details = {entry: status.read_text().strip() for entry in entries
                       if (status := work / entry / "result.txt").exists()}
            raise RuntimeError(f"guest exit {run.returncode}: {details}\n{run.stdout[-500:]}")
        adapters = set()
        for entry in entries:
            case = work / entry
            adapters.add((case / "result.txt").read_text().strip().removeprefix("adapter="))
            value = digest(case / "tensor.bin")
            results[entry] = {"dxil_sha256": digest(case / "translated.cso"), "tensor_sha256": value,
                              "matches_native": value == reference["cases"][entry]["tensor_sha256"]}
        if len(adapters) != 1:
            raise ValueError(f"GPU changed during tensor check: {adapters}")
        report = {"adapter": adapters.pop(), "compiler_sha256": digest(compiler), "cases": results}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    return int(any(not case["matches_native"] for case in results.values()))


if __name__ == "__main__":
    sys.exit(main())
