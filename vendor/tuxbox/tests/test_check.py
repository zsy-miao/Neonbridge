import json
from pathlib import Path
from tempfile import TemporaryDirectory
from types import SimpleNamespace
import unittest
from unittest import mock

from project_tuxbox import check
from tools.check_d3d12_regressions import CASES
from project_tuxbox.verify import BASE_SUITES, LANES_SCENARIOS


class CheckCommandTests(unittest.TestCase):
    def run_checks(self, failing=(), **options):
        commands, lines = [], []

        def run(command, **_):
            commands.append(command)
            return SimpleNamespace(returncode=1 if any(word in command for word in failing) else 0)

        with mock.patch.object(check, "host_target", return_value="x86_64-unknown-linux-gnu"), \
                mock.patch.object(check, "find_native_dxc", return_value="/dxc/bin/dxc"), \
                mock.patch.object(check.shutil, "which", return_value="/usr/bin/llvm-mc"):
            code = check.run_checks(run=run, report=lines.append, **options)
        return code, commands, "\n".join(lines)

    def test_runs_code_checks_then_every_suite_on_the_gpu(self):
        code, commands, report = self.run_checks(host_options=["--backend", "proton"])
        self.assertEqual(code, 0)
        self.assertEqual(commands[-2][1:], ["build.py"])
        verify = commands[-1]
        self.assertIn("verify", verify)
        self.assertNotIn("--only", verify)
        self.assertEqual(verify[verify.index("--backend") + 1], "proton")
        self.assertIn("Attach the GPU results", report)

    def test_a_failure_still_runs_the_remaining_checks(self):
        code, commands, report = self.run_checks(failing=("unittest",))
        self.assertEqual(code, 1)
        self.assertIn("verify", commands[-1])
        self.assertIn("FAILED   Python tests", report)

    def test_a_failed_build_skips_the_gpu_run(self):
        code, commands, report = self.run_checks(failing=("build.py",))
        self.assertEqual(code, 1)
        self.assertFalse(any("verify" in command for command in commands))
        self.assertIn("skipped: a build failed", report)

    def test_regression_controls_need_their_inputs(self):
        code, commands, report = self.run_checks(host_options=["--backend", "proton"])
        self.assertEqual(code, 0)
        self.assertIn("Did not run: D3D12 regression controls", report)
        self.assertIn("Did not run: Shader translator on the GPU", report)
        self.assertFalse(any("tools/check_d3d12_regressions.py" in command for command in commands))

    def test_regression_controls_run_every_case_on_the_chosen_adapter(self):
        options = ["--proton", "/proton", "--prefix", "/prefix", "--adapter", "10de:2705"]
        code, commands, report = self.run_checks(host_options=options, windows_dxc="/dxc")
        self.assertEqual(code, 0)
        controls = next(c for c in commands if "tools/check_d3d12_regressions.py" in c)
        for case in CASES:
            self.assertIn(case, controls)
        self.assertEqual(controls[controls.index("--adapter") + 1], "10de:2705")
        self.assertIn("passed   D3D12 regression controls", report)
        translator = commands[-1]
        self.assertIn("tools/check_translator_gpu.py", translator)
        self.assertEqual(translator[translator.index("--adapter") + 1], "10de:2705")

    def test_without_a_gpu_says_the_change_is_not_ready(self):
        code, commands, report = self.run_checks(gpu=False)
        self.assertEqual(code, 0)
        self.assertFalse(any("verify" in command for command in commands))
        self.assertIn("cannot be merged until Lanes passes", report)

    def test_compares_benchmark_timings_with_a_baseline(self):
        with TemporaryDirectory() as directory:
            def results(name, submit):
                path = Path(directory) / name
                receipt = f"lanes-benchmark PASS pci=10de:0001 submit_us={submit} copy64k_us=10.0"
                path.write_text(json.dumps({"checks": [
                    {"gate": "lanes-benchmark", "passed": True, "receipt": receipt}]}))
                return path

            lines = check.compare_benchmarks(results("old.json", "100.0"), results("new.json", "150.0"))
            self.assertEqual(len(lines), 2)
            self.assertIn("+50%", lines[0])
            self.assertIn("+0%", lines[1])
            empty = Path(directory) / "empty.json"
            empty.write_text("{}")
            self.assertEqual(check.compare_benchmarks(empty, empty), ["No benchmark readings to compare."])

    def test_every_lanes_suite_is_part_of_a_default_run(self):
        self.assertNotIn("xbox-shader", LANES_SCENARIOS)
        for suite in ("lanes", "lanes-selftest", "lanes-shader-cache", "lanes-benchmark"):
            self.assertIn(suite, BASE_SUITES)


if __name__ == "__main__":
    unittest.main()
