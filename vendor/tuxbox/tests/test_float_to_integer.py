import os
from pathlib import Path
import re
import shutil
import subprocess
from tempfile import TemporaryDirectory
import unittest

from tools.reference.scarlett_worker_fixture import uav_shader as native_shader


class FloatToIntegerTests(unittest.TestCase):
    def test_rounding_at_half_and_integral_boundaries(self):
        self.check_conversion(0x7e0818ff, [
            (0x3effffff, 0), (0x3f000000, 1), (0xbf000000, 0),
            (0xbf000001, -1), (0xbfc00000, -1), (0x40200000, 3),
            (0x4b000001, 8388609), (0xcb000001, -8388609),
        ])

    def test_saturating_signed_and_unsigned_conversion(self):
        # gfx1030 V_CVT_I32_F32 and V_CVT_U32_F32. The expected bits include
        # the largest representable f32 below each positive integer boundary.
        cases = [
            (0x4effffff, 0x7fffff80, 0x7fffff80),
            (0x4f000000, 0x7fffffff, 0x80000000),
            (0x4f7fffff, 0x7fffffff, 0xffffff00),
            (0x4f800000, 0x7fffffff, 0xffffffff),
            (0xcf000001, 0x80000000, 0),
            (0x7f800000, 0x7fffffff, 0xffffffff),
            (0xff800000, 0x80000000, 0),
            (0x7fc00001, 0, 0), (0xff800001, 0, 0),
            (0xbfffffff, -1, 0), (0x3fffffff, 1, 1),
        ]
        self.check_conversion(0x7e0810ff, [(bits, s) for bits, s, _ in cases])
        self.check_conversion(0x7e080eff, [(bits, u) for bits, _, u in cases])

    def check_conversion(self, instruction, cases):
        self.check_programs(([instruction, bits], expected) for bits, expected in cases)

    def test_pack_byte_rounds_and_preserves_the_other_three_bytes(self):
        # Set the FP32 round and denormal modes, then insert v1 into byte 2 of v4.
        cases = [
            # Input bits, round mode, denormal mode, expected byte.
            (0x3f000000, 0, 0, 0), (0x3f000000, 1, 0, 1),  # 0.5
            (0x3fc00000, 2, 0, 1), (0x3fc00000, 3, 0, 1),  # 1.5
            (1, 1, 0, 0), (1, 1, 1, 1), (1, 1, 2, 0), (1, 1, 3, 1),
            (0x7f800000, 1, 0, 255), (0x7fc00001, 1, 0, 0),
        ]
        programs = []
        for bits, rounding, denorm, byte in cases:
            code = [0xbfa40000 | rounding, 0xbfa50000 | denorm,
                    0x7e0202ff, bits, 0x7e0802ff, 0x12345678, 0xd55e0004, 0x04110501]
            programs.append((code, 0x12005678 | (byte << 16)))
        self.check_programs(programs)

    def test_integer_to_float_uses_single_precision_rounding(self):
        # v_cvt_f32_{i32,u32} v4, v1. The four results are nearest-even,
        # toward +infinity, toward -infinity, and toward zero.
        signed, unsigned = 0x7e080b01, 0x7e080d01
        cases = [
            (unsigned, 0x01000001, [0x4b800000, 0x4b800001, 0x4b800000, 0x4b800000]),
            (unsigned, 0x01000003, [0x4b800002, 0x4b800002, 0x4b800001, 0x4b800001]),
            (unsigned, 0xffffffff, [0x4f800000, 0x4f800000, 0x4f7fffff, 0x4f7fffff]),
            (signed, 0xfeffffff, [0xcb800000, 0xcb800000, 0xcb800001, 0xcb800000]),
            (signed, 0x7fffffff, [0x4f000000, 0x4f000000, 0x4effffff, 0x4effffff]),
            (signed, 0x80000000, [0xcf000000] * 4),
            (signed, 0, [0] * 4),
        ]
        self.check_programs(
            ([0xbfa40000 | mode, 0x7e0202ff, bits, instruction], expected)
            for instruction, bits, results in cases
            for mode, expected in enumerate(results)
        )

    def test_round_mode_changes_between_two_conversions(self):
        # Convert 2**24 + 1 upward into v2, then downward into v4.
        # The two adjacent floats differ in the low bit.
        self.check_programs([([
            0x7e0202ff, 0x01000001, 0xbfa40001, 0x7e040d01,
            0xbfa40002, 0x7e080d01, 0x3a080902,
        ], 1)])

    def test_ldexp_rounding_and_denormal_modes(self):
        # RX 6800 measurements of input/output flushing: github.com/boykopovar/AnyPS5/pull/6008
        # Input bits, signed exponent, denormal mode, rounding, IEEE, output bits.
        cases = [
            (1, 23, 1, 0, 0, 0x00800000), (1, 23, 2, 0, 0, 0),
            (0x3fffffff, -127, 1, 0, 0, 0), (0x3fffffff, -127, 3, 0, 0, 0x00800000),
            (0x3f800000, -150, 3, 0, 0, 0), (0x3f800000, -150, 3, 1, 0, 1),
            (0xbf800000, -150, 3, 2, 0, 0x80000001), (3, -1, 3, 0, 0, 2),
            (0x7f7fffff, 0x7fffffff, 0, 3, 0, 0x7f7fffff),
            (0xff7fffff, 1, 0, 1, 0, 0xff7fffff),
            (0xff7fffff, 1, 0, 2, 0, 0xff800000),
            (0x80800000, -0x80000000, 2, 2, 0, 0x80000001),
            (0x7f812345, 9, 3, 0, 0, 0x7f812345),
            (0x7f812345, 9, 3, 0, 1, 0x7fc12345),
        ]
        self.check_programs(
            ([0xbfa50000 | denorm, 0xbfa40000 | rounding, 0xba800241, ieee,
              0x7e0202ff, bits, 0x7e0602ff, exponent & 0xffffffff,
              0xd7620004, 0x00020701], expected)  # v_ldexp_f32 v4, v1, v3
            for bits, exponent, denorm, rounding, ieee, expected in cases
        )

    def test_ldexp_output_modifiers(self):
        cases = [
            # MODE, source bits, signed exponent, clamp, OMOD, result bits.
            (0x120, 0x00800000, -1, True, 0, 0x00400000),
            (0x120, 0x80800000, -1, True, 0, 0),
            (0x100, 0x7fc12345, 0, True, 0, 0),
            (0x200, 0x7f812345, 0, True, 0, 0x7fc12345),
            (0, 0x3e800000, 1, False, 1, 0x3f800000),
            (0x020, 0x3e800000, 1, False, 1, 0x3f000000),
            (0x200, 0x3e800000, 1, False, 1, 0x3f000000),
        ]
        self.check_programs(
            ([0xba804801, mode, 0x7e0202ff, bits, 0x7e0602ff, exponent & 0xffffffff,
              0xd7620004 | (clamp << 15), 0x00020701 | (omod << 27)], expected)
            for mode, bits, exponent, clamp, omod, expected in cases
        )

    def check_programs(self, cases):
        xshader = os.environ.get("TUXBOX_XSHADER") or shutil.which("xshader")
        dxc = os.environ.get("DXC") or shutil.which("dxc")
        if not (xshader and dxc):
            self.skipTest("set TUXBOX_XSHADER and DXC to run the native rounding check")

        # gfx1030: load the UAV descriptor, convert a literal, store v4 at lane * 4.
        load = [0xf4080402, 0xfa000000, 0xbf8cc07f]
        store = [0x340a0082, 0xe0701000, 0x80040405, 0xbf810000]
        with TemporaryDirectory() as directory:
            trial = Path(directory)
            for program, expected in cases:
                with self.subTest(program=[hex(word) for word in program]):
                    code = load + program + store
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
                    self.assertEqual([int(value) & 0xffffffff for value in values], [expected & 0xffffffff])
