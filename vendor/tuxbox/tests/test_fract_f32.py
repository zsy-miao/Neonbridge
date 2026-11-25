import os
from pathlib import Path
import re
import shutil
import subprocess
from tempfile import TemporaryDirectory
import unittest


class FractF32Tests(unittest.TestCase):
    def test_tiny_inputs_and_guest_denormal_modes(self):
        dxc = os.environ.get("DXC") or shutil.which("dxc")
        if not dxc:
            self.skipTest("DXC is required for the HLSL numerical regression")
        cases = [
            # Input bits, MODE register, expected result bits.
            (0xb3000000, 0, 0x3f7fffff), (0xb2ffffff, 0, 0x3f7fffff),
            (0xb4000000, 0, 0x3f7ffffe), (0x33000000, 0, 0x33000000),
            (0xbfa00000, 0, 0x3f400000), (0x80000000, 0, 0),
            (0x80000001, 0, 0), (0x80000001, 0x10, 0x3f7fffff),
            (0x80000001, 0x20, 0), (0x80000001, 0x30, 0x3f7fffff),
            (1, 0x10, 0), (1, 0x20, 0), (1, 0x30, 1),
        ]
        helpers = Path(__file__).resolve().parents[1] / "tools/shader-compiler/src"
        with TemporaryDirectory() as directory:
            shader = Path(directory) / "test.hlsl"
            code = ['#include "fract_f32.hlsl"', 'uint main() : SV_Target { uint result = 0u;']
            for i, (bits, mode, expected) in enumerate(cases):
                code.append(f"result |= uint(asuint(tb_fract_f32(asfloat({bits:#x}u), {mode}u))"
                            f" == {expected:#x}u) << {i};")
            shader.write_text("\n".join([*code, "return result; }"]))
            ir = shader.with_suffix(".ll")
            process = subprocess.run([dxc, "-T", "ps_6_0", "-E", "main", "-I", helpers,
                                      "-Fo", shader.with_suffix(".cso"), "-Fc", ir, shader],
                                     capture_output=True, text=True, timeout=30)
            self.assertEqual(process.returncode, 0, process.stderr)
            output = re.search(r"call void @dx\.op\.storeOutput\.i32\([^\n]+i32 (\d+)\)", ir.read_text())
            self.assertIsNotNone(output, "expected one constant pixel output")
            mask = int(output.group(1))
            self.assertEqual([case for i, case in enumerate(cases) if not mask & (1 << i)], [])
