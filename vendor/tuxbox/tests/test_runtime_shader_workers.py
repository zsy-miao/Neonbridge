"""Live worker scheduling controls, with no GPU or game required."""
from collections import Counter
import hashlib
import os
from pathlib import Path
import sys
from tempfile import TemporaryDirectory
from threading import Event, Lock, Thread
from time import monotonic, sleep
import unittest
from unittest.mock import Mock, patch

from project_tuxbox.runtime_shaders import (
    ShaderRefused, _serve_requests, ordered_requests, runtime_jobs,
)


@unittest.skipUnless(sys.platform == "linux", "Proton runtime shader worker")
class RuntimeShaderWorkers(unittest.TestCase):
    def setUp(self):
        temp = TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.session = self.root / "run-test"
        self.requests = self.session / "requests"
        self.requests.mkdir(parents=True)
        self.cache = self.root / "cache"
        self.cache.mkdir()
        self.archive = self.root / "archive"
        self.archive.mkdir()
        self.stop = Event()
        self.release = Event()
        self.errors = []
        self.logs = []

    def request(self, name, suffix=".dxbc"):
        key = hashlib.sha256(name.encode()).hexdigest()
        if suffix == ".stencil":
            key += "_rt0"
        elif suffix == ".link":
            key = "_".join([key] * 3)
        path = self.requests / (key + suffix)
        if suffix == ".link":
            path.mkdir()
        else:
            path.write_text(name)
        return path

    def start(self, compile_one, jobs=2, archive=None):
        def worker():
            try:
                _serve_requests(self.session, self.stop, self.logs.append,
                                self.root / "xshader", lambda: sleep(0.005), archive, jobs)
            except Exception as exc:
                self.errors.append(exc)

        for name, result in (
            ("snapshot_compiler_cache", (("xshader", "dxc", "dxv"), self.cache)),
            ("compile_request", None),
        ):
            patcher = patch("project_tuxbox.runtime_shaders." + name, return_value=result)
            mock = patcher.start()
            self.addCleanup(patcher.stop)
            if name == "compile_request":
                mock.side_effect = compile_one
        thread = Thread(target=worker)
        thread.start()

        def cleanup():
            self.stop.set()
            self.release.set()
            thread.join(5)
            self.assertFalse(thread.is_alive())
            self.assertEqual(self.errors, [])
        self.addCleanup(cleanup)
        return thread

    def until(self, predicate):
        deadline = monotonic() + 5
        while monotonic() < deadline:
            if predicate():
                return
            sleep(0.005)
        self.fail("worker did not publish expected result")

    def test_parallel_fast_job_not_blocked_by_slow_job_and_no_duplicate(self):
        slow = self.request("slow")
        fast = self.request("fast", ".stencil")
        later = self.request("later", ".link")
        for n, path in enumerate((slow, fast, later)):
            os.utime(path, ns=(n + 1, n + 1))
        started = Event()
        counts = Counter()
        lock = Lock()

        def compile_one(source, key, tools, cache, session, remember=None):
            with lock:
                counts[key] += 1
            if source == slow:
                started.set()
                if not self.release.wait(5):
                    raise TimeoutError("test gate")
            (session / (key + ".cso")).write_bytes(b"result")
            return True

        self.start(compile_one)
        self.until(started.is_set)
        self.until(lambda: all((self.session / (p.stem + ".cso")).exists() for p in (fast, later)))
        self.assertFalse((self.session / (slow.stem + ".cso")).exists())
        self.assertEqual(counts[slow.stem], 1)
        self.release.set()
        self.until(lambda: not any(self.requests.iterdir()))
        self.assertEqual(counts, Counter({p.stem: 1 for p in (slow, fast, later)}))

    def test_shutdown_drains_active_jobs_without_starting_new_jobs(self):
        paths = [self.request(str(i)) for i in range(5)]
        started = []
        lock = Lock()

        def compile_one(source, key, tools, cache, session, remember=None):
            with lock:
                started.append(source)
            if not self.release.wait(5):
                raise TimeoutError("test gate")
            return False

        thread = self.start(compile_one, jobs=2)
        self.until(lambda: len(started) == 2)
        self.stop.set()
        self.release.set()
        thread.join(5)
        self.assertFalse(thread.is_alive())
        self.assertEqual(len(started), 2)
        self.assertEqual(set(self.requests.iterdir()), set(paths) - set(started))

    def test_failures_release_waiters_without_losing_other_requests(self):
        good, refused, transient, cached = [self.request(name) for name in ("good", "refused", "error", "cached")]
        invalid = self.requests / "bad.dxbc"
        invalid.touch()
        archived = []

        def compile_one(source, key, tools, cache, session, remember=None):
            # The callback may run on several threads at once.
            remember(source, (source,))
            if source == refused:
                raise ShaderRefused("unhandled", cached=False)
            if source == transient:
                raise OSError("retryable")
            if source == cached:
                raise ShaderRefused("cached", cached=True)
            (session / (key + ".cso")).write_bytes(b"result")
            return True

        def archive_request(archive, source, inputs):
            archived.append(source.name)
            assert archive == self.archive

        with patch("project_tuxbox.runtime_shaders.archive_request", side_effect=archive_request):
            thread = self.start(compile_one, jobs=3, archive=self.archive)
            self.until(lambda: all((self.session / (p.stem + ".failed")).exists() for p in (refused, transient, cached)))
            self.until(lambda: not any(self.requests.iterdir()))
            self.stop.set()
            thread.join(5)
        self.assertEqual(set(archived), {p.name for p in (good, refused, transient, cached)})
        self.assertTrue((self.session / (good.stem + ".cso")).exists())
        self.assertFalse((self.session / (good.stem + ".failed")).exists())
        self.assertTrue(any("1 new unsupported translations, 1 cached refusals" in line for line in self.logs))

    def test_failure_is_visible_before_request_is_removed(self):
        source = self.request("failed-and-retried")
        failed = self.session / (source.stem + ".failed")
        seen = []

        def compile_one(source, key, tools, cache, session):
            raise OSError("retryable")

        def remove(path):
            if path == source:
                # A guest can immediately retry once the old source is absent.
                # Its previous failure must already have been published.
                seen.append(failed.exists())
                path.unlink()
                self.stop.set()

        with patch("project_tuxbox.runtime_shaders.remove_request", side_effect=remove):
            thread = self.start(compile_one)
            thread.join(5)
            self.assertFalse(thread.is_alive())
        self.assertEqual(seen, [True])

    def test_job_count_and_fifo_across_request_types(self):
        report = Mock()
        with patch("os.sched_getaffinity", return_value={0, 1, 2}):
            self.assertEqual(runtime_jobs({}, report), 3)
            self.assertEqual(runtime_jobs({"TUXBOX_RUNTIME_SHADER_JOBS": "1"}, report), 1)
            self.assertEqual(runtime_jobs({"TUXBOX_RUNTIME_SHADER_JOBS": "8"}, report), 3)
            self.assertEqual(runtime_jobs({"TUXBOX_RUNTIME_SHADER_JOBS": "0"}, report), 3)
            report.assert_called_once()
        paths = [self.request("old", ".link"), self.request("middle", ".stencil"), self.request("new")]
        for n, path in enumerate(paths):
            os.utime(path, ns=(n + 1, n + 1))
        self.assertEqual(ordered_requests(self.requests), paths)


if __name__ == "__main__":
    unittest.main()
