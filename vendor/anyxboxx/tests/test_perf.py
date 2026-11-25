"""Compteur d'images (DXGI redirigé si demandé) et banc de performance (objectif 60 i/s)."""
import sys
import xml.etree.ElementTree as ET

import pytest

from emu import perf
from pe_builder import Asm, build_pe

CSV = """secondes,images,fps,ms_moyen,ms_p99
1.000,58,58.00,17.241,25.000
2.000,60,60.00,16.667,17.500
3.000,61,61.00,16.393,17.000
4.000,60,60.00,16.667,18.000
"""


def test_csv_is_parsed_into_seconds():
    rows = perf.parse_csv(CSV)
    assert [row.fps for row in rows] == [58.0, 60.0, 61.0, 60.0]
    assert rows[1].p99_ms == 17.5


def test_summary_ignores_the_warm_up_second():
    summary = perf.summarize(perf.parse_csv(CSV))
    assert summary.average_fps == pytest.approx(60.33, abs=0.01)
    assert summary.minimum_fps == 60.0
    assert summary.low_fps == pytest.approx(1000 / 18.0, abs=0.01)   # pire « 1 % bas » mesuré
    assert summary.constant_60                                        # chaque seconde ≥ 60 (à 0,5 près)


def test_a_slow_second_breaks_the_60_fps_target():
    slow = CSV + "5.000,45,45.00,22.222,40.000\n"
    assert not perf.summarize(perf.parse_csv(slow)).constant_60


@pytest.mark.parametrize("rows", [0, 1], ids=["vide", "seulement la chauffe"])
def test_a_capture_without_a_second_after_the_warm_up_has_no_verdict(rows):
    with pytest.raises(ValueError, match="chauffe"):
        perf.summarize(perf.parse_csv(CSV)[:rows])
    assert "polyline" in perf.svg(perf.parse_csv(CSV)[:rows], "x")      # la courbe reste tracée, en orange


def test_only_the_seconds_after_the_warm_up_decide_the_verdict():
    rows = perf.parse_csv("secondes,images,fps,ms_moyen,ms_p99\n1.000,60,60.00,16.667,16.667\n2.000,30,30.00,33.333,33.333\n")
    summary = perf.summarize(rows)
    assert (summary.seconds, summary.minimum_fps, summary.constant_60) == (1, 30.0, False)


def test_chart_is_valid_svg_with_the_60_fps_line():
    image = perf.svg(perf.parse_csv(CSV), "SimpleTriangle")
    root = ET.fromstring(image)
    assert root.tag.endswith("svg")
    assert "60 fps" in image and "SimpleTriangle" in image


@pytest.mark.skipif(sys.platform != "win32", reason="exécution native sous Windows")
@pytest.mark.parametrize("enabled", [True, False])
def test_dxgi_is_redirected_only_when_measuring(tmp_path, runtime_dir, monkeypatch, enabled):
    from emu.native import NativeImage, runtime

    monkeypatch.delenv("ANYXBOXX_FPS_CSV", raising=False)
    if enabled:
        monkeypatch.setenv("ANYXBOXX_FPS", "1")
    else:
        monkeypatch.delenv("ANYXBOXX_FPS", raising=False)
    exe = build_pe(tmp_path / "dxgi.exe", lambda iat: Asm().mov_eax(1).ret().build(),
                   imports=[("dxgi.dll", "CreateDXGIFactory2")], dll=False)
    image = NativeImage(exe)
    try:
        counted = runtime.shared().export("AnyXboxX_CreateDXGIFactory2")
        assert (image.bound["dxgi.dll!CreateDXGIFactory2"] == counted) is enabled
    finally:
        image.close()


@pytest.mark.skipif(sys.platform != "win32", reason="exécution native sous Windows")
def test_game_dlls_get_anyxboxx_get_proc_address(tmp_path, runtime_dir):
    """SDL3.dll charge DXGI par GetProcAddress : ses appels doivent passer par AnyXboxX."""
    import ctypes

    from emu.native import NativeImage
    from test_native import DLL_MAIN, _iat_slot

    helper = build_pe(tmp_path / "dyn.dll", lambda iat: Asm().mov_eax(1).ret().at(DLL_MAIN).mov_eax(1).ret().build(),
                      imports=[("KERNEL32.dll", "GetProcAddress")], exports=[("Nothing", 0)], entry=DLL_MAIN)
    exe = build_pe(tmp_path / "usesdyn.exe", lambda iat: Asm().mov_eax(1).ret().build(),
                   imports=[("dyn.dll", "Nothing")], dll=False)
    image = NativeImage(exe)
    try:
        kernel32 = ctypes.WinDLL("kernel32")
        kernel32.GetModuleHandleW.restype = ctypes.c_void_p
        slot = _iat_slot(helper, kernel32.GetModuleHandleW(str(helper)), "KERNEL32.dll", "GetProcAddress")
        assert ctypes.c_uint64.from_address(slot).value == image.shims.loader["GetProcAddress"]
    finally:
        image.close()
