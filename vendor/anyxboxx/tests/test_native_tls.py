"""TLS implicite (variables thread_local / __declspec(thread)) en exécution native.

Le code de test lit sa variable TLS comme le fait le compilateur MSVC :
gs:[0x58] (ThreadLocalStoragePointer) [ _tls_index ] puis la valeur. Chaque thread
doit voir sa propre copie du modèle, y compris un thread né avant le chargement.
"""
import ctypes
import struct
import sys
import threading

import pytest

from pe_builder import build_pe

pytestmark = pytest.mark.skipif(sys.platform != "win32", reason="exécution native sous Windows")

TEMPLATE, INDEX, CALLBACKS, DIRECTORY = 0x100, 0x108, 0x110, 0x140
VALUE = 0x58425841   # « AXBX », valeur initiale de la variable TLS


def tls_reader():
    code = bytearray()
    code += bytes.fromhex("65488B042558000000")                     # mov rax, gs:[0x58]
    code += bytes.fromhex("8B0D") + struct.pack("<i", INDEX - 0xF)  # mov ecx, [rip+_tls_index]
    code += bytes.fromhex("488B04C8")                               # mov rax, [rax+rcx*8]
    code += bytes.fromhex("8B00")                                   # mov eax, [rax]
    code += bytes.fromhex("C3")                                     # ret
    body = bytearray(0x168)
    body[:len(code)] = code
    struct.pack_into("<I", body, TEMPLATE, VALUE)
    struct.pack_into("<I", body, INDEX, 0xFFFFFFFF)
    struct.pack_into("<II", body, DIRECTORY + 32, 4, 0)             # SizeOfZeroFill, Characteristics
    return bytes(body)


@pytest.fixture
def tls_exe(tmp_path, runtime_dir):
    pointers = {DIRECTORY: TEMPLATE, DIRECTORY + 8: TEMPLATE + 4, DIRECTORY + 16: INDEX, DIRECTORY + 24: CALLBACKS}
    return build_pe(tmp_path / "tls.exe", lambda iat: tls_reader(), dll=False, pointers=pointers,
                    tls=DIRECTORY, base=0x1_6000_0000)


def test_thread_local_works_on_the_main_and_new_threads(tls_exe):
    from emu.native import NativeImage

    image = NativeImage(tls_exe)
    try:
        assert image.call_entry() & 0xFFFF_FFFF == VALUE
        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel32.CreateThread.restype = ctypes.c_void_p
        kernel32.CreateThread.argtypes = (ctypes.c_void_p, ctypes.c_size_t, ctypes.c_void_p,
                                          ctypes.c_void_p, ctypes.c_uint32, ctypes.c_void_p)
        thread = kernel32.CreateThread(None, 0, image.entry, None, 0, None)
        kernel32.WaitForSingleObject(ctypes.c_void_p(thread), 5000)
        code = ctypes.c_uint32()
        kernel32.GetExitCodeThread(ctypes.c_void_p(thread), ctypes.byref(code))
        kernel32.CloseHandle(ctypes.c_void_p(thread))
        assert code.value == VALUE                                    # nouveau thread : sa propre copie
    finally:
        image.close()


def test_thread_born_before_loading_also_gets_its_copy(tls_exe):
    from emu.native import NativeImage

    ready, go, result = threading.Event(), threading.Event(), {}

    def early_thread():
        ready.set()
        go.wait(5)
        result["value"] = holder["image"].call_entry() & 0xFFFF_FFFF

    holder = {}
    thread = threading.Thread(target=early_thread)
    thread.start()
    ready.wait(5)
    holder["image"] = NativeImage(tls_exe)
    try:
        go.set()
        thread.join(5)
        assert result.get("value") == VALUE
    finally:
        holder["image"].close()
