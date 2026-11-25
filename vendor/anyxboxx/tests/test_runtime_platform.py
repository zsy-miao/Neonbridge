"""Runtime GDK : état HDR de l'écran (XDisplay) et connectivité réseau (XNetworking) du PC.

Structures et signatures : XDisplay.h et XNetworking.h du GDK public (version 261000).
Les valeurs viennent de Windows (DXGI, Network List Manager) : rien n'est inventé.
"""
import ctypes
import math

import pytest
from test_runtime_cpp import S_OK, GdkInterface, load
from test_runtime_events import E_INVALIDARG, E_NOTIMPL, EVENT_CALLBACK

HDR = ("03F0FE74-FDD9-4E5C-B630-F9339C47ACC5", "35F07670-706E-4BFB-9476-090798C5EBF3")
NETWORKING = ("37E56907-2F10-41E8-B72F-36EDB185331A", "BF2346B2-39AF-4658-B5EA-44713C7E83B3")
HDR_UNKNOWN, HDR_ENABLED, HDR_DISABLED = 0, 1, 2
PREFER_HDR = 0
FALLBACK_LUMINANCE = (0.01, 1499.0, 799.0)  # repli de DXVK (NormalizeDisplayMetadata)
LEVEL_NONE, LEVEL_CONSTRAINED = 1, 4


class HdrModeInfo(ctypes.Structure):
    _fields_ = [("min_luminance", ctypes.c_float), ("max_luminance", ctypes.c_float),
                ("max_full_frame_luminance", ctypes.c_float)]


class ConnectivityHint(ctypes.Structure):
    _fields_ = [("level", ctypes.c_uint32), ("cost", ctypes.c_uint32), ("iana_interface_type", ctypes.c_uint32),
                ("network_initialized", ctypes.c_bool), ("approaching_data_limit", ctypes.c_bool),
                ("over_data_limit", ctypes.c_bool), ("roaming", ctypes.c_bool)]


def test_hdr_mode_reports_the_pc_display(runtime_dir):
    try_enable = GdkInterface(runtime_dir, *HDR).method(4, ctypes.c_uint32, ctypes.c_uint32,
                                                        ctypes.POINTER(HdrModeInfo))
    info = HdrModeInfo()
    result = try_enable(PREFER_HDR, ctypes.byref(info))
    assert result in (HDR_UNKNOWN, HDR_ENABLED, HDR_DISABLED)
    if result == HDR_ENABLED:
        assert info.max_luminance > info.min_luminance >= 0
    assert try_enable(PREFER_HDR, None) == result                   # infos facultatives


def _normalized(runtime_dir, minimum, maximum, full_frame):
    normalize = load(runtime_dir, "xgameruntime.dll").AnyXboxX_NormalizeHdrLuminance
    normalize.restype, normalize.argtypes = None, [ctypes.POINTER(HdrModeInfo)]
    info = HdrModeInfo(minimum, maximum, full_frame)
    normalize(ctypes.byref(info))
    return info.min_luminance, info.max_luminance, info.max_full_frame_luminance


@pytest.mark.parametrize("reported", [
    (0.0, 0.0, 0.0),                 # écran sans métadonnées HDR statiques (issue #191, RTX 5070)
    (0.5, 0.5, 0.0),                 # pic égal au minimum
    (0.05, 0.02, 300.0),             # pic sous le minimum
    (-1.0, 600.0, 400.0),            # minimum négatif
    (math.nan, 600.0, 400.0),
    (0.01, math.nan, 400.0),
])
def test_hdr_luminance_without_usable_display_metadata_falls_back(runtime_dir, reported):
    assert _normalized(runtime_dir, *reported) == pytest.approx(FALLBACK_LUMINANCE)


@pytest.mark.parametrize("reported, expected", [
    ((0.0006, 993.5, 276.8), (0.0006, 993.5, 276.8)),   # valeurs réelles d'un écran HDR : inchangées
    ((0.0, 400.0, 400.0), (0.0, 400.0, 400.0)),         # minimum 0 accepté quand le pic est connu
    ((0.01, 1000.0, 0.0), (0.01, 1000.0, 799.0)),       # plein écran absent : repli
    ((0.01, 600.0, 0.0), (0.01, 600.0, 600.0)),         # repli plein écran plafonné au pic
    ((0.01, 400.0, 800.0), (0.01, 400.0, 400.0)),       # plein écran au-dessus du pic
])
def test_hdr_luminance_keeps_coherent_display_values(runtime_dir, reported, expected):
    assert _normalized(runtime_dir, *reported) == pytest.approx(expected, rel=1e-6)


def test_connectivity_hint_comes_from_windows(runtime_dir):
    get = GdkInterface(runtime_dir, *NETWORKING).method(15, ctypes.c_uint32, ctypes.POINTER(ConnectivityHint))
    hint = ConnectivityHint()
    assert get(ctypes.byref(hint)) == S_OK
    assert LEVEL_NONE <= hint.level <= LEVEL_CONSTRAINED and hint.network_initialized
    assert get(None) == E_INVALIDARG


def test_connectivity_changes_can_be_subscribed(runtime_dir):
    api = GdkInterface(runtime_dir, *NETWORKING)
    subscribe = api.method(16, ctypes.c_uint32, ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p,
                           ctypes.POINTER(ctypes.c_uint64))
    unsubscribe = api.method(17, ctypes.c_bool, ctypes.c_uint64, ctypes.c_bool)
    callback, token = EVENT_CALLBACK(lambda context, hint: None), ctypes.c_uint64()
    assert subscribe(None, None, ctypes.cast(callback, ctypes.c_void_p), ctypes.byref(token)) == S_OK
    assert unsubscribe(token.value, True) is True and unsubscribe(token.value, True) is False


def test_other_networking_methods_are_not_implemented(runtime_dir):
    port = GdkInterface(runtime_dir, *NETWORKING).method(3, ctypes.c_uint32, ctypes.c_void_p)
    assert port(None) == E_NOTIMPL                     # XNetworkingQueryPreferredLocalUdpMultiplayerPort
