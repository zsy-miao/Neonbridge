import unittest

from tools.check_ordinary_integer_gpu import reference, selectors, CLAMP, HALF_OPS, WORD_OPS


class OrdinaryIntegerReferenceTests(unittest.TestCase):
    # Hand-computed values, independent of the model's formulas.
    def test_16_bit_signed_and_unsigned_ordering(self):
        values = [0x8000, 0x7fff, 0x0001]
        for name, result in [("v_min3_u16", 0x0001), ("v_max3_u16", 0x8000), ("v_med3_u16", 0x7fff),
                             ("v_min3_i16", 0x8000), ("v_max3_i16", 0x7fff), ("v_med3_i16", 0x0001)]:
            self.assertEqual(reference(name, values, 0, 0xaaaa0000), 0xaaaa0000 | result, name)

    def test_16_bit_half_selection_preserves_the_other_half(self):
        # Destination high: 0xffff + 1 wraps to 0, low half of the old value stays.
        self.assertEqual(reference("v_add_nc_i16", [0xffff, 0x0001, 0], 0b100, 0x12345678), 0x00005678)
        # Source 0 high: 5 - 7 = -2.
        self.assertEqual(reference("v_sub_nc_i16", [0x00050000, 0x00000007, 0], 0b001, 0), 0x0000fffe)
        self.assertEqual(reference("v_mad_u16", [0x0100, 0x0100, 0x0003], 0, 0xffffffff), 0xffff0003)

    def test_32_bit_results(self):
        self.assertEqual(reference("v_sub_nc_i32", [0, 1, 0], 0, 0), 0xffffffff)
        self.assertEqual(reference("v_add_nc_i32", [0x7fffffff, 1, 0], 0, 0), 0x80000000)
        self.assertEqual(reference("v_mad_i32_i16", [0xffff, 0x0002, 5], 0, 0), 3)
        self.assertEqual(reference("v_mad_i32_i16", [0x8000, 0x8000, 0], 0, 0), 0x40000000)
        self.assertEqual(reference("v_mad_i32_i16", [0, 0, 0xfffffff0], 0, 0), 0xfffffff0)
        self.assertEqual(reference("v_mad_u32_u16", [0xffff0000, 0x0002ffff, 1], 0b11, 0), 0x1ffff)

    def test_signed_saturation(self):
        self.assertEqual(reference("v_add_nc_i32", [0x7fffffff, 1, 0], CLAMP, 0), 0x7fffffff)
        self.assertEqual(reference("v_add_nc_i32", [0x80000000, 0xffffffff, 0], CLAMP, 0), 0x80000000)
        self.assertEqual(reference("v_sub_nc_i32", [0x80000000, 1, 0], CLAMP, 0), 0x80000000)
        self.assertEqual(reference("v_sub_nc_i32", [0x7fffffff, 0xffffffff, 0], CLAMP, 0), 0x7fffffff)
        self.assertEqual(reference("v_add_nc_i32", [5, 0xfffffff9, 0], CLAMP, 0), 0xfffffffe)

    def test_every_selector_is_enumerated(self):
        self.assertEqual(sum(len(selectors(n)) for n in HALF_OPS | WORD_OPS), 156)


if __name__ == "__main__":
    unittest.main()
