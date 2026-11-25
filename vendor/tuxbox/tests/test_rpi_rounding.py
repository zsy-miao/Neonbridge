import os
from pathlib import Path
import re
import shutil
import subprocess
from tempfile import TemporaryDirectory
import unittest

from tools.reference.scarlett_worker_fixture import uav_shader as native_shader


class RpiRoundingTests(unittest.TestCase):
    def test_native_rounding_at_half_and_integral_boundaries(self):
        xshader = os.environ.get("TUXBOX_XSHADER") or shutil.which("xshader")
        dxc = os.environ.get("DXC") or shutil.which("dxc")
        if not (xshader and dxc):
            self.skipTest("set TUXBOX_XSHADER and DXC to run the native rounding check")

        # gfx1030: load the UAV descriptor, convert a literal, store v4 at lane * 4.
        load = [0xf4080402, 0xfa000000, 0xbf8cc07f]
        store = [0x340a0082, 0xe0701000, 0x80040405, 0xbf810000]
        cases = [(0x3effffff, 0), (0x3f000000, 1), (0xbf000000, 0),
                 (0xbf000001, -1), (0xbfc00000, -1), (0x40200000, 3),
                 (0x4b000001, 8388609), (0xcb000001, -8388609)]
        with TemporaryDirectory() as directory:
            trial = Path(directory)
            for bits, expected in cases:
                with self.subTest(bits=hex(bits)):
                    code = load + [0x7e0818ff, bits] + store
                    (trial / "native.dxbc").write_bytes(native_shader(code))
                    for command in (
                        [xshader, trial / "native.dxbc", "--hlsl-out", trial / "shader.hlsl"],
                        [dxc, "-T", "cs_6_0", "-E", "main", "-Fc", trial / "shader.ll",
                         "-Fo", trial / "shader.cso", trial / "shader.hlsl"],
                    ):
                        result = subprocess.run(command, capture_output=True, text=True, timeout=30)
                        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                    # DXC folds the conversion before the shader writes its only output.
                    ir = (trial / "shader.ll").read_text()
                    values = re.findall(r"call void @dx\.op\.bufferStore\.i32\([^\n]+?, i32 undef, i32 (-?\d+),", ir)
                    self.assertEqual([int(value) for value in values], [expected])
