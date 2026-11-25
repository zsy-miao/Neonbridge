"""Mirror a game's output to its console and a persistent launch log."""
from contextlib import contextmanager
from datetime import datetime, timezone
import os
from pathlib import Path
import selectors
import subprocess
import sys
import tempfile
from threading import Lock


def log_directory():
    if os.name == "nt":
        return Path(os.getenv("LOCALAPPDATA") or Path.home() / "AppData/Local") / "TuxBox/logs"
    return Path(os.getenv("XDG_STATE_HOME") or Path.home() / ".local/state") / "project-tuxbox/logs"


@contextmanager
def game_log(executable, path=None):
    if path is None:
        directory = log_directory()
        directory.mkdir(parents=True, exist_ok=True)
        stamp = datetime.now(timezone.utc).strftime("%Y%m%d-%H%M%SZ")
        fd, name = tempfile.mkstemp(dir=directory, prefix=f"{executable.stem}-{stamp}-", suffix=".log")
        try:
            stream = os.fdopen(fd, "wb")
        except BaseException:
            os.close(fd)
            try:
                os.unlink(name)
            except OSError:
                pass
            raise
        path = Path(name)
    else:
        path = path.expanduser().resolve()
        if path == executable.resolve() or (path.exists() and executable.exists() and path.samefile(executable)):
            raise ValueError("Choose a log file separate from the game executable")
        path.parent.mkdir(parents=True, exist_ok=True)
        stream = path.open("ab")
    with stream:
        log = GameLog(path, stream)
        log.status(f"Game log: {path}")
        try:
            yield log
        except (ValueError, OSError, subprocess.SubprocessError) as exc:
            # The CLI error handler also reports this to the terminal.
            log.status(f"Launch failed: {exc}", console=False)
            raise


class GameLog:
    def __init__(self, path, stream):
        self.path = path
        self.stream = stream
        self.stdout = sys.stdout
        self.stderr = sys.stderr
        self.lock = Lock()
        self.line_ended = stream.tell() == 0

    def _write(self, data, console, *, separate=False):
        with self.lock:
            if separate and not self.line_ended:
                data = b"\n" + data
            self.stream.write(data)
            self.stream.flush()
            self.line_ended = data.endswith(b"\n")
            if console is not None:
                try:
                    if hasattr(console, "buffer"):
                        console.buffer.write(data)
                    else:
                        console.write(data.decode("utf-8", errors="replace"))
                    console.flush()
                except BrokenPipeError:
                    pass  # A closed console consumer can still read the saved log.

    def status(self, message, *, console=True):
        stamp = datetime.now(timezone.utc).isoformat(timespec="seconds")
        self._write(f"[TuxBox {stamp}] {message}\n".encode("utf-8", errors="backslashreplace"),
                    self.stderr if console else None, separate=True)

    def stderr_bytes(self, data):
        self._write(data, self.stderr)

    # Shader preparation uses print(..., file=sys.stderr).
    def write(self, message):
        if message:
            self.stderr_bytes(message.encode("utf-8", errors="backslashreplace"))
        return len(message)

    def flush(self):
        pass  # Each write flushes both destinations.

    def run(self, command, *, cwd, env, stderr_bytes=None):
        with subprocess.Popen(command, cwd=cwd, env=env, stdout=subprocess.PIPE,
                              stderr=subprocess.PIPE, bufsize=0) as process:
            streams = ((process.stdout, lambda data: self._write(data, self.stdout)),
                       (process.stderr, self.stderr_bytes if stderr_bytes is None else stderr_bytes))
            try:
                if os.name == "nt":
                    self._drain_windows(process, streams)
                else:
                    self._drain(process, streams)
                return subprocess.CompletedProcess(command, process.wait())
            except BaseException:
                process.kill()
                raise

    @staticmethod
    def _drain_windows(process, streams):
        import ctypes
        from ctypes import wintypes
        import msvcrt

        peek = ctypes.WinDLL("kernel32", use_last_error=True).PeekNamedPipe
        peek.argtypes = (wintypes.HANDLE, wintypes.LPVOID, wintypes.DWORD,
                         wintypes.LPDWORD, wintypes.LPDWORD, wintypes.LPDWORD)
        peek.restype = wintypes.BOOL
        active = {pipe: (msvcrt.get_osfhandle(pipe.fileno()), write) for pipe, write in streams}
        available = wintypes.DWORD()
        while active:
            exited = process.poll() is not None
            received = False
            for pipe, (handle, write) in tuple(active.items()):
                if not peek(handle, None, 0, None, ctypes.byref(available), None):
                    error = ctypes.get_last_error()
                    if error in (109, 233):  # ERROR_BROKEN_PIPE or ERROR_PIPE_NOT_CONNECTED
                        del active[pipe]
                        continue
                    raise ctypes.WinError(error)
                # A helper can inherit the write handle. At exit drain the queued
                # bytes once without waiting for that helper to close the pipe.
                remaining = available.value if exited else min(available.value, 65536)
                while remaining:
                    data = pipe.read(min(remaining, 65536))
                    if not data:
                        del active[pipe]
                        break
                    write(data)
                    received = True
                    remaining -= len(data)
            if exited:
                return
            if active and not received:
                try:
                    process.wait(timeout=0.05)
                except subprocess.TimeoutExpired:
                    pass

    @staticmethod
    def _drain(process, streams):
        import fcntl

        with selectors.DefaultSelector() as ready:
            for pipe, write in streams:
                ready.register(pipe, selectors.EVENT_READ, write)
            while ready.get_map():
                exited = process.poll() is not None
                for key, _ in ready.select(0 if exited else 0.2):
                    # One unbuffered read of the full Linux pipe capacity covers
                    # all bytes pending at game exit, even if a helper keeps writing.
                    limit = fcntl.fcntl(key.fileobj, fcntl.F_GETPIPE_SZ) if exited else 65536
                    data = key.fileobj.read(limit)
                    if data:
                        key.data(data)
                    else:
                        ready.unregister(key.fileobj)
                if exited:
                    return
