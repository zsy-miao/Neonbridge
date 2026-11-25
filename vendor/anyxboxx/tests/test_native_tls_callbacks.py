"""Callbacks TLS du titre en exécution native (first-boot-1021.md, étape 4 et W11).

Le titre mesuré a deux callbacks TLS ; le premier est le __dyn_tls_init du CRT MSVC : il
n'agit que pour DLL_THREAD_ATTACH (raison 2) et lance alors les initialiseurs thread_local
sur le thread qui démarre. Le callback de test fait pareil : il compte chaque raison, garde
le premier argument, et pour la raison 2 écrit un marqueur dans la copie TLS du thread
courant. Un thread qui lit ce marqueur prouve que le callback a tourné chez lui, après
le remplissage de sa zone et avec le bon indice.
"""
import ctypes
import struct
import sys
import threading

import pytest

from pe_builder import Asm, build_pe

pytestmark = pytest.mark.skipif(sys.platform != "win32", reason="exécution native sous Windows")

READ_VALUE, CALLBACK = 0x20, 0x40
TEMPLATE, INDEX, CALLBACKS, COUNTERS, FIRST_ARGUMENT, DIRECTORY = 0x100, 0x108, 0x110, 0x120, 0x130, 0x140
VALUE, MARKER = 0x58425841, 0x7713A4ED
MARKER_OFFSET = 4                   # deuxième dword du bloc : zone mise à zéro (SizeOfZeroFill)
UNSET_INDEX = 0xFFFFFFFF
PROCESS_ATTACH, THREAD_ATTACH, THREAD_DETACH = 1, 2, 3
NEW_THREADS = 4
TLS_ARRAY = bytes.fromhex("65488B042558000000")              # mov rax, gs:[0x58]
SLOT = bytes.fromhex("488B04C8")                             # mov rax, [rax+rcx*8]


def _thread_block(asm):
    """rax = zone TLS du titre pour le thread courant."""
    return asm.raw(TLS_ARRAY).rip(b"\x8b\x0d", INDEX).raw(SLOT)


def tls_callback_title(iat):
    asm = _thread_block(Asm()).raw(b"\x8b\x40" + bytes([MARKER_OFFSET])).ret()    # entrée : le marqueur
    _thread_block(asm.at(READ_VALUE)).raw(b"\x8b\x00").ret()                        # la valeur du modèle
    asm.at(CALLBACK).rip(b"\x48\x89\x0d", FIRST_ARGUMENT)                           # mov [premier], rcx
    asm.raw(b"\x89\xd2").rip(b"\x48\x8d\x05", COUNTERS).raw(b"\xf0\xff\x04\x90")   # lock inc [compteurs+raison*4]
    asm.raw(b"\x83\xfa" + bytes([THREAD_ATTACH]) + b"\x75\x1a")                    # raison 2 seulement
    _thread_block(asm).raw(b"\xc7\x40" + bytes([MARKER_OFFSET]) + struct.pack("<I", MARKER)).ret()
    body = bytearray(asm.at(DIRECTORY + 40).build())
    struct.pack_into("<I", body, TEMPLATE, VALUE)
    struct.pack_into("<I", body, INDEX, UNSET_INDEX)
    struct.pack_into("<II", body, DIRECTORY + 32, MARKER_OFFSET, 0)                # SizeOfZeroFill, Characteristics
    return bytes(body)


@pytest.fixture
def title(tmp_path, runtime_dir):
    from emu.native import NativeImage

    pointers = {DIRECTORY: TEMPLATE, DIRECTORY + 8: TEMPLATE + MARKER_OFFSET, DIRECTORY + 16: INDEX,
                DIRECTORY + 24: CALLBACKS, CALLBACKS: CALLBACK}
    exe = build_pe(tmp_path / "tlscb.exe", tls_callback_title, dll=False, pointers=pointers, tls=DIRECTORY,
                   base=0x1_6100_0000)
    image = NativeImage(exe)
    yield image
    image.close()


