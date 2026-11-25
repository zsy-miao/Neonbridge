from pathlib import Path
import sys
from tempfile import TemporaryDirectory
import unittest

from project_tuxbox.persistent_dxc import PersistentDxc


@unittest.skipUnless(sys.platform == "linux", "The DXC helper uses Linux pipes")
class PersistentDxcTests(unittest.TestCase):
    def setUp(self):
        temporary = TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.dxc = self.root / "bin/dxc"
        self.dxc.parent.mkdir()
        self.dxc.touch()
        library = self.root / "lib/libdxcompiler.so"
        library.parent.mkdir()
        library.touch()
        self.source = self.root / "source.hlsl"
        self.source.write_bytes(b"synthetic HLSL")
        self.output = self.root / "out.cso"
        self.report = []

    def helper(self, body):
        executable = self.root / "helper"
        executable.write_text(f"#!{sys.executable}\n" + body)
        executable.chmod(0o700)
        return executable

    def test_replaces_crashed_helper_and_retries_before_reusing_it(self):
        helper = self.helper(
            """import os, sys
from pathlib import Path
for line in sys.stdin:
    source = Path(line.strip())
    output = Path(sys.stdin.readline().strip())
    marker = source.with_suffix(".crashed")
    if not marker.exists():
        marker.touch()
        output.write_bytes(b"partial DXIL")
        os._exit(3)
    assert not output.exists()
    output.write_bytes(f"{os.getpid()}:".encode() + source.read_bytes())
    print("ok", flush=True)
"""
        )
        with PersistentDxc(helper, 1, self.report.append) as pool:
            self.assertTrue(pool.compile(self.dxc, self.source, self.output))
            first = self.output.read_bytes()
            pid = int(first.split(b":")[0])
            self.assertTrue(first.endswith(self.source.read_bytes()))
            self.output.unlink()
            self.assertTrue(pool.compile(self.dxc, self.source, self.output))
            self.assertEqual(self.output.read_bytes(), first)
        self.assertFalse(Path(f"/proc/{pid}").exists())
        self.assertEqual(self.report, [])

    def test_repeated_crash_leaves_no_partial_dxil_for_cli(self):
        helper = self.helper(
            """import os, sys
from pathlib import Path
source = Path(sys.stdin.readline().strip())
output = Path(sys.stdin.readline().strip())
assert not output.exists()
with source.with_suffix(".attempts").open("ab") as log:
    log.write(b"x")
output.write_bytes(b"partial DXIL")
os._exit(3)
"""
        )
        with PersistentDxc(helper, 1, self.report.append) as pool:
            self.assertFalse(pool.compile(self.dxc, self.source, self.output))
        self.assertFalse(self.output.exists())
        self.assertEqual(self.source.with_suffix(".attempts").read_bytes(), b"xx")
        self.assertEqual(len(self.report), 1)
