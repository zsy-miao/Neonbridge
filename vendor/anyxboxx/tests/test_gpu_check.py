"""Banc GPU par carte (issue #70) : adaptateur choisi par son nom, --adaptateur et Run-GpuCheck.ps1."""
import os
import re
import subprocess
import sys
from pathlib import Path

import pytest

from emu import rdna2_gpu

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "tools" / "Run-GpuCheck.ps1"
UNKNOWN = "aucune-carte-de-ce-nom-70"
MINIMAL_HLSL = "RWByteAddressBuffer memory : register(u0);\n[numthreads(1, 1, 1)] void main() { memory.Store(0, 1u); }\n"
gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc ou anyxboxx_gpu_run.exe absent")


def describe(runtime_dir, value):
    """Sortie de `anyxboxx_gpu_run.exe --adaptateur` avec ANYXBOXX_GPU=value (None : variable absente)."""
    environment = {key: item for key, item in os.environ.items() if key != "ANYXBOXX_GPU"}
    if value is not None:
        environment["ANYXBOXX_GPU"] = value
    return subprocess.run([str(runtime_dir / "anyxboxx_gpu_run.exe"), "--adaptateur"], capture_output=True,
                          encoding="utf-8", errors="replace", env=environment, timeout=60)


@gpu
def test_a_name_that_matches_no_adapter_is_refused(runtime_dir, monkeypatch):
    monkeypatch.setenv("ANYXBOXX_GPU", UNKNOWN)
    with pytest.raises(RuntimeError, match=UNKNOWN):
        rdna2_gpu.run(MINIMAL_HLSL, bytes(16))


def test_adapter_flag_describes_warp(runtime_dir):
    result = describe(runtime_dir, "warp")
    assert result.returncode == 0, result.stderr
    lines = result.stdout.splitlines()
    assert re.fullmatch(r"adapter=.+ vendor=1414 device=[0-9a-f]{4}", lines[0])
    assert re.fullmatch(r"driver=(\d+\.\d+\.\d+\.\d+|inconnu)", lines[1])
    assert lines[2] == "backend=warp"
    assert re.fullmatch(r"native_lane_range=\d+-\d+", lines[3])
    assert re.fullmatch(r"raytracing_tier=(\d\.\d|aucun)", lines[4])
    assert lines[5] == "double_precision=oui"                       # WARP calcule les doubles


def test_part_of_a_name_in_any_case_picks_that_adapter(runtime_dir):
    default = describe(runtime_dir, None)
    assert default.returncode == 0, default.stderr
    if "backend=hardware" not in default.stdout:
        pytest.skip("pas d'adaptateur matériel par défaut (CI : rendu logiciel)")
    name = re.match(r"adapter=(.+) vendor=", default.stdout).group(1)
    chosen = describe(runtime_dir, name[len(name) // 3:].lower().strip())
    assert chosen.returncode == 0, chosen.stderr
    assert chosen.stdout.splitlines()[0] == default.stdout.splitlines()[0]


def check(arguments, environment=None):
    """Lance tools/Run-GpuCheck.ps1 avec `arguments` (sortie décodée en UTF-8)."""
    return subprocess.run(["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", str(SCRIPT), *arguments],
                          capture_output=True, encoding="utf-8", errors="replace", cwd=ROOT,
                          env=environment or os.environ.copy(), timeout=600)


@gpu
def test_warp_report_lists_every_test_and_ignores_a_stale_variable(runtime_dir, tmp_path):
    report = tmp_path / "rapport.md"
    result = check(["-Backend", "warp", "-ReportPath", str(report), "-Python", sys.executable],
                   {**os.environ, "ANYXBOXX_GPU": UNKNOWN})
    assert result.returncode == 0, result.stdout + result.stderr
    text = report.read_text(encoding="utf-8")
    assert re.search(r"^- Adaptateur : .+ \(vendor=1414 device=[0-9a-f]{4}\)$", text, re.M)
    assert "Backend : warp" in text
    assert "| Test | Résultat |" in text
    assert "| test_rdna2_hlsl::test_compute_shader_matches_the_interpreter | PASS |" in text
    assert "| FAIL |" not in text


@gpu
def test_default_hardware_run_clears_a_stale_variable(runtime_dir, tmp_path):
    report = tmp_path / "rapport.md"
    result = check(["-ReportPath", str(report), "-Python", sys.executable], {**os.environ, "ANYXBOXX_GPU": UNKNOWN})
    assert result.returncode == 0, result.stdout + result.stderr
    default = re.match(r"adapter=(.+) vendor=", describe(runtime_dir, None).stdout).group(1)
    assert f"- Adaptateur : {default} (vendor=" in report.read_text(encoding="utf-8")


def test_tests_that_cannot_run_are_a_failure_not_a_report_of_passes(runtime_dir, tmp_path):
    fake = tmp_path / "python.cmd"
    fake.write_text("@exit /b 4\n", encoding="ascii")
    report = tmp_path / "rapport.md"
    result = check(["-Backend", "warp", "-ReportPath", str(report), "-Python", str(fake)])
    assert result.returncode == 1
    assert "Les tests n'ont pas pu tourner (code 4)" in result.stdout
    assert not report.exists()


@pytest.mark.skipif(sys.platform != "win32", reason="script PowerShell")
def test_missing_runtime_stops_before_any_report(tmp_path):
    report = tmp_path / "rapport.md"
    result = check(["-Backend", "warp", "-ReportPath", str(report)], {**os.environ, "ANYXBOXX_RUNTIME": str(tmp_path)})
    assert result.returncode == 1
    assert "cmake -S runtime -B runtime/build" in result.stdout + result.stderr
    assert not report.exists()


@gpu
def test_report_names_the_backend_that_actually_ran(runtime_dir, tmp_path):
    report = tmp_path / "rapport.md"
    result = check(["-Adapter", "warp", "-ReportPath", str(report), "-Python", sys.executable])
    assert result.returncode == 0, result.stdout + result.stderr
    assert "Backend : warp" in report.read_text(encoding="utf-8")


def test_missing_dxc_stops_before_any_report(runtime_dir, tmp_path):
    report = tmp_path / "rapport.md"
    result = check(["-Backend", "warp", "-ReportPath", str(report), "-Python", sys.executable],
                   {**os.environ, "ANYXBOXX_DXC": str(tmp_path / "absent" / "dxc.exe")})
    assert result.returncode == 1
    assert "ANYXBOXX_DXC" in result.stdout
    assert not report.exists()


def test_a_name_that_matches_no_adapter_stops_before_any_report(runtime_dir, tmp_path):
    report = tmp_path / "rapport.md"
    result = check(["-Adapter", UNKNOWN, "-ReportPath", str(report), "-Python", sys.executable])
    assert result.returncode == 1
    assert UNKNOWN in result.stdout + result.stderr
    assert not report.exists()
