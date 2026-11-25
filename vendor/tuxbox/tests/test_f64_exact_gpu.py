import unittest

from tools.check_f64_exact_gpu import bits, reference


class F64ExactReferenceTests(unittest.TestCase):
    # Hand-computed IEEE results, independent of the HLSL bit algorithm.
    def test_rounding(self):
        for name, x, want in [("v_rndne_f64", 2.5, 2.0), ("v_rndne_f64", 3.5, 4.0), ("v_rndne_f64", -0.5, -0.0),
                              ("v_floor_f64", -0.25, -1.0), ("v_ceil_f64", -0.25, -0.0),
                              ("v_trunc_f64", -7.9, -7.0), ("v_floor_f64", 2.0**52 + 1, 2.0**52 + 1)]:
            self.assertEqual(reference(name, bits(x)), bits(want), (name, x))

    def test_denormals_and_nans(self):
        negative_denormal = 0x8000_0000_0000_0001
        self.assertEqual(reference("v_floor_f64", negative_denormal), bits(-1.0))
        self.assertEqual(reference("v_floor_f64", negative_denormal, keep_denormals=False), bits(-0.0))
        self.assertEqual(reference("v_trunc_f64", 0x7ff0_0000_0000_0001), 0x7ff8_0000_0000_0001)
        self.assertEqual(reference("v_cvt_i32_f64", 0x7ff8_0000_0000_0000), 0)

    def test_integer_conversions(self):
        self.assertEqual(reference("v_cvt_i32_f64", bits(-(2.0**31) - 0.5)), 0x8000_0000)
        self.assertEqual(reference("v_cvt_i32_f64", bits(3e9)), 0x7fff_ffff)
        self.assertEqual(reference("v_cvt_u32_f64", bits(-1.0)), 0)
        self.assertEqual(reference("v_cvt_u32_f64", bits(4294967295.75)), 0xffff_ffff)
        self.assertEqual(reference("v_cvt_f64_i32", 0x8000_0000), bits(-(2.0**31)))
        self.assertEqual(reference("v_cvt_f64_u32", 0xffff_ffff << 32), bits(4294967295.0))


if __name__ == "__main__":
    unittest.main()
