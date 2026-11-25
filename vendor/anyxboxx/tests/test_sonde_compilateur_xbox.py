"""Tests for the local synthetic Xbox shader compiler probe."""
import importlib.util
from pathlib import Path
from types import SimpleNamespace

spec = importlib.util.spec_from_file_location(
    "shader_probe", Path(__file__).resolve().parents[1] / "tools" / "sonde_compilateur_xbox.py"
)
probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(probe)


def inputs(tmp_path):
    dxc = tmp_path / "dxc.exe"
    dxc.touch()
    dxc.with_name("dxil.dll").touch()
    dlls = tmp_path / "dlls"
    dlls.mkdir()
    for name in probe.REQUIRED:
        (dlls / name).touch()
    return dxc, dlls


def test_reports_missing_kit_file(tmp_path, capsys):
    dxc, dlls = inputs(tmp_path)
    (dlls / "newbe_xs.dll").unlink()
    assert probe.run(dxc, dlls, None) == 2
    assert "newbe_xs.dll" in capsys.readouterr().err


def test_compiles_synthetic_input_and_checks_native_shader(tmp_path, monkeypatch):
    dxc, dlls = inputs(tmp_path)
    victory = tmp_path / "victory.exe"
    victory.touch()
    calls = []

    def fake_run(cmd, **kwargs):
        calls.append(cmd)
        if len(calls) == 1:
            shader = Path(cmd[-1]).read_text(encoding="ascii")
            assert "SV_Target" in shader
            Path(cmd[cmd.index("-Fo") + 1]).write_bytes(b"synthetic")
            return SimpleNamespace(returncode=0, stdout="", stderr="")
        return SimpleNamespace(returncode=0, stdout="native Ps\nwith native code 1\n", stderr="")

    monkeypatch.setattr(probe.subprocess, "run", fake_run)
    assert probe.run(dxc, dlls, victory) == 0
    assert len(calls) == 2


def test_rejects_output_without_native_shader(tmp_path, monkeypatch):
    dxc, dlls = inputs(tmp_path)
    victory = tmp_path / "victory.exe"
    victory.touch()

    def fake_run(cmd, **kwargs):
        if "-Fo" in cmd:
            Path(cmd[cmd.index("-Fo") + 1]).write_bytes(b"synthetic")
            return SimpleNamespace(returncode=0, stdout="", stderr="")
        return SimpleNamespace(returncode=0, stdout="with native code 0\n", stderr="")

    monkeypatch.setattr(probe.subprocess, "run", fake_run)
    assert probe.run(dxc, dlls, victory) == 1
