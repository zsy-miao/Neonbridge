import os
from pathlib import Path
import tempfile
import unittest

from types import SimpleNamespace

from project_tuxbox.inspect import import_symbol, launch_target, manifest, member, package_root


class PackagePaths(unittest.TestCase):
    def setUp(self):
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.root = Path(scratch.name).resolve()
        (self.root / "BiN").mkdir()
        (self.root / "BiN" / "game.exe").touch()
        (self.root / "MicrosoftGame.Config").write_text(
            '<Game xmlns="urn:test"><Identity Name="fixture"/><ExecutableList>'
            '<Executable Name="bin\\GAME.EXE" Id="one" TargetDeviceFamily="Scarlett"/>'
            '<Executable Name="bin\\Absent.exe" Id="two"/></ExecutableList></Game>')

    def test_case_insensitive_nested_declaration_and_missing_entry(self):
        _, identity, rows = manifest(self.root)
        self.assertEqual(identity.get("Name"), "fixture")
        self.assertEqual(rows[0], ("bin\\GAME.EXE", "one", "Scarlett", self.root / "BiN/game.exe"))
        self.assertIsNone(rows[1][-1])
        for app, reason in [(None, "Select one"), ("two", "missing"), ("unknown", "Select one")]:
            with self.assertRaisesRegex(ValueError, reason):
                launch_target(self.root, app)

    def test_package_root_follows_the_declaring_manifest_at_any_depth(self):
        deep = self.root / "Mount/Title/Binaries/XSX/Shipping"
        deep.mkdir(parents=True)
        (deep / "x.exe").touch()
        (self.root / "MicrosoftGame.Config").write_text(
            '<Game><ExecutableList><Executable Name="Mount\\Title\\Binaries\\XSX\\Shipping\\x.exe" Id="x"/>'
            '</ExecutableList></Game>')
        # A nested manifest that does not declare the executable is not its package.
        (deep.parent / "MicrosoftGame.config").write_text(
            '<Game><ExecutableList><Executable Name="other.exe" Id="o"/></ExecutableList></Game>')
        exe = deep / "x.exe"
        self.assertEqual(package_root(self.root, exe), self.root)
        self.assertEqual(package_root(exe, exe), self.root)
        loose = self.root / "BiN/game.exe"
        self.assertIsNone(package_root(loose, loose))

    def test_absolute_parent_and_drive_relative_names(self):
        for name in ["../file", "x/../../file", "C:\\Windows\\x", "D:name", "\\\\server\\share\\x", ""]:
            with self.subTest(name=name), self.assertRaises(ValueError):
                member(self.root, name)

    def test_unresolved_root_and_symlink_escape(self):
        candidates = [self.root / "BiN" / ".."]
        if self.root.drive.casefold() == Path.cwd().drive.casefold():
            candidates.append(Path(os.path.relpath(self.root)))
        for unresolved in candidates:
            with self.subTest(root=unresolved):
                self.assertEqual(member(unresolved, "bin\\GAME.EXE"), self.root / "BiN/game.exe")
                _, identity, rows = manifest(unresolved)
                self.assertEqual(identity.get("Name"), "fixture")
                self.assertEqual(rows[0][-1], self.root / "BiN/game.exe")
        with tempfile.TemporaryDirectory() as outside:
            escaped = Path(outside).resolve() / "outside.exe"
            escaped.touch()
            link = self.root / "BiN" / "escape.exe"
            try:
                link.symlink_to(escaped)
            except OSError:
                pass
            else:
                with self.assertRaisesRegex(ValueError, "leaves the package directory"):
                    member(candidates[0], "bin\\escape.exe")

    def test_case_collision(self):
        (self.root / "BIN").mkdir(exist_ok=True)
        if len([p for p in self.root.iterdir() if p.name.casefold() == "bin"]) < 2:
            self.skipTest("case-insensitive filesystem")
        with self.assertRaisesRegex(ValueError, "Ambiguous"):
            member(self.root, "bin/game.exe")


class ImportIdentity(unittest.TestCase):
    def test_loader_ordinal_survives_friendly_name(self):
        entry = SimpleNamespace(import_by_ordinal=True, ordinal=4, name=b"SysAllocStringLen")
        self.assertEqual(import_symbol(entry), "#4")

    def test_named_import(self):
        entry = SimpleNamespace(import_by_ordinal=False, ordinal=None, name=b"ReadFile")
        self.assertEqual(import_symbol(entry), "ReadFile")
