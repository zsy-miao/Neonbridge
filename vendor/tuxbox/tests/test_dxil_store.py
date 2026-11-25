import hashlib
from pathlib import Path
import struct
import subprocess
import sys
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

from project_tuxbox.dxil_store import DxilStore, write_identity
from project_tuxbox.runtime_shaders import compile_request


def container(tag):
    bitcode = b"BC\xc0\xde" + tag.ljust(4, b"\0")
    # Shader kind 0 (pixel), model 6.0, matching the tests' pixel profiles.
    program = struct.pack("<II4sIII", 0x60, len(bitcode) // 4 + 6, b"DXIL", 0x100, 16, len(bitcode)) + bitcode
    chunk = struct.pack("<4sI", b"DXIL", len(program)) + program
    return b"DXBC" + bytes(16) + struct.pack("<IIII", 1, 36 + len(chunk), 1, 36) + chunk


@unittest.skipUnless(sys.platform == "linux", "Uses Proton request file verification")
class DxilReuse(unittest.TestCase):
    def setUp(self):
        tmp = TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        self.tools = tuple(self.root / name for name in ("xshader", "dxc", "dxv"))
        for path in self.tools:
            path.write_text(path.name)
        self.hlsl = "void main() {}"
        self.profile = "ps_6_0"
        self.timeouts = 0
        self.calls = []

    def cache(self, translator, toolchain=b"dxc 1"):
        cache = self.root / "runtime" / translator
        cache.mkdir(parents=True)
        if toolchain:
            write_identity(cache, toolchain, *self.tools[1:])
        return cache

    def run_tool(self, args, *, timeout):
        if args[0].name == "xshader":
            args[args.index("--hlsl-out") + 1].write_text(self.hlsl)
            args[args.index("--profile-out") + 1].write_text(self.profile)
        elif args[0].name == "dxc":
            self.calls.append("-O0" in args)
            if self.timeouts:
                self.timeouts -= 1
                raise subprocess.TimeoutExpired(args, timeout)
            args[args.index("-Fo") + 1].write_bytes(container(args[-1].read_bytes()[:4]))

    def compile(self, cache, data=b"shader"):
        key = hashlib.sha256(data).hexdigest()
        source = self.root / (key + ".dxbc")
        source.write_bytes(data)
        with patch("project_tuxbox.runtime_shaders.run_tool", side_effect=self.run_tool):
            self.assertTrue(compile_request(source, key, self.tools, cache, None))
        return (cache / (key + ".cso")).read_bytes()

    def test_new_translator_with_same_hlsl_skips_dxc(self):
        first = self.compile(self.cache("a"))
        second = self.compile(self.cache("b"))
        self.assertEqual(first, second)
        self.assertEqual(len(self.calls), 1)
        self.assertEqual((self.root / "runtime/b" / (hashlib.sha256(b"shader").hexdigest() + ".sha256")).read_text(),
                         hashlib.sha256(second).hexdigest())

    def test_changed_hlsl_profile_or_toolchain_recompiles(self):
        self.compile(self.cache("a"))
        self.hlsl = "void main() { }"
        self.compile(self.cache("b"))
        self.profile = "ps_6_6"
        self.compile(self.cache("c"))
        self.compile(self.cache("d", b"dxc 2"))
        self.assertEqual(len(self.calls), 4)

    def test_cache_without_an_identity_never_reuses(self):
        self.compile(self.cache("a", None))
        self.compile(self.cache("b", None))
        self.assertEqual(len(self.calls), 2)
        self.assertFalse((self.root / "dxil").exists())

    def test_damaged_entry_is_recompiled(self):
        self.compile(self.cache("a"))
        stored, = (self.root / "dxil").glob("*/*.cso")
        stored.write_bytes(container(b"bad!"))
        self.compile(self.cache("b"))
        self.assertEqual(len(self.calls), 2)
        self.compile(self.cache("c"))
        self.assertEqual(len(self.calls), 2, "the recompile replaced the damaged entry")

    def test_timed_out_request_goes_straight_to_o0_afterwards(self):
        self.timeouts = 1
        self.compile(self.cache("a"))
        self.assertEqual(self.calls, [False, True])
        self.hlsl = "void main() { }"
        self.compile(self.cache("b"))
        self.assertEqual(self.calls, [False, True, True])
        self.compile(self.cache("c"), b"another")
        self.assertEqual(len(self.calls), 3, "same HLSL is reused across requests")

    def test_identity_follows_the_toolchain_files(self):
        cache = self.cache("a")
        before = DxilStore.for_cache(cache).root
        write_identity(cache, b"dxc 1", *self.tools[1:])
        self.assertEqual(DxilStore.for_cache(cache).root, before)
        self.tools[2].write_text("another validator")
        write_identity(cache, b"dxc 1", *self.tools[1:])
        self.assertNotEqual(DxilStore.for_cache(cache).root, before)


if __name__ == "__main__":
    unittest.main()
