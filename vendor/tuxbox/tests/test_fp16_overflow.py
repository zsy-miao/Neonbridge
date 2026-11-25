"""Check the final register values of synthetic V_CVT_F16_F32 programs."""
import os
from pathlib import Path
import shutil
import subprocess
from tempfile import TemporaryDirectory
import unittest

from tools.check_packed_integer_cpu import parse_stores
from tools.reference.scarlett_worker_fixture import container


class HalfOverflowTests(unittest.TestCase):
    def test_finite_overflow_and_infinity(self):
        compiler = os.environ.get("TUXBOX_XSHADER") or shutil.which("xshader")
        dxc = os.environ.get("DXC") or shutil.which("dxc")
        if not (compiler and dxc):
            self.skipTest("set TUXBOX_XSHADER and DXC to compile the synthetic shader")

        cases = [
            (0, 0x47800000, 0x7c00),  # 65536
            (1, 0x47800000, 0x7bff),
            (1, 0xc7800000, 0xfbff),
            (1, 0x7f800000, 0x7c00),  # infinity
            (1, 0xff800000, 0xfc00),
            (1, 0x477ff000, 0x7bff),  # exact nearest-even overflow threshold
            (0, 0x477ff000, 0x7c00),
        ]
        code, stores, expected = [], [], []
        for index, (flag, source, half) in enumerate(cases):
            dest = (index + 4) << 17
            tag = (0xab00 + index) << 16
            # Set MODE.FP16_OVFL, seed the destination and convert a literal.
            code.extend([0xba8005c1, flag, 0x7e0002ff | dest, tag,
                         0x7e0014ff | dest, source])
            stores.append(f"observed[{index}] = v{index + 4};")
            expected.append(tag | half)

        with TemporaryDirectory() as name:
            directory = Path(name)
            shader, hlsl, listing = (directory / f for f in ("input.dxbc", "shader.hlsl", "shader.ll"))
            shader.write_bytes(container(code + [0xbf810000]))
            subprocess.run([compiler, shader, "--hlsl-out", hlsl], check=True, timeout=30)
            body = hlsl.read_text().rstrip().removesuffix("}")
            hlsl.write_text("RWStructuredBuffer<uint> observed : register(u0);\n"
                            + body + "\n" + "\n".join(stores) + "\n}\n")
            subprocess.run([dxc, "-E", "main", "-T", "cs_6_0", "-Fc", listing,
                            "-Fo", directory / "shader.cso", hlsl], check=True, timeout=30)
            self.assertEqual(parse_stores(listing.read_text(), len(cases)), expected)
