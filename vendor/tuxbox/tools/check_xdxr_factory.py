#!/usr/bin/env python3
"""Measure XDXR factory identity and allocator behavior against a local Xbox DXC DLL.

This inspects COM construction only. It does not validate any ray-compilation
method or provide an XDXR implementation. The supplied DLL is not redistributed.
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
    parser.add_argument("--proton", type=Path)
    parser.add_argument("--prefix", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    host = Host(None, args.proton, args.prefix)
    host.env["WINEDEBUG"] = "-all"
    with TemporaryDirectory(prefix="tuxbox-xdxr-factory-") as temp:
        work = Path(temp)
        toolchain = Toolchain()
        obj = toolchain.compile(ROOT / "tools/reference/xdxr-factory/guest.cpp", work, "/W4", "/WX")
        exe = work / "xdxr-factory.exe"
        toolchain.link(exe, obj, "kernel32.lib")
        shutil.copyfile(args.reference, work / "dxcompiler_xs.dll")
        run = host.run(exe, ["result.txt"], timeout=90, capture=True)
        report = work / "result.txt"
        if run.returncode or not report.is_file():
            raise RuntimeError(f"factory probe failed ({run.returncode}): {run.stdout.decode(errors='replace')[-2000:]}")
        fields = dict(line.split("=", 1) for line in report.read_text().splitlines())
        result = {
            "scope": "Xbox XDXR factory construction, COM identity and caller allocator only; no compilation methods were called. This does not implement or establish ray-compiler coverage.",
            "reference_sha256": digest(args.reference), "guest_sha256": digest(exe), "host": host.version,
            "class_guid": "16bb3c85-006d-4b72-9e10-63cd97df57f0",
            "ino_marshal_guid": "ecc8691b-c1db-4dc0-855e-65f6c551af49",
            "function_table_rvas": ["0x" + fields.pop(f"slot{i}") for i in range(13)],
            "observations": fields,
            "limits": ["Slots after IUnknown are not named or called by this probe.",
                       "The original DLL is only used in this local control, not by TuxBox at runtime.",
                       "Observations apply to the identified Xbox compiler binary only."],
        }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print("XDXR factory identity and allocator controls passed")


if __name__ == "__main__":
    main()
