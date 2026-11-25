"""Read package binaries, PE imports and GDK launch declarations without loading guest code."""
from collections import Counter
from contextlib import closing
from hashlib import sha256
from io import BytesIO
from pathlib import Path, PureWindowsPath
import struct
from xml.etree import ElementTree
import zlib

import pefile

from .gameconfig import binary_game_config
from .symbols import symbol_header


def digest(path):
    result = sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1 << 20), b""):
            result.update(chunk)
    return result.hexdigest()


def export_table(path):
    """Return reference ordinals, their aliases and the slots containing data."""
    with closing(pefile.PE(str(path), fast_load=True)) as pe:
        if pe.FILE_HEADER.Machine != 0x8664 or pe.OPTIONAL_HEADER.Magic != 0x20b:
            raise ValueError(f"{path.name} must be a Windows x64 PE")
        pe.parse_data_directories(directories=[pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_EXPORT"]])
        directory = getattr(pe, "DIRECTORY_ENTRY_EXPORT", None)
        if directory is None:
            raise ValueError(f"{path.name} has no export table")
        table = directory.struct
        first = pe.OPTIONAL_HEADER.DATA_DIRECTORY[0].VirtualAddress
        end = first + pe.OPTIONAL_HEADER.DATA_DIRECTORY[0].Size
        slots = {}
        data = set()
        for offset in range(table.NumberOfFunctions):
            address = pe.get_dword_at_rva(table.AddressOfFunctions + 4 * offset)
            if address is None:
                raise ValueError(f"{path.name} has an unreadable export table")
            if address == 0:
                continue
            ordinal = table.Base + offset
            slots[ordinal] = set()
            if first <= address < end:  # PE forwarder string
                continue
            section = pe.get_section_by_rva(address)
            if section is None:
                raise ValueError(f"{path.name} has an unmapped export at ordinal {ordinal}")
            if not section.Characteristics & 0x20000000:  # IMAGE_SCN_MEM_EXECUTE
                data.add(ordinal)
        for entry in directory.symbols:
            if entry.name and entry.ordinal in slots:
                slots[entry.ordinal].add(entry.name.decode("ascii"))
        return slots, data


def import_symbol(entry):
    if entry.import_by_ordinal:
        return f"#{entry.ordinal}"
    return entry.name.decode("ascii", "backslashreplace")


def image(path, *, imports=True):
    with closing(pefile.PE(str(path), fast_load=True)) as pe:
        header = pe.FILE_HEADER
        result = {
            "filename": path.name,
            "machine": hex(header.Machine),
            "x64_pe": header.Machine == 0x8664 and pe.OPTIONAL_HEADER.Magic == 0x20b,
            "kind": ("dll" if header.Characteristics & 0x2000 else
                     "executable" if header.Characteristics & 0x0002 else "other"),
            "entry_rva": hex(pe.OPTIONAL_HEADER.AddressOfEntryPoint),
        }
        if imports:
            tables = {"imports": "IMAGE_DIRECTORY_ENTRY_IMPORT", "delay_imports": "IMAGE_DIRECTORY_ENTRY_DELAY_IMPORT"}
            pe.parse_data_directories(directories=[pefile.DIRECTORY_ENTRY[name] for name in tables.values()])
            for key, name in tables.items():
                result[key] = [
                    {"module": desc.dll.decode("ascii", "backslashreplace"),
                     "symbols": [import_symbol(entry) for entry in desc.imports]}
                    for desc in getattr(pe, name.removeprefix("IMAGE_"), [])
                ]
            result["sha256"] = digest(path)
            result["parser_notes"] = pe.get_warnings()
    return result


def binary_file(path):
    """Read a bounded prefix. Only game-config hives need a full, size-limited parse."""
    size = path.stat().st_size
    result = {"kind": "opaque binary", "size_bytes": size}
    with path.open("rb") as stream:
        if path.name.casefold() == "microsoft_gameconfig.bin":
            try:
                result["game_config"] = binary_game_config(stream)
                result["kind"] = "Xbox game configuration"
            except ValueError as exc:
                result["error"] = str(exc)
            return result
        head = stream.read(65536)

    if head.startswith(b"MZ"):
        try:
            with closing(pefile.PE(data=head, fast_load=True)) as pe:
                entry = pe.OPTIONAL_HEADER.AddressOfEntryPoint
                if len(pe.sections) != pe.FILE_HEADER.NumberOfSections or not pe.sections:
                    return result
                if any(s.SizeOfRawData and s.PointerToRawData + s.SizeOfRawData > size for s in pe.sections):
                    return result
                executable = any(s.Characteristics & 0x20000000 for s in pe.sections)
                header = pe.OPTIONAL_HEADER
                catalog = header.Subsystem == 17  # IMAGE_SUBSYSTEM_XBOX_CODE_CATALOG in winnt.h.
                result.update(kind="Xbox code catalog" if catalog else "PE image",
                              machine=hex(pe.FILE_HEADER.Machine), entry_rva=hex(entry),
                              sections=len(pe.sections), executable_sections=executable)
                if catalog:
                    result.update(subsystem=header.Subsystem, code_bytes=header.SizeOfCode,
                                  force_integrity=bool(header.DllCharacteristics & 0x80))
        except pefile.PEFormatError:
            pass
    elif head.startswith(b"ANOA\x2a\xcc\xf4\x80") and len(head) >= 24:
        result.update(kind="Unsync build manifest", version=struct.unpack_from("<Q", head, 8)[0])
    elif len(head) > 8:
        unpacked, packed = struct.unpack_from("<II", head)
        if packed != size - 8:
            return result
        try:
            header = zlib.decompressobj().decompress(head[8:], 4096)
            result.update(symbol_header(BytesIO(header), unpacked))
        except (ValueError, zlib.error):
            pass
    return result


def member(root, declared):
    name = PureWindowsPath(declared)
    if not name.parts or name.anchor or ".." in name.parts or ":" in declared:
        raise ValueError(f"Expected a package-relative Windows path, got {declared!r}")
    root = Path(root).resolve()
    folder = root
    for part in name.parts:
        if not folder.is_dir():
            return None
        matches = [p for p in folder.iterdir() if p.name.casefold() == part.casefold()]
        if len(matches) > 1:
            raise ValueError(f"Ambiguous package member {declared!r}")
        if not matches:
            return None
        folder = matches[0].resolve()
        if not folder.is_relative_to(root):
            raise ValueError(f"Package member leaves the package directory: {declared!r}")
    return folder if folder.is_file() else None


def manifest(root):
    config = member(root, "MicrosoftGame.config")
    if config is None:
        config = member(root, "microsoft_gameconfig.bin")
        if config is None:
            raise ValueError("No MicrosoftGame.config or microsoft_gameconfig.bin in that directory")
        with config.open("rb") as stream:
            game_config = binary_game_config(stream)
        declarations = game_config["executables"]
        identity = ElementTree.Element("Identity", game_config["identity"])
    else:
        if config.stat().st_size > 16 * 1024 * 1024:
            raise ValueError("Manifest exceeds the inspector's 16 MiB limit")
        game = ElementTree.parse(config).getroot()
        if game.tag.split("}")[-1] != "Game":
            raise ValueError("Expected a Game manifest")
        identity = game.find("{*}Identity")
        declarations = game.findall("{*}ExecutableList/{*}Executable")
    entries = []
    for exe in declarations:
        name = exe.get("Name")
        if not name:
            raise ValueError("An Executable declaration is missing its Name")
        entries.append((name, exe.get("Id"), exe.get("TargetDeviceFamily"), member(root, name)))
    if not entries:
        raise ValueError("Manifest has no executable declarations")
    return config, identity, entries


def package(root):
    root = root.resolve()
    config, identity, entries = manifest(root)
    files = Counter()
    libraries = []
    binaries = []
    for path in sorted(root.rglob("*")):
        if path.is_symlink() or not path.is_file():
            continue
        ext = path.suffix.lower()
        files[ext] += 1
        if ext == ".dll":
            try:
                info = image(path)
            except (pefile.PEFormatError, OSError) as exc:
                info = {"error": str(exc)}
            libraries.append({"path": path.relative_to(root).as_posix(), **info})
        elif ext == ".bin":
            try:
                info = binary_file(path)
            except OSError as exc:
                info = {"error": str(exc)}
            binaries.append({"path": path.relative_to(root).as_posix(), **info})
    return {
        "manifest_file": config.name, "manifest_sha256": digest(config),
        "identity": dict(identity.attrib) if identity is not None else {},
        "files_by_extension": dict(sorted(files.items())),
        "executables": [{"path": name, "id": app_id, "target": target, "present": path is not None,
                         **({"image": image(path)} if path else {})} for name, app_id, target, path in entries],
        "libraries": libraries, "binary_files": binaries,
    }


def package_root(target, executable):
    """Return the package directory whose manifest declares the executable, or None.

    A package directory target is its own root. For an executable target, the closest
    ancestor whose MicrosoftGame.config or microsoft_gameconfig.bin declares that
    executable is the root, at any depth.
    """
    executable = executable.resolve()
    target = target.resolve()
    for directory in (target,) if target.is_dir() else executable.parents:
        try:
            _, _, entries = manifest(directory)
        except (OSError, ValueError, struct.error, ElementTree.ParseError):
            continue
        if any(path is not None and path == executable for *_, path in entries):
            return directory
    return None


def launch_target(path, app_id):
    path = path.resolve()
    if path.is_dir():
        _, _, entries = manifest(path)
        choices = ", ".join(str(row[1]) for row in entries)
        if app_id is not None:
            entries = [row for row in entries if row[1] == app_id]
        if len(entries) != 1:
            raise ValueError(f"Select one declared executable with --app-id; available IDs: {choices}")
        name, _, _, candidate = entries[0]
        if candidate is None:
            raise ValueError(f"Declared executable is missing: {name}")
        path = candidate
    elif app_id is not None:
        raise ValueError("--app-id applies to a package directory")
    if not path.is_file():
        raise ValueError(f"File does not exist: {path}")
    info = image(path, imports=False)
    if not info["x64_pe"] or info["kind"] != "executable" or info["entry_rva"] == "0x0":
        raise ValueError("Launch requires an x86-64 PE32+ executable with an entry point")
    return path
