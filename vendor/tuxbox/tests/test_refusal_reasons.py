import hashlib
import os
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

from project_tuxbox.runtime_shaders import REASON_BYTES, ShaderRefused, compile_request
from project_tuxbox.scarlett_report import refusal_bucket
from project_tuxbox.shader_prep import ToolFailure

REASON = "wave reduction at 0x35c may execute in divergent control flow"


class CachedRefusalReasons(unittest.TestCase):
    def setUp(self):
        temporary = TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        self.cache = root / "cache"
        self.cache.mkdir()
        self.session = root / "session"
        self.session.mkdir()
        self.source = root / "request.dxbc"
        self.source.write_bytes(b"Xbox compute shader")
        self.key = hashlib.sha256(self.source.read_bytes()).hexdigest()
        self.reason = self.cache / (self.key + ".reason")
        self.stderr = f"note: translating\n{REASON}\n"

    def refuse(self):
        def run(args, *, timeout):
            self.assertEqual(args[0], "xshader")
            raise ToolFailure("xshader", 65, self.stderr)

        with patch("project_tuxbox.runtime_shaders.run_tool", side_effect=run) as tool:
            with self.assertRaises(ShaderRefused) as raised:
                compile_request(self.source, self.key, ("xshader", "dxc", "dxv"), self.cache, self.session)
        return raised.exception, tool.call_count

    def test_warm_refusal_keeps_the_emitter_reason(self):
        fresh, calls = self.refuse()
        self.assertEqual((fresh.cached, calls), (False, 1))
        # The provider reads this marker and expects exactly the key.
        self.assertEqual((self.cache / (self.key + ".unsupported")).read_text(), self.key)
        self.assertEqual(self.reason.read_text(), REASON)

        warm, calls = self.refuse()
        self.assertEqual((warm.cached, warm.remembered, calls), (True, True, 0))
        self.assertEqual(str(warm), REASON)
        self.assertEqual(refusal_bucket(warm), refusal_bucket(fresh))
        self.assertEqual(refusal_bucket(warm), "wave_divergent")

    def test_marker_without_reason_stays_unknown(self):
        self.refuse()
        self.reason.unlink()
        warm, calls = self.refuse()
        self.assertEqual((warm.cached, warm.remembered, calls), (True, False, 0))
        self.assertEqual(refusal_bucket(warm), "cached_reason_unavailable")

    def test_unusable_reason_files_are_ignored(self):
        self.refuse()
        self.reason.write_bytes(b"x" * (REASON_BYTES + 1))
        self.assertEqual(refusal_bucket(self.refuse()[0]), "cached_reason_unavailable")
        self.reason.unlink()
        elsewhere = self.cache / "elsewhere"
        elsewhere.write_text(REASON)
        os.symlink(elsewhere, self.reason)
        self.assertEqual(refusal_bucket(self.refuse()[0]), "cached_reason_unavailable")

    def test_long_reason_is_bounded(self):
        self.stderr = "s_load " + "x" * (2 * REASON_BYTES)
        self.refuse()
        self.assertEqual(len(self.reason.read_bytes()), REASON_BYTES)
        warm, _ = self.refuse()
        self.assertTrue(warm.remembered)


if __name__ == "__main__":
    unittest.main()
