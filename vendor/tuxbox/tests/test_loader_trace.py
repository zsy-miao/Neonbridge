from pathlib import Path, PureWindowsPath
from tempfile import TemporaryDirectory
import unittest

from project_tuxbox.inspect import digest
from tools.check_loader_trace import attempt, exception_thread, graphics_device_loss, title_exception


class LoaderTraceTests(unittest.TestCase):
    def test_bounded_run_requires_map_load_on_the_title_thread(self):
        with TemporaryDirectory() as folder:
            root = Path(folder)
            guest = root / "Guest.exe"
            guest.write_bytes(b"guest fixture")
            trace = root / "trace.log"
            header = (f"Command: {[str(guest)]!r}\nProton: test\n"
                      f'123.000:0144:trace:loaddll:build_module Loaded L"S:\\{root.name}\\Guest.exe" at 1000: native\n')
            event = '123.001:0144:warn:seh:OutputDebugStringA "LogLoad: Took 2.0 seconds to LoadMap(/Game/Test)"'
            trace.write_text(header + event)
            result = attempt(trace, guest, digest(guest), 124, allow_timeout=True)
            self.assertEqual(result["loaded_map"], "/Game/Test")
            self.assertFalse(result["frame_verified"])
            with self.assertRaises(ValueError):
                attempt(trace, guest, digest(guest), 124)
            for tail in [event.replace("0144", "0150"), event + "\nwine: Unhandled page fault on read access"]:
                trace.write_text(header + tail)
                with self.assertRaises(ValueError):
                    attempt(trace, guest, digest(guest), 124, allow_timeout=True)

    def test_graphics_device_loss_belongs_to_the_title(self):
        game = ("123.000:0140:0144:info:vkd3d-proton:vkd3d_instance_apply_application_workarounds: "
                "Detected game Guest.exe, adding shader quirks for specific shaders.")
        removed = ('124.000:0140:0234:warn:vkd3d-proton:d3d12_device_mark_as_removed: '
                   'Device 00001000 is lost (reason 0x887a0005, "VK_ERROR_DEVICE_LOST").')
        self.assertEqual(graphics_device_loss(game + "\n" + removed, "0144", "Guest.exe")["hresult"], "0x887a0005")
        for trace in (removed, removed + "\n" + game, game + "\n" + removed.replace("0140", "0220")):
            self.assertIsNone(graphics_device_loss(trace, "0144", "Guest.exe"))

    def test_debugger_worker_is_owned_by_the_exact_staged_process(self):
        table = ("wine: Unhandled page fault on read access to 0000 at address 1042 "
                 "(thread 0254), starting debugger...\nThreads:\n00000030 (D) G:\\Game\\Guest.exe\n"
                 "\t00000144   0\n\t00000254   2 Render\n\t00000258   2 <==\n00000200 helper.exe\n")
        target = PureWindowsPath(r"G:\Game\Guest.exe")
        self.assertEqual(exception_thread(table, target, "0144"), "0254")
        self.assertEqual(exception_thread(table.replace(r"G:\Game", r"G:\Other"), target, "0144"), "0144")
        self.assertEqual(exception_thread(table.replace("(D) ", ""), target, "0144"), "0144")

    event = "123.000:0144:trace:seh:dispatch_exception code=c0000005 (EXCEPTION_ACCESS_VIOLATION) flags=0 addr=1042"
    frame = '123.000:0144:warn:seh:virtual_unwind backtrace: 1042: L"Guest.exe" + 0042.'
    terminal = "wine: Unhandled page fault on read access to 0000 at address 1042 (thread 0144), starting debugger..."

    def classify(self, *lines):
        return title_exception("\n".join(lines), "0144", "Guest.exe", 0x1000)

    def test_title_fatal_requires_its_diagnostic_exception_and_worker(self):
        fatal = r'123.000:0144:warn:seh:OutputDebugStringW L"Fatal error: [Line: 860] \nAllocateTexture failed \n at Render.cpp:5"...'
        event = self.event.replace("c0000005", "1")
        raised = "wine: Unhandled exception 0x00000001 in thread 144 at address 1042 (thread 0144), starting debugger..."
        failure = self.classify(fatal, event, self.frame, raised)
        self.assertEqual(failure["kind"], "title-fatal-error")
        self.assertEqual(failure["title_diagnostic"], "AllocateTexture failed")
        for lines in [(event, self.frame, raised), (fatal, event, raised),
                      (fatal, event, self.frame, raised.replace("in thread 144", "in thread 188"))]:
            with self.subTest(lines=lines), self.assertRaises(ValueError):
                self.classify(*lines)

    def test_terminal_access_violation_needs_its_own_title_frame(self):
        result = self.classify(self.event, self.frame, self.terminal)
        self.assertEqual(result["kind"], "access-violation")
        self.assertEqual(result["title_frame_rva"], "0x42")
        dbg_print = self.event.replace("c0000005", "4001000a").replace("addr=1042", "addr=abcd")
        self.assertEqual(self.classify(self.event, self.frame, dbg_print, self.terminal), result)
        with self.assertRaises(ValueError):
            self.classify(self.event, dbg_print, self.frame, self.terminal)
        fatal = r'123.000:0144:warn:seh:OutputDebugStringW L"Fatal error: [Line: 97] \nTest runtime failed\n"'
        helper = fatal.replace("0144", "0188").replace("Test runtime failed", "Unrelated helper error")
        self.assertNotIn("title_diagnostic", self.classify(helper, self.event, self.frame, self.terminal))
        self.assertEqual(self.classify(fatal, helper, self.event, self.frame, self.terminal)["title_diagnostic"],
                         "Test runtime failed")
        for lines in [(self.event, self.frame),
                      (self.event, self.frame.replace("0144", "0150"), self.terminal),
                      (self.event, self.frame, self.terminal.replace("thread 0144", "thread 0150")),
                      (self.event, self.frame.replace("+ 0042", "+ 0043"), self.terminal),
                      (self.event, self.frame, self.event, self.terminal)]:
            with self.subTest(lines=lines), self.assertRaises(ValueError):
                self.classify(*lines)

    def test_isolated_provider_dependencies_and_case_sensitive_host_file(self):
        with TemporaryDirectory() as folder:
            root = Path(folder)
            guest = root / "Guest.exe"
            guest.write_bytes(b"test fixture")
            provider = root / "XcRdApI.DlL"
            provider.write_bytes(b"mixed-case on the host")
            media_provider = root / "mfplat_sys.dll"
            media_provider.write_bytes(b"Proton media provider loaded from the title directory")
            title = rf"S:\{root.name}\Guest.exe"
            dll = rf"S:\{root.name}\XCRDAPI.dll"
            system32 = root / "system32"
            system32.mkdir()
            runtime = system32 / "XGameRuntime.dll"
            runtime.write_bytes(b"system runtime")
            trace = root / "trace.log"
            trace.write_text("\n".join([
                f"Command: {[str(guest)]!r}", "Proton: test",
                f'123.000:0144:trace:loaddll:build_module Loaded L"{title}" at 1000: native',
                f'123.000:0144:trace:loaddll:build_module Loaded L"{dll}" at 2000: native',
                rf'123.000:0144:trace:loaddll:build_module Loaded L"S:\{root.name}\mfplat_sys.dll" at 3000: builtin',
                f'123.000:0144:warn:module:import_dll No implementation for dep.a imported from L"{dll}"',
                f'123.000:0144:warn:module:import_dll No implementation for xmem.dll.work imported from L"{title}"',
                f'123.000:0188:warn:module:import_dll No implementation for other.work imported from L"{title}"',
                r'123.000:0144:trace:loaddll:build_module Loaded L"C:\WINDOWS\System32\xgameruntime.dll" at abcd: native',
                r'123.000:0144:trace:loaddll:build_module Loaded L"C:\other\GameConfigHelper.dll" at abba: native',
                r'123.000:0188:trace:loaddll:build_module Loaded L"C:\windows\system32\GameConfigHelper.dll" at ffff: native',
                self.event.replace("c0000005", "80000100"), self.frame,
                "wine: Unimplemented function xmem.dll.work called at address 1042 (thread 0144), starting debugger...",
            ]))
            result = attempt(trace, guest, digest(guest), 1, system32)
            self.assertEqual(result["stage"], "title-code")
            self.assertEqual(result["app_local_provider_sha256"],
                             {"xcrdapi.dll": digest(provider), "mfplat_sys.dll": digest(media_provider)})
            self.assertEqual(result["unresolved_provider_symbols"], {"xcrdapi.dll": ["dep.a"]})
            self.assertEqual(result["unresolved_direct_symbols"], ["xmem.dll.work"])
            self.assertFalse(result["frame_verified"])
            self.assertEqual(result["prefix_system32_provider_sha256"], {"xgameruntime.dll": digest(runtime)})
