#!/usr/bin/env python3
"""Compare Project TuxBox's static PE inventory with LLVM and an optional fork audit.

From the project root:
  python tools/check_package.py "$PACKAGE" --fork-imports "$FORK/tools/progress/scarlett-dll-imports.tsv" --output evidence/package-inventory.json

The comparison preserves symbol spelling, ordinal numbers and duplicate entries.
It reads DLLs and package metadata. It does not execute a title or decode archives.
"""
import argparse
from collections import Counter, defaultdict
import csv
import hashlib
import json
import os
from pathlib import Path, PureWindowsPath
import re
import subprocess
import sys
import tempfile
from xml.etree import ElementTree

import pefile

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from project_tuxbox.inspect import package


def llvm_imports(executable, path):
    result = subprocess.run([executable, "--file-headers", "--coff-imports", str(path)],
                            check=True, capture_output=True, text=True, timeout=60)
    tables = {"imports": [], "delay_imports": []}
    for kind, block in re.findall(r"^(Import|DelayImport) \{\n(.*?)^\}", result.stdout, re.M | re.S):
        module = re.search(r"^  Name: (.+)$", block, re.M)
        if not module:
            raise ValueError(f"{path.name}: missing LLVM import DLL name")
        symbols = []
        for line in block.splitlines():
            if line.lstrip().startswith("Symbol:"):
                entry = re.fullmatch(r" +Symbol: (.*?) \((\d+)\)", line)
                if not entry:
                    raise ValueError(f"{path.name}: unexpected LLVM symbol line {line!r}")
                symbols.append(entry[1] or f"#{entry[2]}")
        key = "imports" if kind == "Import" else "delay_imports"
        tables[key].append((module[1].casefold(), tuple(sorted(symbols))))
    # A missing descriptor walker must not turn a nonempty directory into an empty pass.
    for table, rva_label in (("imports", "ImportTableRVA"), ("delay_imports", "DelayImportDescriptorRVA")):
        rva = re.search(rf"^    {rva_label}: (0x[0-9A-Fa-f]+)$", result.stdout, re.M)
        if not rva or bool(int(rva[1], 16)) != bool(tables[table]):
            raise ValueError(f"{path.name}: {table} disagrees with the PE data directory in LLVM")
    return tables


def fork_rows(path):
    rows = defaultdict(Counter)
    with path.open(encoding="utf-8", newline="") as stream:
        for row in csv.DictReader(stream, delimiter="\t"):
            rows[row["importer"].casefold()][row["dependency"].casefold(), row["symbol"]] += 1
    return rows


