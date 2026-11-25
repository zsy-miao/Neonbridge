"""Inventory PE metadata and compare it with exports advertised on this x64 Windows host."""

import argparse
from collections import Counter, defaultdict
import ctypes
from functools import cache
import hashlib
import json
import os
from pathlib import Path
import struct
import sys
from tempfile import NamedTemporaryFile

import pefile


# The linked tests exercise desktop behavior; none provides a Scarlett oracle.
EXPERIMENTS = {
    ("xmem.dll", "XMemAlloc"): "era_heap_allocation, era_heap_failure, era_heap_concurrency, era_heap_observed_words, era_heap_large_alignment, xmem_shim_allocation",
    ("xmem.dll", "XMemFree"): "era_heap_allocation, era_heap_concurrency, era_heap_observed_words, era_heap_large_alignment, xmem_shim_allocation",
    ("xmem.dll", "XMemVirtualQuery"): "desktop_memory_query, era_query_fields, era_query_tail, xmem_shim_query: Windows page states; tail byte is a desktop policy; Scarlett ABI unverified",
    ("xmem.dll", "XMemGetWorkingSetStatistics"): "desktop_memory_query, xmem_shim_working_set_*: desktop xmem.dll returns DXGI GPU memory usage in bytes for selector 0x20000001; Scarlett scope unverified",
    ("d3d12_xs.dll", "D3D12SerializeVersionedRootSignature"): "desktop_root_signature",
    ("d3d12_xs.dll", "D3D12CreateRootSignatureDeserializer"): "desktop_root_signature",
}


def label(raw):
    return raw.decode("ascii", errors="backslashreplace") if raw is not None else None


def _file_digest(path):
    with path.open("rb") as f:
        return hashlib.file_digest(f, "sha256").hexdigest()


@cache
def _parsed_api_sets(path, digest):
    # Read the v6 namespace and its per-importer overrides from the host image.
    with pefile.PE(path, fast_load=True) as pe:
        section = next(s for s in pe.sections if s.Name.rstrip(b"\0") == b".apiset")
        data = section.get_data()
    version, _, _, count, entries, _, _ = struct.unpack_from("<7I", data)
    if version != 6:
        raise ValueError(f"Unsupported Windows API-set schema {version}")

    def name(offset, size):
        return data[offset:offset + size].decode("utf-16-le").lower()

    contracts = {}
    for i in range(count):
        _, start, _, hashed_size, values, number = struct.unpack_from("<6I", data, entries + i * 24)
        hosts = {}
        for j in range(number):
            _, alias, alias_size, host, host_size = struct.unpack_from("<5I", data, values + j * 20)
            hosts[name(alias, alias_size)] = name(host, host_size)
        contracts[name(start, hashed_size)] = hosts
    return contracts


def api_sets(system):
    schema = Path(system) / "apisetschema.dll"
    return _parsed_api_sets(str(schema), _file_digest(schema))


