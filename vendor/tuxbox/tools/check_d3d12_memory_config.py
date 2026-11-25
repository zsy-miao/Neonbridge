#!/usr/bin/env python3
"""Compare the verified Xbox D3D12 virtual-memory validation and initial getter.

The provider also exercises first and repeated VM configuration on the host. The original DLL is used only for this local control.
"""
import argparse
import json
from pathlib import Path
import shutil
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import Toolchain
from project_tuxbox.host import Host
from project_tuxbox.inspect import digest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--reference", type=Path, required=True)
    parser.add_argument("--xmem", type=Path, required=True)
    parser.add_argument("--provider", type=Path, required=True)
    parser.add_argument("--proton", type=Path)
    parser.add_argument("--prefix", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    host = Host(None, args.proton, args.prefix)
    host.env["WINEDEBUG"] = "-all"
    with TemporaryDirectory(prefix="tuxbox-d3d12-memory-") as temp:
        root = Path(temp)
        tools = Toolchain()
        obj = tools.compile(ROOT / "runtime/d3d12/memory_config_guest.cpp", root, "/W4", "/WX")
        exe = root / "memory-config.exe"
        tools.link(exe, obj, "kernel32.lib")
        guest_hash = digest(exe)
        runs = {}
        for name, source in (("reference", args.reference), ("provider", args.provider)):
            dest = root / name
            dest.mkdir()
            shutil.copyfile(exe, dest / exe.name)
            shutil.copyfile(source, dest / "d3d12_xs.dll")
            shutil.copyfile(args.xmem, dest / "xmem.dll")
            result = host.run(dest / exe.name, ["--" + name, "result.txt"], timeout=90, capture=True)
            receipt = dest / "result.txt"
            if result.returncode or not receipt.is_file():
                raise RuntimeError(f"{name} control failed ({result.returncode}): {result.stdout.decode(errors='replace')[-2000:]}")
            runs[name] = dict(line.split("=", 1) for line in receipt.read_text().splitlines())
    shared = ("initial", "null", "invalid_flags", "invalid_count", "after_invalid", "outputs_unchanged")
    if any(runs["reference"][key] != runs["provider"][key] for key in shared):
        raise ValueError("Reference and provider validation results differ")
    result = {"scope": "Initial framebuffer-preservation size and invalid virtual-memory configuration. TuxBox additionally checks first configuration with zero preservation, repeat configuration and the unavailable framebuffer allocation path.",
              "layout_basis": "Series xgskmd10 ConfigureVirtualMemory and MmConfigureGraphicsPtes from Frosty era.vbi. The default uses 5120 pages. Each input unit requests 1024 pages. The kernel fits 2056-byte directory/leaf groups, rounds down to 8 groups and caps the VA range at 1 TiB.",
              "reference_sha256": digest(args.reference), "provider_sha256": digest(args.provider),
              "xmem_sha256": digest(args.xmem), "guest_sha256": guest_hash, "host": host.version, "runs": runs}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print("D3D12 memory configuration controls passed")


if __name__ == "__main__":
    main()
