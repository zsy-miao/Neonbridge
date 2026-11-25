from pathlib import Path
import unittest

from tools.check_translator_gpu import CHECKS, ROOT, command_for, printed_block


class TranslatorCheckListTests(unittest.TestCase):
    def test_every_listed_tool_emitter_and_unit_test_exists(self):
        compiler = ROOT / "tools/shader-compiler"
        sources = "".join(path.read_text(encoding="utf-8") for path in (compiler / "src").rglob("*.rs"))
        for name, check in CHECKS.items():
            self.assertTrue((ROOT / "tools" / f"{check.tool}.py").is_file(), name)
            if check.emitter and check.emitter != "xshader":
                self.assertTrue((compiler / "examples" / f"{check.emitter}.rs").is_file(), name)
            if check.capture:
                self.assertIn(f"fn {check.capture[0]}", sources, name)

    def test_passes_each_tool_only_what_it_takes(self):
        tools = {"llvm-mc": "mc", "dxc": "dxc", "dxv": "dxv"}
        emitters = {check.emitter: Path("/emitters") / str(check.emitter) for check in CHECKS.values()}
        for name in CHECKS:
            command = command_for(name, emitters, tools, {"adapter": "10de:2705"}, Path("/work"))
            source = Path(command[1]).read_text(encoding="utf-8")
            for option in (word for word in command[2:] if word.startswith("--")):
                self.assertIn(option.removeprefix("--"), source, f"{name} {option}")

    def test_reads_the_program_a_unit_test_prints(self):
        output = "running 1 test\nBEGIN_HLSL\nvoid main() {}\n\nEND_HLSL\ntest ok\nBEGIN_HLSL\nsecond\nEND_HLSL\n"
        self.assertEqual(printed_block(output), "void main() {}\n\n")
        self.assertEqual(printed_block("test a::b ... BEGIN_ATOMIC\nbody\nEND_ATOMIC\n"), "body\n")
        with self.assertRaises(ValueError):
            printed_block("running 0 tests\n")


if __name__ == "__main__":
    unittest.main()
