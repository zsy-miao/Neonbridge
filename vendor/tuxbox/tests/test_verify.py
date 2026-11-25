import unittest
from unittest import mock

from pathlib import Path
import struct
import tempfile

from project_tuxbox.verify import (
    lanes_imports, missing_provider_imports, private_x_server, screen_capture_problem, xauthority_entry)


class PrivateXServerTests(unittest.TestCase):
    def test_cookie_record_matches_any_host_and_display(self):
        cookie = bytes(range(16))
        record = xauthority_entry(cookie)
        self.assertEqual(struct.unpack(">H", record[:2])[0], 0xFFFF)  # FamilyWild
        fields, at = [], 2
        while at < len(record):
            (size,) = struct.unpack(">H", record[at:at + 2])
            fields.append(record[at + 2:at + 2 + size])
            at += 2 + size
        self.assertEqual(fields, [b"", b"", b"MIT-MAGIC-COOKIE-1", cookie])

    def test_no_server_without_a_wayland_session_or_xwayland(self):
        with tempfile.TemporaryDirectory() as directory, \
             mock.patch("project_tuxbox.verify.sys.platform", "linux"), \
             mock.patch("project_tuxbox.verify.subprocess.Popen") as start:
            with mock.patch.dict("project_tuxbox.verify.os.environ", {}, clear=True), \
                 mock.patch("project_tuxbox.verify.shutil.which", return_value="/usr/bin/Xwayland"):
                with private_x_server(directory) as server:
                    self.assertIsNone(server)
            with mock.patch.dict("project_tuxbox.verify.os.environ", {"WAYLAND_DISPLAY": "wayland-0"}), \
                 mock.patch("project_tuxbox.verify.shutil.which", return_value=None):
                with private_x_server(directory) as server:
                    self.assertIsNone(server)
            start.assert_not_called()


class ScreenCaptureTests(unittest.TestCase):
    def test_only_an_x_display_on_linux_is_probed(self):
        with mock.patch("project_tuxbox.verify.ctypes.CDLL") as load:
            with mock.patch("project_tuxbox.verify.sys.platform", "win32"):
                self.assertIsNone(screen_capture_problem(display=":0"))
            with mock.patch("project_tuxbox.verify.sys.platform", "linux"):
                self.assertIsNone(screen_capture_problem(display=""))
            load.assert_not_called()

    def test_a_missing_xlib_leaves_the_guest_check_to_report(self):
        with mock.patch("project_tuxbox.verify.sys.platform", "linux"):
            self.assertIsNone(screen_capture_problem(display=":0", library="/nonexistent/libX11.so"))


class LanesImportsTests(unittest.TestCase):
    def test_game_imports_and_shared_os_services(self):
        pe = {"imports": [{"module": "d3d12_xs.dll"}, {"module": "api-ms-win-core-synch-l1-2-0.dll"}],
              "delay_imports": [{"module": "GameInput.DLL"}]}
        self.assertEqual(lanes_imports(pe), ["api-ms-win-core-synch-l1-2-0.dll", "d3d12_xs.dll", "gameinput.dll"])

    def test_desktop_bypasses_fail_in_either_table(self):
        for table in ("imports", "delay_imports"):
            for desktop in ("d3d12.dll", "dxgi.dll", "GDI32.dll", "USER32.dll", "XInput1_4.dll"):
                with self.subTest(table=table, dll=desktop):
                    pe = {"imports": [], "delay_imports": []}
                    pe[table].append({"module": desktop})
                    with self.assertRaisesRegex(ValueError, "Xbox title profile"):
                        lanes_imports(pe)


class ProviderImportTests(unittest.TestCase):
    def test_named_and_ordinal_expectations_match_without_export_lookup(self):
        modules = [
            {"module": "xmem.dll", "symbols": ["XMemAllocDefault", "XMemFreeDefault"]},
            {"module": "GameInput.dll", "symbols": ["#1"]},
        ]
        with mock.patch("project_tuxbox.verify.export_table") as exports:
            self.assertEqual(
                missing_provider_imports(modules, Path("xmem.dll"), ("XMemAllocDefault", "XMemFreeDefault")), [])
            self.assertEqual(missing_provider_imports(modules, Path("GameInput.dll"), ("#1",)), [])
        exports.assert_not_called()

    def test_ordinal_imports_resolve_against_provider_export_table(self):
        modules = [
            {"module": "XMEM.DLL", "symbols": ["#2", "#7"]},
            {"module": "d3d12_xs.dll", "symbols": ["#10", "#13"]},
        ]
        tables = {
            "xmem.dll": ({1: {"XMemAlloc"}, 2: {"XMemAllocDefault"}, 7: {"XMemFreeDefault"}}, set()),
            "d3d12_xs.dll": ({10: {"D3D12SerializeRootSignature"}, 13: {"D3D12XboxCreateDevice"}}, set()),
        }
        with mock.patch("project_tuxbox.verify.export_table", side_effect=lambda path: tables[path.name]):
            self.assertEqual(
                missing_provider_imports(modules, Path("xmem.dll"), ("XMemAllocDefault", "XMemFreeDefault")), [])
            self.assertEqual(
                missing_provider_imports(
                    modules, Path("d3d12_xs.dll"), ("D3D12SerializeRootSignature", "D3D12XboxCreateDevice")),
                [])

    def test_wrong_or_unknown_ordinals_and_missing_explicit_ordinals_fail(self):
        modules = [
            {"module": "xmem.dll", "symbols": ["#1", "#99"]},
            {"module": "GameInput.dll", "symbols": ["GameInputCreate"]},
        ]
        with mock.patch("project_tuxbox.verify.export_table", return_value=({1: {"XMemAlloc"}}, set())):
            self.assertEqual(
                missing_provider_imports(modules, Path("xmem.dll"), ("XMemAllocDefault", "XMemFreeDefault")),
                ["XMemAllocDefault", "XMemFreeDefault"])
        self.assertEqual(missing_provider_imports(modules, Path("GameInput.dll"), ("#1",)), ["#1"])