def inventory(root, llvm, fork_tsv):
    report = package(root)
    files = {}
    extensions = Counter()
    for parent, dirs, names in os.walk(root):
        dirs[:] = [d for d in dirs if not (Path(parent) / d).is_symlink()]
        for name in names:
            path = Path(parent) / name
            if path.is_symlink() or not path.is_file():
                continue
            key = path.relative_to(root).as_posix().casefold()
            if key in files:
                raise ValueError(f"Ambiguous case-insensitive path {key}")
            files[key] = path
            extensions[path.suffix.lower()] += 1
    if dict(extensions) != report["files_by_extension"]:
        raise ValueError("Extension counts disagree with the filesystem inventory")

    declared = []
    for node in ElementTree.parse(files["microsoftgame.config"]).findall("./{*}ExecutableList/{*}Executable"):
        target = node.attrib["Name"]
        declared.append({"path": target, "id": node.get("Id"), "target": node.get("TargetDeviceFamily"),
                         "present": PureWindowsPath(target).as_posix().casefold() in files})
    if declared != [{k: row[k] for k in ("path", "id", "target", "present")} for row in report["executables"]]:
        raise ValueError("Executable declarations or presence disagree with the inventory")
    appx_declared = None
    if "appxmanifest.xml" in files:
        appx_declared = [{"id": node.get("Id"), "path": node.get("Executable")} for node in
                         ElementTree.parse(files["appxmanifest.xml"]).findall("./{*}Applications/{*}Application")]

    forks = fork_rows(fork_tsv) if fork_tsv else None
    libraries, distinct = [], {"imports": set(), "delay_imports": set()}
    totals = {key: Counter(named=0, ordinal=0, descriptors=0) for key in distinct}
    used_fork = set()
    for lib in report["libraries"]:
        path = root / lib["path"]
        if "error" in lib:
            raise ValueError(f"{lib['path']}: {lib['error']}")
        parsed = llvm_imports(llvm, path)
        row = {"path": lib["path"], "sha256": lib["sha256"]}
        for table in distinct:
            expected = [(entry["module"].casefold(), tuple(sorted(entry["symbols"]))) for entry in lib[table]]
            if sorted(parsed[table]) != sorted(expected):
                raise ValueError(f"{lib['path']}: LLVM and project inspector disagree on {table}")
            pairs = Counter((module, symbol) for module, symbols in expected for symbol in symbols)
            if table == "imports" and forks is not None:
                name = path.name.casefold()
                if name in used_fork or name not in forks or forks[name] != pairs:
                    raise ValueError(f"{lib['path']}: ordinary imports disagree with fork TSV or basename repeated")
                used_fork.add(name)
            distinct[table].update(pairs)
            count = sum(pairs.values())
            ordinals = sum(n for (_, symbol), n in pairs.items() if re.fullmatch(r"#\d+", symbol))
            totals[table].update(named=count - ordinals, ordinal=ordinals, descriptors=len(expected))
            row[table] = count
        libraries.append(row)
    if forks is not None and set(forks) != used_fork:
        raise ValueError("The fork audit contains different DLLs from this package")
    for table in totals:
        totals[table]["distinct_module_symbol_pairs"] = len(distinct[table])

    version = subprocess.run([llvm, "--version"], check=True, capture_output=True, text=True).stdout
    llvm_version = re.search(r"LLVM version ([^\s]+)", version)
    return {"package_version": report["identity"].get("Version"), "manifest_sha256": report["manifest_sha256"],
            "declared_executables": declared, "appx_applications": appx_declared,
            "inventory": {"dlls": len(libraries), "rpf_files": extensions[".rpf"], "ttpf_files": extensions[".ttpf"]},
            "cross_check": {"llvm_version": llvm_version[1] if llvm_version else "unknown", "pefile_version": pefile.__version__,
                            "ordinary_and_delay_match_llvm": True,
                            "ordinary_match_fork_tsv": True if fork_tsv else None,
                            "fork_tsv_sha256": hashlib.sha256(fork_tsv.read_bytes()).hexdigest() if fork_tsv else None},
            "symbol_totals": totals, "libraries": libraries}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("package", type=Path)
    parser.add_argument("--llvm-readobj", default="llvm-readobj")
    parser.add_argument("--fork-imports", type=Path, help="optional per-DLL ordinary import TSV")
    parser.add_argument("--output", type=Path, help="replace the named JSON only after all comparisons pass")
    args = parser.parse_args()
    try:
        data = json.dumps(inventory(args.package.resolve(), args.llvm_readobj, args.fork_imports), indent=2) + "\n"
        if args.output:
            with tempfile.NamedTemporaryFile(mode="w", dir=args.output.parent, delete=False, encoding="utf-8") as out:
                scratch = Path(out.name)
            try:
                scratch.write_text(data, encoding="utf-8")
                scratch.replace(args.output)
            finally:
                scratch.unlink(missing_ok=True)
        else:
            sys.stdout.write(data)
    except (OSError, ValueError, KeyError, ElementTree.ParseError, subprocess.SubprocessError) as exc:
        parser.exit(1, f"Package cross-check failed: {exc}\n")


if __name__ == "__main__":
    main()
