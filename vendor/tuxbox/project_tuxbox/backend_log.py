"""Route Wine game diagnostics through the live session log."""
from concurrent.futures import ThreadPoolExecutor
from contextlib import ExitStack, contextmanager
import os
from pathlib import Path
import re
from threading import Event


# Proton sends a console game's stdio to its own conhost. Wine's console
# channel exposes those writes. Unicode debug strings use the SEH channel.
_WINE_CHANNELS = "-all,err+all,warn+debugstr,warn+seh"
_CONSOLE = b"trace:console:"
_DISPATCH = b"warn:seh:dispatch_exception"
_ANSI = b"warn:debugstr:OutputDebugStringA"
_WIDE = b"warn:seh:OutputDebugStringW"
_ANSI_FALLBACK = b"warn:seh:OutputDebugStringA"
_RECORDS = (_CONSOLE, _DISPATCH + b' "', _DISPATCH + b' L"', _ANSI + b' "',
            _WIDE + b' L"', _ANSI_FALLBACK + b' "')
_THREAD = re.compile(rb"[0-9a-fA-F]{1,8}")


class _WineMessages:
    """Hide Wine's internal echoes while retaining the game's original bytes."""

    def __init__(self, write_bytes):
        self.write_bytes = write_bytes
        self.pending = b""
        self.passthrough = False
        self.last_debug = None

    def keep(self, thread, body):
        function, _, payload = body.partition(b" ")
        current = (thread, payload)
        if function == _DISPATCH and current == self.last_debug:
            return False
        previous = self.last_debug
        self.last_debug = None
        if function in (_ANSI, _WIDE, _ANSI_FALLBACK) and payload.startswith((b'"', b'L"')):
            self.last_debug = current
            # Wine follows a wide OutputDebugString with an ANSI fallback.
            # Drop it only when the same thread just reported the same text.
            return function != _ANSI_FALLBACK or previous != (thread, b"L" + payload)
        return not body.startswith(_CONSOLE) or body.startswith(_CONSOLE + b"write_console ")

    def __call__(self, data):
        combined = self.pending + data
        self.pending = b""
        output = bytearray()
        start = 0
        while start < len(combined):
            newline = combined.find(b"\n", start)
            complete = newline >= 0
            end = newline + 1 if complete else len(combined)
            line = combined[start:end]
            start = end
            thread, colon, body = line.partition(b":")
            # A binary payload or a very long partial line must pass through
            # immediately. Buffer only a possible, short Wine record.
            candidate = not self.passthrough and len(line) <= 8192 and _THREAD.fullmatch(thread) and (
                not colon or (len(thread) >= 4 and any(
                    body.startswith(record) or record.startswith(body) for record in _RECORDS
                ))
            )
            if candidate and not complete:
                self.pending = line
                continue
            if not candidate:
                self.last_debug = None
            if not candidate or self.keep(thread, body):
                output.extend(line)
            self.passthrough = not complete
        if output:
            self.write_bytes(bytes(output))

    def finish(self):
        if self.pending:
            self.write_bytes(self.pending)


def _proton_log_path(backend, env, cwd):
    if backend != "proton" or env.get("PROTON_LOG", "") in ("", "0"):
        return None
    # Host uses umu's GAMEID=umu-default, which sets SteamGameId=default.
    directory = Path(env.get("PROTON_LOG_DIR", env.get("HOME", str(Path.home()))))
    return Path(cwd) / directory / "steam-default.log"


def check_log_path(backend, env, cwd, log_path):
    """Reject a mirror that aliases the file Proton replaces on startup."""
    raw = _proton_log_path(backend, env, cwd)
    if log_path is None or raw is None:
        return
    target = Path(log_path)
    if target.resolve() == raw.resolve() or (
        target.is_file() and raw.is_file() and target.samefile(raw)
    ):
        raise ValueError(f"Choose a TuxBox log file separate from Proton's {raw}")


def _open_log(path):
    try:
        return path.open("rb", buffering=0) if path.is_file() else None
    except FileNotFoundError:
        return None  # Proton can unlink the file between stat and open.


def _follow(path, previous, done, write_bytes):
    source = None
    previous_stat = os.fstat(previous.fileno()) if previous is not None else None
    with ExitStack() as files:
        while True:
            # Sample completion *before* opening/reading so a fast-exiting
            # game's last bytes are forwarded even between polls.
            finishing = done.is_set()
            if source is None:
                candidate = _open_log(path)
                if candidate is not None:
                    if previous_stat and os.path.samestat(previous_stat, os.fstat(candidate.fileno())):
                        candidate.close()
                    else:
                        source = files.enter_context(candidate)
                        if previous is not None:
                            previous.close()
            if source is not None:
                remaining = os.fstat(source.fileno()).st_size - source.tell()
                while remaining > 0:
                    chunk = source.read(min(remaining, 65536))
                    if not chunk:
                        break
                    write_bytes(chunk)
                    remaining -= len(chunk)
            if finishing:
                return
            done.wait(0.1)


@contextmanager
def game_diagnostics(backend, env, cwd, write_bytes, *, log_path=None):
    """Configure a launch, yielding its stderr consumer and following Proton."""
    raw = _proton_log_path(backend, env, cwd)
    if raw is not None:
        check_log_path(backend, env, cwd, log_path)
        done = Event()
        with ExitStack() as session:
            # Pin the previous inode so it cannot be reused after Proton
            # unlinks its old log, and follow only this launch's new file.
            previous = _open_log(raw)
            if previous is not None:
                session.enter_context(previous)
            pool = session.enter_context(ThreadPoolExecutor(max_workers=1))
            follower = pool.submit(_follow, raw, previous, done, write_bytes)
            try:
                yield write_bytes
            finally:
                done.set()
            follower.result()
        return
    if backend in ("wine", "proton"):
        channels = _WINE_CHANNELS + (",trace+console" if backend == "proton" else "")
        env.setdefault("DXVK_LOG_LEVEL", "warn")
        env.setdefault("VKD3D_DEBUG", "warn")
        if "WINEDEBUG" not in env:
            env["WINEDEBUG"] = channels
            messages = _WineMessages(write_bytes)
            try:
                yield messages
            finally:
                messages.finish()
            return
    yield write_bytes
