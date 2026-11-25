from functools import reduce
from operator import xor
from pathlib import Path
import struct
from tempfile import TemporaryDirectory
import unittest
import zlib

from project_tuxbox.inspect import binary_file, launch_target, manifest, package
from project_tuxbox.symbols import lookup_symbols


def config_hive(executable):
    """Build the REGF cells for one executable using synthetic package metadata."""
    block = bytearray(4096)
    pos = 32

    def allocate(data):
        nonlocal pos
        offset = pos
        pos += (4 + len(data) + 7) & ~7
        struct.pack_into("<i", block, offset, offset - pos)
        block[offset+4:offset+4+len(data)] = data
        return offset

    def key(name, parent):
        record = bytearray(76 + len(name))
        record[:2] = b"nk"
        struct.pack_into("<H", record, 2, 0x20)
        struct.pack_into("<I", record, 16, parent)
        struct.pack_into("<H", record, 72, len(name))
        record[76:] = name.encode()
        return allocate(record)

    def value(name, data):
        if isinstance(data, str):
            raw = (data + "\0").encode("utf-16-le")
            size, offset, kind = len(raw), allocate(raw), 1
        else:
            size, offset, kind = 0x80000004, data, 4
        return allocate(struct.pack("<2sHIIIHH", b"vk", len(name), size, offset, kind, 1, 0) + name.encode())

    root = key("Game", 0xffffffff)
    group = key("Executable", root)
    entry = key("00000000", group)
    for parent, child in ((root, group), (group, entry)):
        index = allocate(struct.pack("<2sHII", b"lf", 1, child, 0))
        struct.pack_into("<III", block, parent + 4 + 20, 1, 0, index)
    for parent, values in ((root, {"configVersion": 1, "TitleId": 0x213}),
                           (entry, {"Name": executable, "Id": "Main", "TargetDeviceFamily": "Scarlett"})):
        pointers = [value(name, data) for name, data in values.items()]
        index = allocate(struct.pack(f"<{len(values)}I", *pointers))
        struct.pack_into("<II", block, parent + 4 + 36, len(values), index)
    struct.pack_into("<4sII", block, 0, b"hbin", 0, len(block))
    struct.pack_into("<i", block, pos, len(block) - pos)
    header = bytearray(4096)
    struct.pack_into("<4sII", header, 0, b"regf", 1, 1)
    struct.pack_into("<7I", header, 20, 1, 3, 0, 1, root, len(block), 1)
    struct.pack_into("<I", header, 508, reduce(xor, struct.unpack_from("<127I", header)))
    return header + block


class PackageBinaries(unittest.TestCase):
    def setUp(self):
        tmp = TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name).resolve()
        self.config = self.root / "Microsoft_GameConfig.BIN"

    def test_binary_only_declaration_and_missing_game(self):
        self.config.write_bytes(config_hive(r"Bin\Game.exe"))
        nested = self.root / "data"
        nested.mkdir()
        (nested / "assets.BIN").write_bytes(b"unknown data")
        result = package(self.root)
        self.assertEqual(result["manifest_file"], self.config.name)
        self.assertEqual(result["executables"], [{"path": r"Bin\Game.exe", "id": "Main",
                                                  "target": "Scarlett", "present": False}])
        bins = {row["path"]: row for row in result["binary_files"]}
        self.assertEqual(bins["data/assets.BIN"]["kind"], "opaque binary")
        self.assertEqual(bins[self.config.name]["game_config"]["title_id"], "00000213")
        with self.assertRaisesRegex(ValueError, "Declared executable is missing"):
            launch_target(self.root, "Main")
        (self.root / "bin").mkdir()
        (self.root / "bin/game.EXE").touch()
        _, _, rows = manifest(self.root)
        self.assertEqual(rows[0][-1], self.root / "bin/game.EXE")

    def test_xml_precedence_and_untrusted_declaration(self):
        self.config.write_bytes(config_hive("../outside.exe"))
        with self.assertRaisesRegex(ValueError, "package-relative"):
            launch_target(self.root, "Main")
        xml = self.root / "MicrosoftGame.config"
        xml.write_text('<Game><ExecutableList><Executable Name="xml.exe" Id="XML"/></ExecutableList></Game>')
        self.assertEqual(manifest(self.root)[2], [("xml.exe", "XML", None, None)])
        xml.write_text('<Game/>')
        with self.assertRaisesRegex(ValueError, "no executable"):
            manifest(self.root)

    def test_bad_hive_and_oversized_file(self):
        raw = config_hive("game.exe")
        raw[0x1020:0x1024] = struct.pack("<i", -99999)
        self.config.write_bytes(raw)
        self.assertIn("cell bounds", binary_file(self.config)["error"])
        with self.assertRaisesRegex(ValueError, "cell bounds"):
            launch_target(self.root, "Main")
        with self.config.open("wb") as f:
            f.truncate(1 << 30)
        self.assertIn("exceeds", binary_file(self.config)["error"])

    def test_catalog_identification_uses_subsystem(self):
        # Minimal data-only PE32+ with a single padded 4 KiB section.
        pe = bytearray(8192)
        pe[:2] = b"MZ"
        struct.pack_into("<I", pe, 60, 64)
        struct.pack_into("<4sHHIIIHH", pe, 64, b"PE\0\0", 0x8664, 1, 0, 0, 0, 240, 2)
        optional = 88
        struct.pack_into("<H", pe, optional, 0x20b)
        struct.pack_into("<II", pe, optional + 32, 4096, 512)
        struct.pack_into("<II", pe, optional + 56, 8192, 4096)
        struct.pack_into("<HH", pe, optional + 68, 17, 0x1c0)
        struct.pack_into("<I", pe, optional + 108, 16)
        struct.pack_into("<IIII", pe, optional + 240 + 8, 4096, 4096, 4096, 4096)
        struct.pack_into("<I", pe, optional + 240 + 36, 0x40000000)
        wrapped = self.root / "arbitrary-name.bin"
        wrapped.write_bytes(pe)
        catalog = binary_file(wrapped)
        self.assertEqual((catalog["kind"], catalog["subsystem"], catalog["code_bytes"], catalog["force_integrity"]),
                         ("Xbox code catalog", 17, 0, True))
        struct.pack_into("<H", pe, optional + 68, 3)
        other = self.root / "Catalog000.bin"
        other.write_bytes(pe)
        self.assertEqual(binary_file(other)["kind"], "PE image")


