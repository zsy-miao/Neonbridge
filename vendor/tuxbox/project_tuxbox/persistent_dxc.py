"""Session-scoped pool for the experimental Linux DXC helper."""

import os
from pathlib import Path
from queue import Queue
import select
import subprocess
from time import monotonic


class CompilerExited(ConnectionError):
    pass


class Compiler:
    def __init__(self, helper, library):
        env = dict(os.environ, LD_LIBRARY_PATH=str(library))
        self.process = subprocess.Popen(
            [helper], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL, bufsize=0, env=env,
        )

    def compile(self, hlsl, output, deadline):
        request = b"\n".join(os.fsencode(path) for path in (hlsl, output)) + b"\n"
        self.process.stdin.write(request)
        response = b""
        while len(response) < 3:
            if not select.select([self.process.stdout], [], [], max(0, deadline - monotonic()))[0]:
                raise TimeoutError("persistent DXC timed out")
            data = os.read(self.process.stdout.fileno(), 3 - len(response))
            if not data:
                raise CompilerExited("persistent DXC exited")
            response += data
        if response != b"ok\n":
            raise ValueError("invalid persistent DXC response")

    def close(self, *, kill=False):
        if kill:
            self.process.kill()
        self.process.stdin.close()
        try:
            self.process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()
        finally:
            self.process.stdout.close()


class PersistentDxc:
    def __init__(self, helper, jobs, report):
        self.helper = helper
        self.report = report
        self.slots = Queue(jobs)
        for _ in range(jobs):
            self.slots.put(None)

    def compile(self, dxc, hlsl, output, *, timeout=15):
        # The helper uses newline-delimited ASCII paths and the same Linux
        # library as the Microsoft DXC installation selected for this session.
        dxc = Path(dxc).resolve()
        library = dxc.parent.parent / "lib"
        if not (library / "libdxcompiler.so").is_file() or any(
            not str(path).isascii() or "\n" in str(path) for path in (hlsl, output)
        ):
            return False
        deadline = monotonic() + timeout
        compiler = self.slots.get()
        try:
            for attempt in range(2):
                try:
                    if compiler is None:
                        compiler = Compiler(self.helper, library)
                    compiler.compile(hlsl, output, deadline)
                    return True
                except (OSError, ValueError) as error:
                    if compiler is not None:
                        compiler.close(kill=True)
                        compiler = None
                    output.unlink(missing_ok=True)
                    if attempt or not isinstance(error, (CompilerExited, BrokenPipeError)) or monotonic() >= deadline:
                        self.report(f"Persistent DXC unavailable, using the CLI: {error}")
                        return False
        finally:
            self.slots.put(compiler)

    def __enter__(self):
        return self

    def __exit__(self, *_):
        for _ in range(self.slots.maxsize):
            compiler = self.slots.get_nowait()
            if compiler is not None:
                compiler.close()
