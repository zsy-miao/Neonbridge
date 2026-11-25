import os
import sys
from pathlib import Path
import tempfile
from threading import Event
import unittest

from project_tuxbox.backend_log import check_log_path, game_diagnostics


class BackendDiagnostics(unittest.TestCase):
    def test_default_console_filters_only_paired_messages_across_reads(self):
        wide = b'012c:warn:seh:OutputDebugStringW L"wide\\n"\n'
        wide_echo = b'012c:warn:seh:dispatch_exception L"wide\\n"\n'
        ansi_copy = b'012c:warn:seh:OutputDebugStringA "wide\\n"\n'
        ansi_echo = b'012c:warn:seh:dispatch_exception "wide\\n"\n'
        ansi = b'012c:warn:debugstr:OutputDebugStringA "marker"\n'
        housekeeping = b'0148:trace:console:write_console_input 1\n'
        console = b'0148:trace:console:write_console L"guest wrote to stdout"\n'
        other_thread = b'00ab:warn:seh:dispatch_exception "marker"\n'
        fault = b'0024:warn:seh:dispatch_exception access violation\n'
        binary = b"ordinary stderr\xff\x00"
        partial = b"0024:warn:seh:dispatch_exc"
        chunks = []
        with game_diagnostics("proton", {}, ".", chunks.append) as write:
            stream = (wide + wide_echo + ansi_copy + ansi_echo + ansi
                      + b'012c:warn:seh:dispatch_exception "marker"\n'
                      + other_thread + housekeeping + console + fault)
            for offset in range(0, len(stream), 5):
                write(stream[offset:offset + 5])
            write(binary)
            self.assertTrue(b"".join(chunks).endswith(binary))
            write(b"\n" + partial)
        self.assertEqual(b"".join(chunks), wide + ansi + other_thread + console + fault + binary + b"\n" + partial)

    def test_long_partial_wine_message_and_raw_override(self):
        chunks = []
        line = b'0024:warn:seh:OutputDebugStringW L"' + b"x" * 65536
        with game_diagnostics("wine", {}, ".", chunks.append) as write:
            write(line)
            self.assertEqual(chunks, [line])
            write(b'"\n')
        with game_diagnostics("proton", {"WINEDEBUG": "+all"}, ".", chunks.append) as write:
            write(b"0024:trace:console:update_output 1\n")
        self.assertEqual(chunks[-1], b"0024:trace:console:update_output 1\n")

    def test_default_guest_channels_and_caller_overrides(self):
        for backend in ("wine", "proton"):
            with self.subTest(backend=backend):
                env = {"PROTON_LOG": "0"}
                with game_diagnostics(backend, env, ".", self.fail):
                    self.assertIn("warn+seh", env["WINEDEBUG"])
                    self.assertIn("warn+debugstr", env["WINEDEBUG"])
                    self.assertEqual("trace+console" in env["WINEDEBUG"], backend == "proton")
                overrides = {"WINEDEBUG": "-all", "VKD3D_DEBUG": "none", "DXVK_LOG_LEVEL": "debug"}
                env = overrides.copy()
                with game_diagnostics(backend, env, ".", self.fail):
                    self.assertEqual(env, overrides)

        env = {}
        with game_diagnostics("native", env, ".", self.fail):
            self.assertEqual(env, {})

    @unittest.skipUnless(sys.platform == "linux", "Proton raw log rotation requires POSIX unlink-while-open semantics")
    def test_proton_raw_log_arrives_live_and_drains_final_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "logs/steam-default.log"
            path.parent.mkdir()
            path.write_bytes(b"stale run\n")
            received = bytearray()
            live = Event()

            def write(data):
                received.extend(data)
                live.set()

            explicit = {"PROTON_LOG": "+loaddll", "PROTON_LOG_DIR": "logs"}
            env = explicit.copy()
            with game_diagnostics("proton", env, directory, write):
                self.assertEqual(env, explicit)
                path.unlink()
                with path.open("wb", buffering=0) as raw:
                    raw.write(b"first diagnostic")
                    self.assertTrue(live.wait(3))
                    self.assertEqual(received, b"first diagnostic")
                    raw.write(b"\n" + b"x" * 65536 + b"\xff")
            self.assertEqual(received, path.read_bytes())

    def test_proton_startup_failure_does_not_replay_previous_run(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "steam-default.log"
            path.write_bytes(b"previous run")
            with game_diagnostics("proton", {"PROTON_LOG": "1", "HOME": directory}, ".", self.fail):
                pass

    def test_reject_same_path_and_hard_link_before_opening_mirror(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "steam-default.log"
            env = {"PROTON_LOG": "1", "HOME": directory}
            with self.assertRaises(ValueError):
                check_log_path("proton", env, ".", path)
            path.write_bytes(b"previous run")
            alias = Path(directory) / "mirror.log"
            os.link(path, alias)
            with self.assertRaises(ValueError):
                check_log_path("proton", env, ".", alias)
            self.assertEqual(path.read_bytes(), b"previous run")