class SymbolLookup(unittest.TestCase):
    def setUp(self):
        tmp = TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.path = Path(tmp.name) / "game-Symbols.bin"
        module = b"game.exe\0"
        self.header = struct.pack("<II", 1, len(module)) + module + bytes(8) + struct.pack("<I", 2)
        self.ranges = struct.pack("<6I", 0x1100, 8, 0, 0x1200, 3, 0)
        self.names = struct.pack("<II2sI2s", 2, 2, b"A\0", 2, b"B\0")

    def write_symbols(self, raw, *, raw_size=None, extra=b""):
        packed = zlib.compress(raw) + extra
        size = len(raw) if raw_size is None else raw_size
        self.path.write_bytes(struct.pack("<II", size, len(packed)) + packed)

    def test_symbol_header_and_unknown_payload(self):
        path = self.path
        self.write_symbols(self.header + self.ranges + self.names)
        parsed = binary_file(path)
        self.assertEqual((parsed["kind"], parsed["module"], parsed["symbol_count"]),
                         ("Unreal symbol database", "game.exe", 2))
        path.write_bytes(path.read_bytes()[:-1])
        self.assertEqual(binary_file(path)["kind"], "opaque binary")
        with self.assertRaisesRegex(ValueError, "Compressed symbol size"):
            lookup_symbols(path, [0x1100])

    def test_rva_lookup_preserves_order_and_half_open_ranges(self):
        self.write_symbols(self.header + self.ranges + self.names)
        data = lookup_symbols(self.path, iter([0x1201, 0x1108, 0x1100, 0x1201]))
        self.assertEqual(data["lookups"], [
            {"rva": "0x1201", "status": "matched", "function": "B", "function_rva": "0x1200", "offset": "0x1"},
            {"rva": "0x1108", "status": "unmapped"},
            {"rva": "0x1100", "status": "matched", "function": "A", "function_rva": "0x1100", "offset": "0x0"},
        ])
        self.write_symbols(self.header + self.ranges[:12] * 2 + self.names)
        self.assertEqual(lookup_symbols(self.path, [0x1101])["lookups"], [{"rva": "0x1101", "status": "ambiguous"}])

    def test_rejects_invalid_tables_labels_and_zlib_lengths(self):
        raw = self.header + self.ranges + self.names
        cases = [
            (self.header + self.ranges + struct.pack("<I", 1) + self.names[4:], {}, "counts differ"),
            (self.header + self.ranges + self.names[:10] + struct.pack("<I", 0xffffffff) + b"B\0", {},
             "name length"),
            (raw[:-1] + b"!", {}, "name terminator"),
            (raw, {"extra": b"trailing"}, "length"),
            (raw, {"raw_size": len(raw) + 5}, "length"),
            (raw, {"raw_size": (1 << 30)}, "limit"),
        ]
        for contents, kwargs, error in cases:
            with self.subTest(error=error, kwargs=kwargs):
                self.write_symbols(contents, **kwargs)
                with self.assertRaisesRegex(ValueError, error):
                    lookup_symbols(self.path, [0x1100])
        self.write_symbols(raw)
        corrupted = bytearray(self.path.read_bytes())
        corrupted[-1] ^= 1
        self.path.write_bytes(corrupted)
        with self.assertRaisesRegex(ValueError, "compression"):
            lookup_symbols(self.path, [0x1100])
