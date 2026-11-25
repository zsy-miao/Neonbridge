#!/usr/bin/env python3
"""Load the supplied Scarlett DLLs that import the staged Xbox graphics and memory providers."""
import argparse
from contextlib import closing
import json
from pathlib import Path
import shutil
import sys
from tempfile import TemporaryDirectory

import pefile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import Toolchain
from project_tuxbox.host import Host
from project_tuxbox.inspect import digest, image, member


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("package", type=Path)
    parser.add_argument("--providers", type=Path, required=True)
    parser.add_argument("--proton", type=Path)
    parser.add_argument("--prefix", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--all-libraries", action="store_true", help="Also load supplied DLLs without Xbox graphics or memory imports")
    args = parser.parse_args()
    host = Host(None, args.proton, args.prefix)
    providers = {}
    exports = {}
    for name in ("xmem.dll", "d3d12_xs.dll"):
        path = member(args.providers.resolve(), name)
        if not path:
            parser.error(f"Missing {name} in provider directory")
        providers[name] = path
        with closing(pefile.PE(str(path), fast_load=True)) as pe:
            pe.parse_data_directories(directories=[pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_EXPORT"]])
            exports[name] = {e.name.decode("ascii") for e in pe.DIRECTORY_ENTRY_EXPORT.symbols if e.name}
    result = {"host": host.version, "providers": {k: digest(v) for k, v in providers.items()}, "libraries": {}}
    build = ROOT / "build/scarlett-libraries"
    build.mkdir(parents=True, exist_ok=True)
    with TemporaryDirectory(dir=build) as tmp:
        work = Path(tmp)
        tools = Toolchain()
        obj = tools.compile(ROOT / "tools/reference/scarlett/load.c", work, "/W4", "/WX")
        exe = work / "probe.exe"
        tools.link(exe, obj, "kernel32.lib")
        result["guest_sha256"] = digest(exe)
        package_dlls = sorted(f for f in args.package.iterdir() if f.suffix.casefold() == ".dll")
        for dll in package_dlls:
            shutil.copyfile(dll, work / dll.name)
        for name, path in providers.items():
            shutil.copyfile(path, work / name)
        for dll in package_dlls:
            pe = image(dll)
            imports = {d["module"].lower(): d["symbols"] for d in pe["imports"] if d["module"].lower() in providers}
            if not imports and not args.all_libraries:
                continue
            receipt = work / "load.txt"
            receipt.unlink(missing_ok=True)
            run = host.run(exe, [dll.name], timeout=60, capture=True)
            measured = receipt.read_text().splitlines() if receipt.is_file() else []
            result["libraries"][dll.name] = {
                "sha256": pe["sha256"], "provider_imports": imports,
                "missing_named_exports": {k: sorted(set(v) - exports[k]) for k, v in imports.items()},
                "process_exit": run.returncode,
                "LoadLibraryW_error": int(measured[0]) if measured else None,
                "loaded_from_supplied_path": measured[1] == "1" if measured else None,
            }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    rows = result["libraries"].values()
    return int(not rows or any(r["process_exit"] or r["LoadLibraryW_error"] != 0 or
                               any(r["missing_named_exports"].values()) for r in rows))


if __name__ == "__main__":
    sys.exit(main())