def _at(image, offset, kind=ctypes.c_uint32):
    return kind.from_address(image.base + 0x1000 + offset).value


def _counter(image, reason):
    return _at(image, COUNTERS + 4 * reason)


def _run_threads(address, count):
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel32.CreateThread.restype = ctypes.c_void_p
    kernel32.CreateThread.argtypes = (ctypes.c_void_p, ctypes.c_size_t, ctypes.c_void_p,
                                      ctypes.c_void_p, ctypes.c_uint32, ctypes.c_void_p)
    handles = [kernel32.CreateThread(None, 0, address, None, 0, None) for _ in range(count)]
    codes = []
    for handle in handles:
        kernel32.WaitForSingleObject(ctypes.c_void_p(handle), 5000)
        code = ctypes.c_uint32()
        kernel32.GetExitCodeThread(ctypes.c_void_p(handle), ctypes.byref(code))
        kernel32.CloseHandle(ctypes.c_void_p(handle))
        codes.append(code.value)
    return codes


def test_index_in_the_image_is_overwritten_and_attach_reaches_the_title(title):
    assert _at(title, INDEX) != UNSET_INDEX
    assert _counter(title, PROCESS_ATTACH) == 1                  # à l'installation, comme Windows pour un .exe
    assert _at(title, FIRST_ARGUMENT, ctypes.c_uint64) == title.base


def test_every_new_thread_runs_the_callback_with_thread_attach(title):
    attached, detached = _counter(title, THREAD_ATTACH), _counter(title, THREAD_DETACH)
    assert _run_threads(title.entry, NEW_THREADS) == [MARKER] * NEW_THREADS
    assert _counter(title, THREAD_ATTACH) - attached >= NEW_THREADS
    assert _counter(title, THREAD_DETACH) - detached >= NEW_THREADS
    assert _counter(title, PROCESS_ATTACH) == 1


def test_installing_thread_is_left_to_the_title_crt(title):
    """Le thread qui installe reçoit DLL_PROCESS_ATTACH seulement : le CRT MSVC du titre appelle lui-même
    son callback avec DLL_THREAD_ATTACH pour le thread principal (comme sous Windows)."""
    assert title.call_entry() & 0xFFFF_FFFF == 0
    reader = ctypes.WINFUNCTYPE(ctypes.c_uint32)(title.base + 0x1000 + READ_VALUE)
    assert reader() == VALUE


def test_thread_born_before_install_gets_the_template_but_no_attach_call(tmp_path, runtime_dir):
    """Comme Windows pour une DLL chargée après coup : zone remplie, pas de raison 2 sur ce thread.
    Les gardes TLS de MSVC (/Zc:tlsGuards) initialisent alors au premier usage."""
    from emu.native import NativeImage

    ready, go, seen = threading.Event(), threading.Event(), {}

    def early_thread():
        ready.set()
        go.wait(5)
        image = holder["image"]
        seen["marker"] = image.call_entry() & 0xFFFF_FFFF
        seen["value"] = ctypes.WINFUNCTYPE(ctypes.c_uint32)(image.base + 0x1000 + READ_VALUE)()

    holder = {}
    thread = threading.Thread(target=early_thread)
    thread.start()
    ready.wait(5)
    pointers = {DIRECTORY: TEMPLATE, DIRECTORY + 8: TEMPLATE + MARKER_OFFSET, DIRECTORY + 16: INDEX,
                DIRECTORY + 24: CALLBACKS, CALLBACKS: CALLBACK}
    exe = build_pe(tmp_path / "early.exe", tls_callback_title, dll=False, pointers=pointers, tls=DIRECTORY,
                   base=0x1_6200_0000)
    holder["image"] = NativeImage(exe)
    try:
        go.set()
        thread.join(5)
        assert seen == {"marker": 0, "value": VALUE}
    finally:
        holder["image"].close()
