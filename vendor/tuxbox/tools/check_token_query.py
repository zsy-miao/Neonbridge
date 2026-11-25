#!/usr/bin/env python3
"""Measure the class-39 size query on x64 Windows or selected Proton distributions.

python tools/check_token_query.py --proton "/path/to/Proton - Experimental" --proton "/path/to/GE-Proton"
"""

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from zipfile import ZipFile, ZIP_DEFLATED

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import Toolchain
from project_tuxbox.host import Host
from project_tuxbox.inspect import digest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--proton", type=Path, action="append", help="installed distribution (repeatable)")
    parser.add_argument("--output", type=Path, help="write JSON to this file")
    parser.add_argument("--prepare-control", action="store_true",
                        help="build the transferable Windows control ZIP and exit")
    args = parser.parse_args()
    if args.prepare_control and (args.proton or args.output):
        parser.error("--prepare-control is a build-only action and takes no run arguments")
    build = ROOT / "build/token-query"
    tools = Toolchain()
    obj = tools.compile(Path(__file__).with_name("token_query.c").resolve(), build, "/W4", "/WX")
    exe = build / "token-query.exe"
    tools.link(exe, obj, "ntdll.lib", "kernel32.lib")
    if args.prepare_control:
        sources = ROOT / "tools/token-query-control"
        archive = build / "token-query-control.zip"
        with ZipFile(archive, "w", compression=ZIP_DEFLATED) as bundle:
            bundle.write(exe, "package/token-query.exe")
            bundle.write(sources / "Compare-TokenQuery.ps1", "Compare-TokenQuery.ps1")
            bundle.write(sources / "AppxManifest.xml", "package/AppxManifest.xml")
            for logo in sorted((sources / "Assets").glob("Logo*.png")):
                bundle.write(logo, f"package/Assets/{logo.name}")
        print(f"{archive}\nGuest SHA-256 {digest(exe)}")
        return
    result = {"type": "host-capability", "probe": "NtQueryInformationToken",
              "target": "current process token", "information_class": 39, "buffer_bytes": 0,
              "exe_sha256": digest(exe), "observations": []}
    if os.name == "nt" and args.proton:
        parser.error("--proton requires Linux")
    for proton in args.proton or [None]:
        backend = "native" if os.name == "nt" else "proton"
        prefix = None if backend == "native" else build / "prefixes" / (proton.name if proton else "default")
        host = Host(backend, proton, prefix)
        with tempfile.TemporaryDirectory(dir=build) as temp:
            receipt = Path(temp) / "result.json"
            proc = host.run(exe, [str(receipt.relative_to(build))], timeout=120, capture=True)
            proc.check_returncode()
            values = json.loads(receipt.read_text(encoding="utf-8"))
        result["observations"].append({"backend": backend, "provider": host.version, **values})
    text = json.dumps(result, indent=2) + "\n"
    if args.output:
        args.output.write_text(text, encoding="utf-8")
    else:
        print(text, end="")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, subprocess.SubprocessError) as exc:
        sys.exit(f"token query failed: {exc}")
