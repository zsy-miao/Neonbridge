import io
import os
from pathlib import Path
import sys
from tempfile import TemporaryDirectory
import time
import unittest
from unittest.mock import patch
from concurrent.futures import ThreadPoolExecutor

from project_tuxbox.game_log import GameLog, game_log


class GameLogs(unittest.TestCase):
    def test_both_console_streams_and_log_update_before_exit(self):
        with TemporaryDirectory() as tmp:
            directory = Path(tmp)
            path = directory / "logs with spaces" / "game.log"
            stdout = io.TextIOWrapper(io.BytesIO())
            stderr = io.TextIOWrapper(io.BytesIO())
            gate = directory / "continue"
            child = """\
import os, pathlib, sys, time
os.write(1, b"ready-stdout")
os.write(2, b"ready-stderr")
end = time.monotonic() + 8
while not pathlib.Path(sys.argv[1]).exists() and time.monotonic() < end:
    time.sleep(0.01)
os.write(1, bytes([255]) * 200000)
os.write(2, b"last error")
sys.exit(17)
"""
            with patch("sys.stdout", stdout), patch("sys.stderr", stderr), \
                 game_log(Path(sys.executable), path) as log, ThreadPoolExecutor(1) as pool:
                run = pool.submit(log.run, [sys.executable, "-c", child, str(gate)], cwd=directory, env=None)
                try:
                    deadline = time.monotonic() + 5
                    while time.monotonic() < deadline:
                        if (b"ready-stdout" in stdout.buffer.getvalue() and
                                b"ready-stderr" in stderr.buffer.getvalue()):
                            break
                        time.sleep(0.01)
                    self.assertIn(b"ready-stdout", stdout.buffer.getvalue())
                    self.assertIn(b"ready-stderr", stderr.buffer.getvalue())
                    for marker in (b"ready-stdout", b"ready-stderr"):
                        self.assertIn(marker, path.read_bytes())
                    self.assertFalse(run.done())
                finally:
                    gate.touch()
                self.assertEqual(run.result(timeout=5).returncode, 17)
            self.assertEqual(stdout.buffer.getvalue(), b"ready-stdout" + b"\xff" * 200000)
            self.assertNotIn(b"ready-stdout", stderr.buffer.getvalue())
            saved = path.read_bytes()
            self.assertEqual(saved.count(b"\xff"), 200000)
            self.assertIn(b"last error", saved)
            with patch("sys.stderr", io.StringIO()), game_log(Path(sys.executable), path):
                pass
            self.assertTrue(path.read_bytes().startswith(saved + b"\n"))

    def test_default_logs_are_unique_and_reported(self):
        with TemporaryDirectory() as tmp, patch("sys.stderr", new_callable=io.StringIO) as terminal, \
             patch("project_tuxbox.game_log.log_directory", return_value=Path(tmp)):
            paths = []
            for _ in range(2):
                with game_log(Path("Lanes.exe")) as log:
                    paths.append(log.path)
                    self.assertIn(str(log.path), terminal.getvalue())
            self.assertNotEqual(*paths)
            self.assertTrue(all(p.name.startswith("Lanes-") and p.is_file() for p in paths))

    def test_helper_holding_pipes_does_not_delay_exit(self):
        with TemporaryDirectory() as tmp:
            directory = Path(tmp)
            gate = directory / "continue"
            helper = ("import pathlib,sys,time\nend=time.monotonic()+10\n"
                      "while not pathlib.Path(sys.argv[1]).exists() and time.monotonic()<end: time.sleep(0.05)")
            child = ("import os,subprocess,sys\nsubprocess.Popen([sys.executable,'-c',sys.argv[1],sys.argv[2]],"
                     "stdout=sys.stdout,stderr=sys.stderr,cwd=os.path.dirname(sys.executable))\n"
                     "print('done', flush=True)")
            with patch("sys.stdout", io.StringIO()), patch("sys.stderr", io.StringIO()), \
                 game_log(Path(sys.executable), directory / "game.log") as log, ThreadPoolExecutor(1) as pool:
                run = pool.submit(log.run, [sys.executable, "-c", child, helper, str(gate)], cwd=directory, env=None)
                try:
                    self.assertEqual(run.result(timeout=3).returncode, 0)
                    self.assertIn(b"done", log.path.read_bytes().splitlines())
                finally:
                    gate.touch()

    def test_failed_stderr_consumer_interrupts_running_child(self):
        child = """\
import os, pathlib, sys, time
os.write(2, b"marker")
end = time.monotonic() + 10
while not pathlib.Path(sys.argv[1]).exists() and time.monotonic() < end:
    time.sleep(.05)
"""
        with TemporaryDirectory() as tmp, ThreadPoolExecutor(1) as pool:
            gate = Path(tmp) / "finish"
            log = GameLog(Path(tmp) / "game.log", io.BytesIO())

            def fail(data):
                self.assertEqual(data, b"marker")
                raise OSError("log volume full")

            run = pool.submit(log.run, [sys.executable, "-c", child, str(gate)],
                              cwd=tmp, env=None, stderr_bytes=fail)
            try:
                with self.assertRaisesRegex(OSError, "log volume full"):
                    run.result(timeout=4)
            finally:
                gate.touch()

    def test_game_log_closes_fd_and_unlinks_on_fdopen_error(self):
        closed = []
        real_close = os.close

        def track_close(fd):
            closed.append(fd)
            real_close(fd)

        with TemporaryDirectory() as tmp, \
             patch("project_tuxbox.game_log.log_directory", return_value=Path(tmp)), \
             patch("os.fdopen", side_effect=OSError("fdopen failed")), \
             patch("os.close", side_effect=track_close):
            with self.assertRaises(OSError):
                with game_log(Path("game.exe")):
                    pass
        self.assertTrue(len(closed) > 0)
        self.assertEqual(list(Path(tmp).glob("*.log")), [])
