import unittest

from tools.check_packed_integer_gpu import load_input


class ShaderInputRewriteTests(unittest.TestCase):
    def test_rewrites_a_single_or_grouped_predicated_move(self):
        single = "    if (exec_lo & 1u) v4 = 0x12345678u;\n"
        grouped = "    if (exec_lo & 1u) {\n        v4 = 0x12345678u;\n        v5 = 0x1u;\n    }\n"
        for text in (single, grouped):
            changed = load_input(text, 4, 0x12345678, "output[0]")
            self.assertIn("v4 = output[0];", changed)
            self.assertNotIn("0x12345678u", changed)

    def test_refuses_a_missing_or_repeated_input(self):
        for text in ("v14 = 0x1u;", "v4 = 0x1u;\nv4 = 0x1u;", "v4 = 0x10u;"):
            with self.assertRaises(ValueError):
                load_input(text, 4, 1, "output[0]")


if __name__ == "__main__":
    unittest.main()
