import json
from pathlib import Path
import re
import sys
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

from project_tuxbox.inspect import digest
from project_tuxbox.shader_prep import matching_tuxbox_provider

if sys.platform != "linux":
    raise unittest.SkipTest("Frosty's staging and launch tools require Linux")

from tools.prepare_frosty import BIN, ENGINE_INI, TITLE, XG_PATH, prepare, refresh_xg, stage_xg_provider, update_provider_manifest, verify_staged_files


class FrostyPreparationTests(unittest.TestCase):
    def setUp(self):
        scratch = TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.work = Path(scratch.name)
        stage = self.work / "Mount"
        self.binaries = stage / "Frosty/Binaries/XSX"
        self.binaries.mkdir(parents=True)
        (self.work / "stage").write_text(str(stage) + "\n")
        self.records = {}
        originals = ("Frosty-XSX-Shipping.exe", "GameChat2.dll", "MSVCP140.dll",
                     "VCRUNTIME140.dll", "VCRUNTIME140_1.dll")
        for name in (*originals, "d3d12_xs.dll"):
            path = self.binaries / name
            path.write_bytes(name.encode())
            self.records[str(path.relative_to(self.work))] = {
                "origin": "Original retail package" if name in originals else "TuxBox d3d12",
                "sha256": digest(path),
            }
        self.provider = self.binaries / "d3d12_xs.dll"
        self.manifest = self.work / "providers.json"
        self.manifest.write_text(json.dumps(self.records))

    def test_refresh_retires_only_the_previously_staged_gameconfig(self):
        relative = "prefix/drive_c/windows/system32/gameconfighelper.dll"
        helper = self.work / relative
        helper.parent.mkdir(parents=True)
        for origin, modified, keep in (("TuxBox gameconfig", False, False),
                                       ("TuxBox gameconfig", True, True),
                                       ("Original retail package", False, True)):
            with self.subTest(origin=origin, modified=modified):
                helper.write_bytes(b"previous DLL")
                records = dict(self.records)
                records[relative] = {"origin": origin, "sha256": digest(helper)}
                self.manifest.write_text(json.dumps(records))
                if modified:
                    helper.write_bytes(b"replacement DLL")
                update_provider_manifest(self.work, self.records)
                self.assertEqual(helper.exists(), keep)
                self.assertEqual(json.loads(self.manifest.read_text()), self.records)

    def test_rejects_replacement_of_original_and_provider(self):
        verify_staged_files(self.work)
        for name in ("GameChat2.dll", "d3d12_xs.dll"):
            with self.subTest(name=name):
                path = self.binaries / name
                original = path.read_bytes()
                path.write_bytes(b"different DLL with the same name")
                with self.assertRaisesRegex(ValueError, re.escape(name)):
                    verify_staged_files(self.work)
                path.write_bytes(original)

    def test_rejects_preparation_record_missing_an_original(self):
        del self.records[str((self.binaries / "GameChat2.dll").relative_to(self.work))]
        self.manifest.write_text(json.dumps(self.records))
        with self.assertRaisesRegex(ValueError, "original retail binaries"):
            verify_staged_files(self.work)

    def test_preserves_existing_graphics_settings(self):
        staged = self.work / "Mount" / ENGINE_INI
        staged.parent.mkdir(parents=True, exist_ok=True)
        legacy = (Path(__file__).parent / "fixtures/frosty-legacy-user-engine.ini").read_bytes()
        for contents in (legacy, b"existing user settings"):
            staged.write_bytes(contents)
            verify_staged_files(self.work)
            self.assertEqual(staged.read_bytes(), contents)

    def test_staged_provider_survives_removal_of_shared_build(self):
        shared = self.work / "build/d3d12-smoke/d3d12_xs.dll"
        shared.parent.mkdir(parents=True)
        shared.write_bytes(self.provider.read_bytes())
        sha = digest(self.provider)
        with patch("project_tuxbox.shader_prep.TUXBOX_PROVIDER", shared):
            shared.unlink()
            self.assertTrue(matching_tuxbox_provider(self.provider, sha))

    def test_manifest_mismatch_takes_precedence_over_matching_shared_build(self):
        shared = self.work / "shared.dll"
        shared.write_bytes(self.provider.read_bytes())
        sha = digest(self.provider)
        relative = str(self.provider.relative_to(self.work))
        with patch("project_tuxbox.shader_prep.TUXBOX_PROVIDER", shared):
            for record in ({"origin": "TuxBox d3d12", "sha256": "0" * 64},
                           {"origin": "comparison provider", "sha256": sha}, None):
                with self.subTest(record=record):
                    self.manifest.write_text(json.dumps({relative: record} if record else {}))
                    self.assertFalse(matching_tuxbox_provider(self.provider, sha))

    def test_xg_staging_defaults_and_explicit_tuxbox_provenance(self):
        self.manifest.unlink()
        packaged = self.work / "os/Windows/System32/xg_xs.dll"
        packaged.parent.mkdir(parents=True)
        packaged.write_bytes(b"packaged XG")
        output = self.work / "build"
        built = output / "xg-provider/xg_xs.dll"
        built.parent.mkdir(parents=True)
        built.write_bytes(b"fresh TuxBox XG")
        with patch("tools.prepare_frosty.OUTPUT", output), patch("tools.prepare_frosty.build_target") as build:
            record = stage_xg_provider(object(), self.work / "os", self.work, self.binaries, use_tuxbox=False)
            build.assert_not_called()
            self.assertEqual(record, {"origin": "Packaged Xbox XG", "sha256": digest(packaged)})
            self.assertEqual((self.binaries / "xg_xs.dll").read_bytes(), b"packaged XG")
            record = stage_xg_provider(object(), self.work / "os", self.work, self.binaries, use_tuxbox=True)
            build.assert_called_once()
            self.assertEqual(record, {"origin": "TuxBox xg", "sha256": digest(built)})
            self.assertEqual((self.binaries / "xg_xs.dll").read_bytes(), b"fresh TuxBox XG")

    def test_existing_work_rejects_changed_provider_before_replacing_it(self):
        packaged = self.work / "os/Windows/System32/xg_xs.dll"
        packaged.parent.mkdir(parents=True)
        packaged.write_bytes(b"new provider")
        staged = self.binaries / "xg_xs.dll"
        staged.write_bytes(b"original provider")
        self.records[str(XG_PATH)] = {"origin": "Packaged Xbox XG", "sha256": digest(staged)}
        self.manifest.write_text(json.dumps(self.records))
        with self.assertRaisesRegex(ValueError, "XG provider changed"):
            stage_xg_provider(object(), self.work / "os", self.work, self.binaries, use_tuxbox=False)
        self.assertEqual(staged.read_bytes(), b"original provider")
        packaged.write_bytes(staged.read_bytes())
        staged.write_bytes(b"tampered provider")
        with self.assertRaisesRegex(ValueError, "differs from its preparation record"):
            stage_xg_provider(object(), self.work / "os", self.work, self.binaries, use_tuxbox=False)
        self.assertEqual(staged.read_bytes(), b"tampered provider")

    def test_verification_rejects_selection_provenance_mismatch(self):
        staged = self.binaries / "xg_xs.dll"
        staged.write_bytes(b"provider")
        self.records[str(XG_PATH)] = {"origin": "Packaged Xbox XG", "sha256": digest(staged)}
        self.manifest.write_text(json.dumps(self.records))
        (self.work / "inputs.json").write_text(json.dumps({"xg_provider": "tuxbox"}))
        with self.assertRaisesRegex(ValueError, "XG provider does not match"):
            verify_staged_files(self.work)
        self.records[str(XG_PATH)]["origin"] = "TuxBox xg"
        self.manifest.write_text(json.dumps(self.records))
        verify_staged_files(self.work)

    def test_reusing_work_cannot_change_xg_selection(self):
        source = self.work / "source"
        source_bin = source / BIN
        source_bin.mkdir(parents=True)
        for name in (TITLE, "GameChat2.dll", "MSVCP140.dll", "VCRUNTIME140.dll", "VCRUNTIME140_1.dll"):
            (source_bin / name).write_bytes(name.encode())
        plan = self.work / "plan.json"
        plan.write_text(json.dumps({"executable_sha256": digest(source_bin / TITLE)}))
        work = self.work / "existing"
        work.mkdir()
        settings = {"source": str(source), "packaged_os": str(self.work / "os"), "proton": str(self.work / "proton")}
        (work / "inputs.json").write_text(json.dumps(settings))
        with patch("tools.prepare_frosty.FROSTY_PLAN", plan), patch("tools.prepare_frosty.Toolchain") as toolchain:
            with self.assertRaisesRegex(ValueError, "XG selection"):
                prepare(source, self.work / "os", work, self.work / "proton", self.work / "dxc", use_tuxbox_xg=True)
            toolchain.assert_not_called()

    def test_explicit_xg_refresh_verifies_old_provider_and_updates_manifest(self):
        staged = self.binaries / "xg_xs.dll"
        staged.write_bytes(b"old TuxBox XG")
        self.records[str(XG_PATH)] = {"origin": "TuxBox xg", "sha256": digest(staged)}
        self.manifest.write_text(json.dumps(self.records))
        (self.work / "inputs.json").write_text(json.dumps({"xg_provider": "tuxbox"}))
        settings = self.work / "Mount" / ENGINE_INI
        settings.parent.mkdir(parents=True)
        settings.write_bytes(b"original graphics settings")
        output = self.work / "build"
        built = output / "xg-provider/xg_xs.dll"
        built.parent.mkdir(parents=True)
        built.write_bytes(b"new TuxBox XG")
        with patch("tools.prepare_frosty.OUTPUT", output), patch("tools.prepare_frosty.Toolchain"), \
                patch("tools.prepare_frosty.build_target") as build:
            staged.write_bytes(b"tampered")
            with self.assertRaisesRegex(ValueError, "Staged file differs"):
                refresh_xg(self.work)
            build.assert_not_called()
            staged.write_bytes(b"old TuxBox XG")
            refresh_xg(self.work)
            self.assertEqual(staged.read_bytes(), built.read_bytes())
            self.assertEqual(json.loads(self.manifest.read_text())[str(XG_PATH)]["sha256"], digest(built))
            self.assertEqual(settings.read_bytes(), b"original graphics settings")
            verify_staged_files(self.work)
            (self.work / "inputs.json").write_text("{}")
            with self.assertRaisesRegex(ValueError, "requires an existing TuxBox XG"):
                refresh_xg(self.work)
