import unittest

from tools.check_sad_mad64_gpu import expected_outputs


class SadMad64ReferenceTests(unittest.TestCase):
    # Hand-computed values, independent of the model's formulas.
    def test_sums_of_absolute_differences(self):
        out = expected_outputs(0x0100ff10, 0x02ff0020, 7, 0)
        self.assertEqual(out[0], 1 + 255 + 255 + 16 + 7)               # v_sad_u8
        self.assertEqual(out[1], ((1 + 255 + 255 + 16) << 16) + 7)     # v_sad_hi_u8
        self.assertEqual(out[2], (0xff10 - 0x0020) + (0x02ff - 0x0100) + 7)  # v_sad_u16
        self.assertEqual(out[3], 0x02ff0020 - 0x0100ff10 + 7)          # v_sad_u32
        self.assertEqual(expected_outputs(0, 0xffffffff, 0xffffffff, 0)[3], 0xfffffffe)

    def test_64_bit_multiply_add(self):
        # -1 * 2 + 5 = 3 signed; 0xffffffff * 2 + 5 unsigned.
        out = expected_outputs(0xffffffff, 2, 5, 0)
        self.assertEqual((out[4], out[5]), (3, 0))
        self.assertEqual((out[6], out[7]), (3, 2))  # 0x2_0000_0003
        # INT_MIN * INT_MIN = 2^62; addend -1 (all ones).
        out = expected_outputs(0x80000000, 0x80000000, 0xffffffff, 0xffffffff)
        self.assertEqual((out[4], out[5]), (0xffffffff, 0x3fffffff))
        self.assertEqual(out[8:10], out[4:6])


if __name__ == "__main__":
    unittest.main()