def known_dlls():
    """Return the section names in the host's \\KnownDlls object directory.

    The loader maps these names from System32 before it searches the application directory. The
    directory can name more modules than the KnownDLLs registry value. ntdll.dll is mapped into every
    process before any import is resolved, so a package copy never replaces it either.
    """
    from ctypes import wintypes

    class UnicodeString(ctypes.Structure):
        _fields_ = [("Length", wintypes.USHORT), ("MaximumLength", wintypes.USHORT), ("Buffer", ctypes.c_void_p)]

    class ObjectAttributes(ctypes.Structure):
        _fields_ = [("Length", wintypes.ULONG), ("RootDirectory", wintypes.HANDLE),
                    ("ObjectName", ctypes.POINTER(UnicodeString)), ("Attributes", wintypes.ULONG),
                    ("SecurityDescriptor", ctypes.c_void_p), ("SecurityQualityOfService", ctypes.c_void_p)]

    class DirectoryEntry(ctypes.Structure):
        _fields_ = [("Name", UnicodeString), ("TypeName", UnicodeString)]

    def text(value):
        return ctypes.wstring_at(value.Buffer, value.Length // 2)

    ntdll = ctypes.WinDLL("ntdll")
    ntdll.NtOpenDirectoryObject.restype = ntdll.NtQueryDirectoryObject.restype = ctypes.c_long
    ntdll.NtOpenDirectoryObject.argtypes = [ctypes.POINTER(wintypes.HANDLE), wintypes.ULONG,
                                            ctypes.POINTER(ObjectAttributes)]
    ntdll.NtQueryDirectoryObject.argtypes = [wintypes.HANDLE, ctypes.c_void_p, wintypes.ULONG, wintypes.BOOLEAN,
                                             wintypes.BOOLEAN, ctypes.POINTER(wintypes.ULONG),
                                             ctypes.POINTER(wintypes.ULONG)]
    path = ctypes.create_unicode_buffer("\\KnownDlls")
    name = UnicodeString(2 * (len(path) - 1), 2 * len(path), ctypes.cast(path, ctypes.c_void_p))
    attributes = ObjectAttributes(ctypes.sizeof(ObjectAttributes), None, ctypes.pointer(name),
                                  0x40, None, None)  # OBJ_CASE_INSENSITIVE
    handle = wintypes.HANDLE()
    status = ntdll.NtOpenDirectoryObject(ctypes.byref(handle), 1, ctypes.byref(attributes))  # DIRECTORY_QUERY
    if status < 0:
        raise OSError(f"NtOpenDirectoryObject(\\KnownDlls) failed with NTSTATUS {status & 0xFFFFFFFF:#010x}")
    names = {"ntdll.dll"}
    try:
        buffer = ctypes.create_string_buffer(4096)
        context, returned = wintypes.ULONG(), wintypes.ULONG()
        first = True
        while True:
            status = ntdll.NtQueryDirectoryObject(handle, buffer, len(buffer), True, first,
                                                  ctypes.byref(context), ctypes.byref(returned))
            first = False
            if status & 0xFFFFFFFF == 0x8000001A:  # STATUS_NO_MORE_ENTRIES
                break
            if status < 0:
                raise OSError(f"NtQueryDirectoryObject(\\KnownDlls) failed with NTSTATUS {status & 0xFFFFFFFF:#010x}")
            entry = DirectoryEntry.from_buffer(buffer)
            if text(entry.TypeName) == "Section":
                names.add(text(entry.Name).lower())
    finally:
        ctypes.windll.kernel32.CloseHandle(handle)
    return frozenset(names)


def exports(pe):
    """Read the export directory and its tables as stored.

    pefile stops reading names at the first one outside its character set, lists the remaining
    named exports as bare ordinals, cuts names at 512 bytes and gives up after 8,192 entries.
    It also drops a name or forwarder it cannot read, or the whole table, with only a warning.
    Here a table, name or forwarder that does not lie in section file data is an error.
    """
    start, size = pe.OPTIONAL_HEADER.DATA_DIRECTORY[0].VirtualAddress, pe.OPTIONAL_HEADER.DATA_DIRECTORY[0].Size
    if not start:
        return []
    (base, count, name_count, functions_rva, names_rva,
     ordinals_rva) = struct.unpack("<16x6I", read_rva(pe, start, 40, "Export directory"))

    def array(form, rva, count, what):
        return struct.unpack(f"<{count}{form}", read_rva(pe, rva, struct.calcsize(form) * count, what))

    functions = array("I", functions_rva, count, "Export address table")
    names = array("I", names_rva, name_count, "Export name pointer table")
    indices = array("H", ordinals_rva, name_count, "Export name ordinal table")
    if any(i >= len(functions) for i in indices):
        raise ValueError("Export name ordinal is outside the export address table")
    entries = [(i, read_name(pe, rva, "export name")) for rva, i in zip(names, indices)]
    named = set(indices)
    entries += [(i, None) for i in range(len(functions)) if i not in named]
    return [dict(ordinal=base + i, name=label(name),
                 forwarder=label(read_name(pe, functions[i], "export forwarder", end=start + size))
                 if start <= functions[i] < start + size else None)
            for i, name in entries if functions[i]]


def export_keys(rows):
    keys = {f"#{row['ordinal']}" for row in rows}
    keys.update(row["name"] for row in rows if row["name"] is not None)
    return keys


def _parse_forwarded_ordinal(digits):
    """Parse a forwarded ordinal as an ASCII decimal integer in 1..65535, or return None."""
    if not digits:
        return None
    value = 0
    for ch in digits:
        if ch < "0" or ch > "9":
            return None
        value = value * 10 + (ord(ch) - ord("0"))
        if value > 0xFFFF:
            return None
    return value if value >= 1 else None


def resolve_desktop(dll, symbol, importer, contracts, system_exports, package_exports=None, known_dlls=()):
    """Follow export forwarders without loading any DLL.

    package_exports, when given, maps a plain module name to its package paths and export table.
    Package images then take precedence over System32. API-set contracts always select System32 hosts,
    and so do known_dlls: the loader maps those names from System32 before it searches the
    application directory.
    """
    chain = []
    seen = set()
    while True:
        aliases = contracts.get(dll.rpartition("-")[0]) if dll.startswith(("api-", "ext-")) else None
        known = aliases is None and dll in known_dlls
        packaged, table = (package_exports(dll) if package_exports and aliases is None and not known
                           else ((), None))
        if len(packaged) == 1:
            host = packaged[0]
        else:
            host = dll if aliases is None else aliases.get(importer, aliases.get("", ""))
        step = dict(dll=dll, provider=host or None, symbol=symbol)
        if package_exports:
            step["package"] = bool(packaged)
            if known:
                step["known_dll"] = True
        chain.append(step)
        if len(packaged) > 1:
            step["provider"] = None
            status = "ambiguous_package_basename"
            break
        if (host, symbol) in seen:
            status = "forwarder_cycle"
            break
        seen.add((host, symbol))
        if not packaged:
            table = system_exports(host)
        if table is None:
            status = "provider_not_found"
            break
        if symbol not in table:
            status = "export_not_found"
            break
        target = table[symbol]
        if target is None:
            status = "export_present"
            break
        step["forwarder"] = target
        module, separator, name = target.partition(".")
        if name.lower().startswith("dll."):
            name = name[4:]
        if not separator or not module or not name:
            status = "invalid_forwarder"
            break
        if name.startswith("#"):
            ordinal = _parse_forwarded_ordinal(name[1:])
            if ordinal is None:
                status = "invalid_forwarder"
                break
            name = f"#{ordinal}"
        dll, symbol, importer = module.lower() + ".dll", name, Path(host).name.lower()
    return dict(status=status, chain=chain)


def read_rva(pe, rva, size, what="Import data"):
    section = pe.get_section_by_rva(rva)
    data = section.get_data(rva, size) if section else b""
    if len(data) != size:
        raise ValueError(f"{what} at RVA {rva:#x} is outside the file's sections")
    return data


def read_name(pe, rva, what="import name", *, end=None):
    section = pe.get_section_by_rva(rva)
    limit = 0x10000 if end is None else min(0x10000, end - rva)
    data = section.get_data(rva, limit) if section and limit > 0 else b""
    if not data:
        raise ValueError(f"{what.capitalize()} at RVA {rva:#x} is outside the file's sections")
    name, terminator, _ = data.partition(b"\0")
    if not terminator:
        raise ValueError(f"Unterminated {what} at RVA {rva:#x}")
    return name


def module_name(raw):
    """Return the file name the loader looks for when an import descriptor names raw.

    Like LoadLibrary, the loader appends .dll to a name without an extension. A trailing dot stops
    the append and is not part of the file name.
    """
    name = label(raw).lower()
    if name.endswith("."):
        return name.rstrip(".")
    return name if "." in name.replace("/", "\\").rpartition("\\")[2] else name + ".dll"


def import_tables(pe, path):
    """Read import descriptors and lookup tables as stored.

    pefile skips lookup entries whose names fall outside its function-name character set and
    entries for ordinal 0, drops a descriptor left empty by either, and names ordinal imports
    from a few system DLLs. The inventory keeps every entry exactly as the image requests it,
    with each module under the file name the loader looks for.
    """
    wide = pe.OPTIONAL_HEADER.Magic == 0x20B
    width, by_ordinal = (8, 1 << 63) if wide else (4, 1 << 31)
    directories = pe.OPTIONAL_HEADER.DATA_DIRECTORY
    imports = []
    for kind, index, layout in (("normal", 1, struct.Struct("<5I")), ("delay", 13, struct.Struct("<8I"))):
        if index >= len(directories) or not (directories[index].VirtualAddress or directories[index].Size):
            continue
        rva, found = directories[index].VirtualAddress, 0
        while True:
            fields = layout.unpack(read_rva(pe, rva, layout.size))
            rva += layout.size
            if kind == "normal":
                original_first_thunk, _, _, name, first_thunk = fields
                lookup, base = original_first_thunk or first_thunk, 0
            else:
                attributes, name, _, _, lookup = fields[:5]
                # Version 1 delay descriptors hold RVAs. Older ones hold virtual addresses.
                base = 0 if attributes & 1 else pe.OPTIONAL_HEADER.ImageBase
            if not name:
                break
            found += 1
            module = module_name(read_name(pe, name - base))
            entry = lookup - base
            while value := int.from_bytes(read_rva(pe, entry, width), "little"):
                entry += width
                if value & by_ordinal:
                    symbol = f"#{value & 0xFFFF}"
                else:
                    target_rva = value - base
                    if wide and (target_rva > 0x7FFFFFFF or target_rva < 0):
                        raise ValueError(f"Reserved bits set in PE32+ import thunk {value:#x}")
                    symbol = label(read_name(pe, (target_rva & 0x7FFFFFFF) + 2))
                imports.append((kind, module, symbol))
        if not found:
            raise ValueError(f"Could not parse {kind} imports in {path}")
    return imports


def parse_image(path):
    with pefile.PE(str(path), fast_load=True) as pe:
        pe.parse_data_directories(directories=[0])
        imports = import_tables(pe, path)
        digest = _file_digest(path)
        info = {
            "machine": pefile.MACHINE_TYPE.get(pe.FILE_HEADER.Machine, hex(pe.FILE_HEADER.Machine)),
            "sha256": digest, "exports": exports(pe), "parse_warnings": pe.get_warnings(),
        }
    return info, imports


def _export_table(rows):
    table = {}
    for row in rows:
        table[f"#{row['ordinal']}"] = row["forwarder"]
        if row["name"] is not None:
            table[row["name"]] = row["forwarder"]
    return table


@cache
def _parsed_system_exports(path, digest):
    with pefile.PE(path, fast_load=True) as pe:
        pe.parse_data_directories(directories=[0])
        return _export_table(exports(pe))


def scan(root, *, extra_image=None, system=None):
    system = Path(system) if system is not None else Path(os.environ["SystemRoot"]) / "System32"
    process, native = ctypes.c_ushort(), ctypes.c_ushort()
    if not ctypes.windll.kernel32.IsWow64Process2(
            ctypes.c_void_p(-1), ctypes.byref(process), ctypes.byref(native)):
        raise ctypes.WinError()
    if process.value != 0 or native.value != 0x8664:
        raise ValueError("Use a native x64 Windows Python for the desktop comparison")
    contracts = api_sets(system)
    known = known_dlls()
    system_images = {p.name.lower(): p for p in system.glob("*.dll")}

    @cache
    def system_exports(name):
        if name not in system_images:
            return None
        target = system_images[name]
        return _parsed_system_exports(str(target), _file_digest(target))

    images = {}
    edges = []
    bundled = defaultdict(list)
    paths = {p.relative_to(root).as_posix(): p for p in root.rglob("*")
             if p.suffix.lower() in {".dll", ".exe"} and p.is_file()}
    if extra_image:
        relative, candidate = extra_image
        present = next((p for name, p in paths.items() if name.casefold() == relative.casefold()), None)
        if present and present.resolve() != candidate.resolve():
            raise ValueError(f"Another file already occupies {relative}")
        if not present:
            paths = {name: path for name, path in paths.items() if path.resolve() != candidate.resolve()}
            paths[relative] = candidate
    for relative, path in sorted(paths.items()):
        images[relative], imports = parse_image(path)
        bundled[Path(relative).name.lower()].append((relative, export_keys(images[relative]["exports"])))
        edges.extend((relative, *imp) for imp in imports)
    if not images:
        raise ValueError(f"No DLLs or EXEs under {root}")

    def package_exports(name):
        found = [path for path, _ in bundled[name]]
        return found, _export_table(images[found[0]]["exports"]) if len(found) == 1 else None

    rows = []
    for image, kind, dll, symbol in sorted(set(edges)):
        importer = Path(image).name.lower()
        resolution = resolve_desktop(dll, symbol, importer, contracts, system_exports)
        first = resolution["chain"][0]
        # Keep desktop_export as the first-hop comparison for existing report readers.
        status = "export_present" if "forwarder" in first else resolution["status"]
        candidate = None
        if dll == "d3d12_xs.dll" and "d3d12.dll" in system_images and symbol in system_exports("d3d12.dll"):
            candidate = "d3d12.dll"
        rows.append({
            "image": image, "kind": kind, "dll": dll, "symbol": symbol,
            "bundle_exports_in": sorted(p for p, keys in bundled[dll] if symbol in keys),
            "desktop_provider": first["provider"], "desktop_export": status, "desktop_candidate": candidate,
            "desktop_resolution": resolution,
            # A KnownDLL forwarder may lead into the package even without a same-name package file.
            "package_resolution": resolve_desktop(dll, symbol, importer, contracts, system_exports,
                                                  package_exports, known)
                                  if bundled[dll] or (dll in known and "forwarder" in first) else None,
        })
    # A packaged forwarder satisfies an edge only when its chain reaches a terminal export.
    unresolved = [r for r in rows if r["desktop_resolution"]["status"] != "export_present"
                  and not (r["package_resolution"] and
                           (r["package_resolution"]["status"] == "export_present" or
                            (r["bundle_exports_in"] and r["package_resolution"]["status"] == "ambiguous_package_basename")))]
    return {
        "schema": 1, "host_windows_build": sys.getwindowsversion().build, "pefile": pefile.__version__,
        "behavior": "Scarlett behavior unverified for every import. Static export matches include forwarders.",
        "scope": "Static PE directory entries only. Dynamic lookups and object methods are unknown.",
        "summary": {
            "images": len(images), "edges": len(rows), "import_modules": len({r["dll"] for r in rows}),
            "distinct_imports": len({(r["dll"], r["symbol"]) for r in rows}),
            "normal": sum(r["kind"] == "normal" for r in rows),
            "delay": sum(r["kind"] == "delay" for r in rows),
            "by_ordinal": sum(r["symbol"].startswith("#") for r in rows),
            "exports": sum(len(i["exports"]) for i in images.values()),
            "desktop_results": dict(Counter(r["desktop_export"] for r in rows)),
            "desktop_terminal_results": dict(Counter(r["desktop_resolution"]["status"] for r in rows)),
            "desktop_forwarded_edges": sum(len(r["desktop_resolution"]["chain"]) > 1 for r in rows),
            "package_terminal_results": dict(Counter(r["package_resolution"]["status"]
                                                     for r in rows if r["package_resolution"])),
        },
        "unresolved_edges": [dict(r, desktop_experiment=EXPERIMENTS.get((r["dll"], r["symbol"])))
                             for r in unresolved],
        "known_dlls": sorted(known), "images": images, "imports": rows,
    }


def changes(old, new):
    if old["schema"] != new["schema"]:
        raise ValueError("Inventory schema mismatch")

    def edges(report):
        return {(r["image"], r["kind"], r["dll"], r["symbol"]) for r in report["imports"]}

    before, after = edges(old), edges(new)
    previous, current = old["images"], new["images"]
    return {
        "added_images": sorted(current.keys() - previous.keys()),
        "removed_images": sorted(previous.keys() - current.keys()),
        "changed_images": sorted(p for p in previous.keys() & current.keys()
                                 if previous[p]["sha256"] != current[p]["sha256"]),
        "added_edges": sorted(after - before), "removed_edges": sorted(before - after),
        "newly_required_modules": sorted({e[2] for e in after} - {e[2] for e in before}),
        "newly_required_symbols": sorted({e[2:] for e in after} - {e[2:] for e in before}),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--baseline", type=Path)
    args = parser.parse_args()
    root = args.directory.resolve(strict=True)
    output = args.output.resolve()
    if output.is_relative_to(root) or (args.baseline and output == args.baseline.resolve()):
        parser.error("Choose an output outside the input tree and separate from the baseline")
    result = scan(root)
    if args.baseline:
        result["diff"] = changes(json.loads(args.baseline.read_text(encoding="utf-8")), result)
    output.parent.mkdir(parents=True, exist_ok=True)
    with NamedTemporaryFile(dir=output.parent, mode="w", encoding="utf-8", delete=False) as f:
        json.dump(result, f, indent=2, ensure_ascii=False)
        f.write("\n")
    Path(f.name).replace(output)
    print(f"{len(result['images'])} images | {len(result['imports'])} import edges | {output}")
    print(f"Desktop terminal results: {result['summary']['desktop_terminal_results']} | "
          f"forwarded edges: {result['summary']['desktop_forwarded_edges']}")


if __name__ == "__main__":
    main()
