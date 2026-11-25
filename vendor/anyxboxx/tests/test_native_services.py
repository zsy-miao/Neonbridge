"""Bibliothèques de jeu dont l'API est la même sur Xbox et sur PC : servies par Windows en mode natif.

GameInput (manettes, clavier, souris) et XAudio2 sont importés par ordinal 1
(GameInputCreate, XAudio2Create) par les DLL du jeu, comme sur PC : le chargeur
natif d'AnyXboxX les lie aux DLL de Windows, sans traduction.
"""
import ctypes
import sys

import pytest

from pe_builder import Asm, build_pe

pytestmark = pytest.mark.skipif(sys.platform != "win32", reason="exécution native sous Windows")

OUT_SLOT = 0x300   # 8 octets réservés dans la section du PE de test


def windows_export(dll, name):
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel32.LoadLibraryW.restype = ctypes.c_void_p
    kernel32.GetProcAddress.restype = ctypes.c_void_p
    kernel32.GetProcAddress.argtypes = (ctypes.c_void_p, ctypes.c_char_p)
    module = kernel32.LoadLibraryW(dll)
    return kernel32.GetProcAddress(module, name.encode()) if module else None


@pytest.mark.parametrize("dll, function", [("GameInput.dll", "GameInputCreate"), ("XAudio2_9.dll", "XAudio2Create")])
def test_ordinal_import_is_bound_to_windows(tmp_path, runtime_dir, dll, function):
    from emu.native import NativeImage

    expected = windows_export(dll, function)
    if not expected:
        pytest.skip(f"{dll} absent de cette machine")
    exe = build_pe(tmp_path / "service.exe", lambda iat: Asm().mov_eax(1).ret().build(), imports=[(dll, 1)], dll=False)
    image = NativeImage(exe)
    try:
        assert image.bound[f"{dll}!#1"] == expected
    finally:
        image.close()


def test_game_input_is_created_through_windows(tmp_path, runtime_dir):
    from emu.native import NativeImage

    if not windows_export("GameInput.dll", "GameInputCreate"):
        pytest.skip("GameInput absent de cette machine")

    def code(iat):
        return (Asm().frame().lea_rcx(OUT_SLOT).call_iat(iat[("GameInput.dll", 1)]).unframe().ret()
                .at(OUT_SLOT).raw(bytes(8)).build())

    exe = build_pe(tmp_path / "input.exe", code, imports=[("GameInput.dll", 1)], dll=False)
    image = NativeImage(exe)
    try:
        assert image.call_entry() & 0xFFFF_FFFF == 0          # S_OK : IGameInput créé
        assert ctypes.c_uint64.from_address(image.base + 0x1000 + OUT_SLOT).value != 0
    finally:
        image.close()


def test_d3d12_override_points_to_another_dll(tmp_path, monkeypatch):
    from emu.native import runtime

    monkeypatch.delenv(runtime.D3D12_OVERRIDE, raising=False)
    assert runtime._dll_path(tmp_path, "d3d12_xs.dll") == tmp_path / "d3d12_xs.dll"
    monkeypatch.setenv(runtime.D3D12_OVERRIDE, str(tmp_path / "tombox" / "d3d12_xs.dll"))
    assert runtime._dll_path(tmp_path, "d3d12_xs.dll") == (tmp_path / "tombox" / "d3d12_xs.dll").resolve()
    assert runtime._dll_path(tmp_path, "xmem.dll") == tmp_path / "xmem.dll"


def test_tombox_dir_supplies_console_dlls(tmp_path, monkeypatch):
    from emu.native import runtime

    (tmp_path / "xmem.dll").write_bytes(b"MZ")
    (tmp_path / "xg_xs.dll").write_bytes(b"MZ")
    monkeypatch.delenv(runtime.D3D12_OVERRIDE, raising=False)
    monkeypatch.setenv(runtime.TOMBOX_DIR, str(tmp_path))
    assert runtime.tombox_file("XG_XS.DLL") == (tmp_path / "xg_xs.dll").resolve()
    assert runtime._dll_path(tmp_path / "ours", "xmem.dll") == (tmp_path / "xmem.dll").resolve()
    assert runtime._dll_path(tmp_path / "ours", "xgameruntime.dll") == tmp_path / "ours" / "xgameruntime.dll"
    assert runtime.tombox_file("absent.dll") is None
