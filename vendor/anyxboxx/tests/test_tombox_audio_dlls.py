"""DLL audio console de TomBox (W14 : AcpHal.dll et chad.dll, compilées par tools/build_tombox.py).

Vérifie la présence des 9 exports d'AcpHal.dll et des 5 exports de chad.dll importés par les
exécutables Scarlett, ainsi qu'un cycle complet d'allocation APU (ApuCreateHeap → ApuAlloc →
ApuMapVirtualAddress / ApuMapApuAddress → ApuFree), AcpHalCreate et XDspConnect/XDspDisconnect.
"""
import ctypes
import os
import sys

import pytest

from emu import tombox_paths

pytestmark = pytest.mark.skipif(sys.platform != "win32", reason="chargement DLL Windows uniquement")

ACPHAL_EXPORTS = (
    "AcpHalCreate",
    "AcpHalAllocateShapeContexts",
    "AcpHalReleaseShapeContexts",
    "ApuAlloc",
    "ApuCreateHeap",
    "ApuFree",
    "ApuIsVirtualAddressValid",
    "ApuMapApuAddress",
    "ApuMapVirtualAddress",
)
CHAD_EXPORTS = (
    "XDspActivate",
    "XDspConnectWithMaximumStreamLimit",
    "XDspDeactivate",
    "XDspDisconnect",
    "XDspSubmitCommand",
)


def _require_tombox_dll(name):
    path = tombox_paths.TOMBOX_INSTALL / name
    if not path.is_file():
        if os.environ.get("ANYXBOXX_REQUIRE_RUNTIME") == "1":
            pytest.fail(f"{name} absente de {tombox_paths.TOMBOX_INSTALL} : {tombox_paths.BUILD_COMMAND}")
        pytest.skip(f"{name} non compilée ({tombox_paths.BUILD_COMMAND})")
    return ctypes.WinDLL(str(path))


def test_acphal_exports_and_apu_heap_round_trip():
    dll = _require_tombox_dll("acphal.dll")
    for symbol in ACPHAL_EXPORTS:
        assert hasattr(dll, symbol), f"export manquant dans acphal.dll : {symbol}"

    dll.ApuCreateHeap.argtypes = [ctypes.c_uint32, ctypes.c_uint32]
    dll.ApuCreateHeap.restype = ctypes.c_int32
    dll.ApuAlloc.argtypes = [
        ctypes.POINTER(ctypes.c_void_p),
        ctypes.POINTER(ctypes.c_uint32),
        ctypes.c_uint32,
        ctypes.c_uint32,
        ctypes.c_uint32,
    ]
    dll.ApuAlloc.restype = ctypes.c_int32
    dll.ApuIsVirtualAddressValid.argtypes = [ctypes.c_void_p, ctypes.c_uint32]
    dll.ApuIsVirtualAddressValid.restype = ctypes.c_int32
    dll.ApuMapVirtualAddress.argtypes = [ctypes.c_void_p]
    dll.ApuMapVirtualAddress.restype = ctypes.c_uint32
    dll.ApuMapApuAddress.argtypes = [ctypes.c_uint32]
    dll.ApuMapApuAddress.restype = ctypes.c_void_p
    dll.ApuFree.argtypes = [ctypes.c_void_p]
    dll.ApuFree.restype = ctypes.c_int32
    dll.AcpHalCreate.argtypes = [ctypes.POINTER(ctypes.c_void_p)]
    dll.AcpHalCreate.restype = ctypes.c_int32

    assert dll.ApuCreateHeap(0x10_0000, 0x10_0000) >= 0

    va = ctypes.c_void_p()
    apu = ctypes.c_uint32()
    # heap = 1 (APU_ALLOC_CACHED), alignement 2048 octets (XMA2)
    assert dll.ApuAlloc(ctypes.byref(va), ctypes.byref(apu), 4096, 2048, 1) == 0
    assert va.value is not None and va.value != 0
    assert dll.ApuIsVirtualAddressValid(va, 2048) != 0
    mapped_apu = dll.ApuMapVirtualAddress(va)
    assert mapped_apu == apu.value and mapped_apu != 0
    assert dll.ApuMapApuAddress(mapped_apu) == va.value
    assert dll.ApuFree(va) == 0

    hal = ctypes.c_void_p()
    assert dll.AcpHalCreate(ctypes.byref(hal)) == 0
    assert hal.value is not None and hal.value != 0


def test_chad_exports_and_xdsp_session_lifecycle():
    dll = _require_tombox_dll("chad.dll")
    for symbol in CHAD_EXPORTS:
        assert hasattr(dll, symbol), f"export manquant dans chad.dll : {symbol}"

    dll.XDspConnectWithMaximumStreamLimit.argtypes = [
        ctypes.c_void_p,
        ctypes.c_uint32,
        ctypes.c_uint32,
        ctypes.c_uint32,
        ctypes.c_uint32,
        ctypes.POINTER(ctypes.c_void_p),
    ]
    dll.XDspConnectWithMaximumStreamLimit.restype = ctypes.c_int32
    dll.XDspDisconnect.argtypes = [ctypes.c_void_p]
    dll.XDspDisconnect.restype = ctypes.c_int32

    scratch = ctypes.create_string_buffer(0x10000)
    client = ctypes.c_void_p()
    assert dll.XDspConnectWithMaximumStreamLimit(scratch, 0x10000, 4, 16, 1, ctypes.byref(client)) == 0
    assert client.value is not None and client.value != 0
    assert dll.XDspDisconnect(client) == 0
    # Client nul refusé avec E_INVALIDARG (< 0)
    assert dll.XDspDisconnect(None) < 0
