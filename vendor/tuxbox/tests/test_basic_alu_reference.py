import unittest

from tools.check_basic_alu_gpu import (COMPARES, FLOAT_COMPARES, FLOATS, OPERATIONS, SCALAR, Float,
                                       chunks_of, count, fused, is_float, matches, results)


class AluReferenceTests(unittest.TestCase):
    # Hand-computed values, independent of the model's formulas.
    def test_shift_add_carry_counts_shifted_out_bits(self):
        self.assertEqual(results("s_lshl1_add_u32", [0x80000000, 2, 0]), [2, 1])
        self.assertEqual(results("s_lshl4_add_u32", [1, 2, 0]), [18, 0])

    def test_bit_operations(self):
        self.assertEqual(results("v_ffbh_u32", [1, 0, 0]), [31])
        self.assertEqual(results("v_ffbl_b32", [0, 0, 0]), [0xffffffff])
        self.assertEqual(results("v_alignbit_b32", [0x1, 0x80000000, 31]), [3])

    def test_float_rules(self):
        quiet = 0x7fc00000
        self.assertTrue(FLOATS["v_min_f32"][1](quiet, 0x3f800000, 0).matches(0x3f800000))
        self.assertEqual(results("v_cvt_i32_f32", [quiet, 0, 0]), [0])
        self.assertEqual(results("v_cvt_u32_f32", [0xbf800000, 0, 0]), [0])
        self.assertEqual(results("v_cvt_i32_f32", [0x4f800000, 0, 0]), [0x7fffffff])
        # 1 + 2^-24 squared rounds differently when rounded once.
        self.assertEqual(fused(0x3f800001, 0x3f800001, 0xbf800002), 0x28800000)

    def test_expected_floats_allow_only_what_the_console_leaves_open(self):
        self.assertTrue(Float(0x7fc00000).matches(0xffc00001))
        self.assertTrue(Float(0x00000001).matches(0))
        self.assertFalse(Float(0x00000000).matches(0x80000000))
        self.assertTrue(Float(0x00000000, any_zero=True).matches(0x80000000))
        self.assertTrue(Float(0x3f800000, ulps=1).matches(0x3f800001))
        self.assertFalse(matches(0x3f800001, Float(0x3f800000)))

    def test_unordered_comparisons(self):
        quiet = 0x7fc00000
        self.assertEqual(results("v_cmp_lt_f32", [quiet, 0, 0]), [0])
        self.assertEqual(results("v_cmp_nlt_f32", [quiet, 0, 0]), [1])
        self.assertEqual(results("v_cmp_lg_f32", [quiet, 0, 0]), [0])
        self.assertEqual(results("v_cmp_neq_f32", [quiet, 0, 0]), [1])

    def test_each_shader_holds_one_kind_of_input(self):
        names = [*OPERATIONS, *SCALAR, *COMPARES, *FLOATS, *FLOAT_COMPARES]
        chunks = list(chunks_of(names))
        self.assertEqual(sorted(n for chunk in chunks for n in chunk), sorted(names))
        for chunk in chunks:
            self.assertEqual(len({is_float(name) for name in chunk}), 1)
            self.assertLessEqual(sum(count(name) for name in chunk), 8)


if __name__ == "__main__":
    unittest.main()
