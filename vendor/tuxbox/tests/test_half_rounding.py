import os
from pathlib import Path
import re
import shutil
import subprocess
from tempfile import TemporaryDirectory
import unittest

from tools.reference.scarlett_worker_fixture import container


class HalfRoundingTests(unittest.TestCase):
    def check_inline_results(self, cases):
        self.check_register_results((words, 0xa5a50000 | low) for words, low in cases)

    def check_register_results(self, cases):
        xshader = os.environ.get("TUXBOX_XSHADER") or shutil.which("xshader")
        dxc = os.environ.get("DXC") or shutil.which("dxc")
        if not (xshader and dxc):
            self.skipTest("set TUXBOX_XSHADER and DXC for native half-rounding tests")

        with TemporaryDirectory() as directory:
            trial = Path(directory)
            native, hlsl, ir = (trial / name for name in ("shader.dxbc", "shader.hlsl", "shader.ll"))
            for instructions, expected in cases:
                words = [instructions] if isinstance(instructions, int) else instructions
                with self.subTest(words=[hex(word) for word in words]):
                    code = [0x7e0802ff, 0xa5a5abcd, *words, 0xbf810000]
                    native.write_bytes(container(code))
                    subprocess.run([xshader, native, "--hlsl-out", hlsl], check=True,
                                   capture_output=True, timeout=30)
                    # Expose only the final register; DXC folds the translated instructions.
                    body = hlsl.read_text().rstrip().removesuffix("}")
                    hlsl.write_text("RWByteAddressBuffer observed : register(u0);\n" +
                                    body + "\nobserved.Store(gtid.x * 4u, v4);\n}\n")
                    result = subprocess.run([dxc, "-T", "cs_6_0", "-E", "main", "-Fc", ir,
                                             "-Fo", trial / "shader.cso", hlsl],
                                            text=True, capture_output=True, timeout=30)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    stores = re.findall(r"call void @dx\.op\.bufferStore\.i32\([^\n]+?, i32 undef, i32 (-?\d+),", ir.read_text())
                    self.assertEqual([int(v) & 0xffffffff for v in stores], [expected])

    def test_inline_fraction_keeps_other_half(self):
        # gfx1030 v_fract_f16 v4 with inline half constants -0.5, +0.5, -1, +1.
        cases = [(0x7e08bef1, 0x3800), (0x7e08bef0, 0x3800),
                 (0x7e08bef3, 0), (0x7e08bef2, 0)]
        # With round-down active, FRACT must use the already-rounded FP16
        # inline ROM entry for 1/(2*pi).
        cases.append(([0xbfa40008, 0x7e08bef8], 0x3118))
        self.check_inline_results(cases)

    def test_round_to_integer_keeps_other_half_and_signed_zero(self):
        # gfx1030 VOP1 with v4 as destination. Include both ties to even at zero.
        self.check_inline_results([
            (0x7e08b6f1, 0xbc00),  # floor(-0.5) = -1
            (0x7e08b8f0, 0x3c00),  # ceil(+0.5) = 1
            (0x7e08baf1, 0x8000),  # trunc(-0.5) = -0
            (0x7e08bcf1, 0x8000),  # roundEven(-0.5) = -0
            (0x7e08bcf0, 0x0000),  # roundEven(+0.5) = +0
        ])

    def test_half_rounding_retains_nan_payload_and_obeys_mode(self):
        ieee = [0xba800241, 1]   # set MODE.IEEE
        dx10 = [0xba800201, 1]   # set MODE.DX10_CLAMP
        cases = []
        for opcode in (0xb6, 0xb8, 0xba, 0xbc, 0xbe):
            literal = [0x7e0800ff | (opcode << 8), 0xfc35]
            cases.extend([(literal, 0xfc35), (ieee + literal, 0xfe35)])
        # v_floor_f16_e64 v4, literal clamp; v_fract_f16 v4, -infinity.
        clamp = [0xd5db8004, 0xff, 0xfe35]
        # A register source with -abs changes sign before quieting. mul:2
        # is enabled with MODE=0 and still preserves the NaN payload.
        negabs_mul = [0x7e0202ff, 0x7c35, 0xd5dc0104, 0x28000101]
        cases.extend([(clamp, 0xfe35), (dx10 + clamp, 0),
                      ([0xbfa50000] + negabs_mul, 0xfc35),
                      (ieee + negabs_mul, 0xfe35),
                      ([0x7e08beff, 0xfc00], 0xfe00)])
        self.check_inline_results(cases)

    def test_half_output_multiplier_obeys_mode(self):
        # v_fract_f16 v4, 0.5 mul:2 and v_floor_f16 v4, 1.0 mul:2.
        # MODE initially preserves FP16 output denormals. Setting IEEE also
        # disables scaling, even after s_denorm_mode has disabled denormals.
        fraction = [0xd5df0004, 0x080000f0]
        floor = [0xd5db0004, 0x080000f2]
        ieee = [0xbfa50000, 0xba800241, 1]
        negative_zero = [0xd5de0004, 0x080000f1]  # roundEven(-0.5) mul:2
        self.check_inline_results([
            ([0xbfa50000] + negative_zero, 0x0000),
            (ieee + negative_zero, 0x8000),
            (fraction, 0x3800), (floor, 0x3c00),
            (ieee + fraction, 0x3800), (ieee + floor, 0x3c00),
            # s_mov_b32 s8,1; s_setreg_b32 hwreg(MODE,9,1),s8
            ([0xbfa50000, 0xbe880381, 0xb9880241] + floor, 0x3c00),
            # A full MODE write sets IEEE and clears output denormal enable.
            ([0xba80f801, 0x200] + floor, 0x3c00),
        ])

    def test_integer_to_half_obeys_signedness_rounding_and_inline_bits(self):
        self.check_inline_results([
            (0x7e08a2c1, 0xbc00),                  # signed -1
            (0x7e08a0c1, 0x7c00),                  # unsigned 65535, nearest-even
            ([0xbfa4000c, 0x7e08a0c1], 0x7bff),    # same integer, toward zero
            ([0xd5d00004, 0xf0], 0x7300),          # inline 0.5 supplies bits 0x3800
        ])

    def test_half_to_integer_preserves_high_half(self):
        self.check_inline_results([
            (0x7e08a6f5, 0xfffe),                # signed -2
            (0x7e08a4f5, 0),                     # unsigned -2 saturates to zero
            ([0x7e08a6ff, 0x7e11], 0),           # positive NaN
            ([0x7e08a4ff, 0xfe11], 0),           # negative NaN
        ])

    def test_half_to_integer_sdwa_selects_source_and_destination(self):
        # v1 contains a NaN in WORD_1 and +2 in WORD_0. The destination
        # initially contains 0xa5a5abcd, so a misplaced write is observable.
        seed = [0x7e0202ff, 0x7e114000]
        self.check_register_results([
            (seed + [0x7e08a6f9, 0x51501], 0x0000abcd),  # high word, preserve low
            (seed + [0x7e08a4f9, 0x50401], 0x00000000),  # low word, pad high
        ])

    def test_normalized_half_to_integer_rounds_once(self):
        # 0x3801 is the next binary16 above 0.5. A binary32 product can
        # round it onto a tie before the final integer rounding.
        self.check_inline_results([
            ([0x7e08c6ff, 0x3801], 0x400f),
            ([0x7e08c8ff, 0x3801], 0x801f),
            (0x7e08c6f0, 0x4000),                 # exact tie at +0.5
            (0x7e08c6f3, 0x8001),                 # SNORM(-1) = -32767
            ([0x7e08c6ff, 0xfe11], 0),            # negative NaN
            ([0x7e08c8ff, 0x03ff], 4),            # largest half subnormal
            ([0xbfa50000, 0x7e08c8ff, 0x03ff], 0),  # flush input subnormals
        ])

    def test_sdwa_dword_zero_extends_the_result(self):
        # v1.h is NaN. Converting it writes a full zero word even though
        # the instruction encodes DST_UNUSED=PRESERVE.
        seed = [0x7e0202ff, 0x7e114000]
        self.check_register_results([
            (seed + [0x7e08c6f9, 0x51601], 0),   # normalized signed
            (seed + [0x7e08a4f9, 0x51601], 0),   # ordinary unsigned
        ])
