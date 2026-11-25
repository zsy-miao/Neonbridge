"""Ordre de chargement des DLL de TomBox : emplacement par défaut, variable d'environnement, absence."""
import importlib.util
import sys
from pathlib import Path

import pytest

from emu import tombox_paths

if sys.platform == "win32":
    from emu.native import runtime
else:
    runtime = None

pytestmark = pytest.mark.skipif(runtime is None, reason="exécution native sous Windows")

ROOT = Path(__file__).resolve().parents[1]
CI_CRATES = ("d3d12_xs", "xg_xs", "xmem", "d3d12_x", "xaudio2_9", "xapu", "xgs12", "xcrdapi", "acphal", "chad")


def build_script():
    spec = importlib.util.spec_from_file_location("build_tombox", ROOT / "tools" / "build_tombox.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_default_folder_is_the_build_output(monkeypatch):
    monkeypatch.delenv(tombox_paths.TOMBOX_ENV, raising=False)
    assert runtime.tombox_dir() == tombox_paths.TOMBOX_INSTALL.resolve()
    assert tombox_paths.TOMBOX_INSTALL == ROOT / "runtime" / "build" / "tombox"


def test_environment_variable_overrides_the_default(tmp_path, monkeypatch):
    monkeypatch.setenv(tombox_paths.TOMBOX_ENV, str(tmp_path))
    assert runtime.tombox_dir() == tmp_path.resolve()


def test_default_folder_serves_preferred_dlls_when_compiled(tmp_path, monkeypatch):
    monkeypatch.delenv(tombox_paths.TOMBOX_ENV, raising=False)
    monkeypatch.setattr(runtime, "TOMBOX_INSTALL", tmp_path)
    (tmp_path / "d3d12_xs.dll").write_bytes(b"MZ")
    ours = tmp_path / "ours"
    assert runtime._dll_path(ours, "d3d12_xs.dll") == (tmp_path / "d3d12_xs.dll").resolve()
    assert runtime._dll_path(ours, "XCRDAPI.dll") == ours / "XCRDAPI.dll"    # pas encore compilée : la nôtre
    assert runtime.missing_tombox() == ["xmem.dll", "xcrdapi.dll"]


def test_a_missing_default_folder_falls_back_to_our_dlls(tmp_path, monkeypatch):
    monkeypatch.delenv(tombox_paths.TOMBOX_ENV, raising=False)
    monkeypatch.setattr(runtime, "TOMBOX_INSTALL", tmp_path / "absent")
    ours = tmp_path / "ours"
    assert runtime.tombox_file("d3d12_xs.dll") is None
    assert runtime._dll_path(ours, "d3d12_xs.dll") == ours / "d3d12_xs.dll"
    assert runtime.missing_tombox() == list(tombox_paths.TOMBOX_PREFERRED)


def test_tombox_preferred_dlls_are_the_ones_tombox_serves_more_completely():
    assert set(tombox_paths.TOMBOX_PREFERRED) == {"d3d12_xs.dll", "xmem.dll", "xcrdapi.dll"}
    assert "xgameruntime.dll" not in tombox_paths.TOMBOX_PREFERRED


def test_default_crates_include_every_crate_of_the_ci():
    assert set(CI_CRATES) <= set(build_script().DEFAULT_CRATES)


def test_install_copies_compiled_dlls_under_lowercase_names(tmp_path):
    script = build_script()
    output = tmp_path / "release"
    output.mkdir()
    (output / "XAudio2_9.dll").write_bytes(b"MZ")
    (output / "d3d12_xs.dll").write_bytes(b"MZ")
    destination = tmp_path / "tombox"
    copied = script.install(("xaudio2_9", "d3d12_xs"), output, destination)
    assert sorted(path.name for path in copied) == ["d3d12_xs.dll", "xaudio2_9.dll"]
    assert (destination / "xaudio2_9.dll").read_bytes() == b"MZ"


def test_install_names_the_dll_that_was_not_compiled(tmp_path):
    script = build_script()
    output = tmp_path / "release"
    output.mkdir()
    with pytest.raises(RuntimeError, match="xapu.dll absente"):
        script.install(("xapu",), output, tmp_path / "tombox")
