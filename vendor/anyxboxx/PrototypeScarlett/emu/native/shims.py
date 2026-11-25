"""Fonctions du chargeur de modules qu'AnyXboxX substitue en exécution native.

LoadLibrary, GetProcAddress, GetModuleHandle et GetModuleFileName : le jeu chargé
à la main doit se voir lui-même et non python.exe, et `xgameruntime.dll` doit être
celui d'AnyXboxX (runtime C++, voir runtime.py) et non celui de Windows.
"""
import ctypes
import os
from ctypes import c_uint32, c_void_p

from . import win32
from . import redirects, runtime
from .runtime import GAME_RUNTIME

LOAD_W = ctypes.WINFUNCTYPE(c_void_p, c_void_p, c_void_p, c_uint32)
LOAD_W1 = ctypes.WINFUNCTYPE(c_void_p, c_void_p)
PROC = ctypes.WINFUNCTYPE(c_void_p, c_void_p, c_void_p)
HANDLE = ctypes.WINFUNCTYPE(c_void_p, c_void_p)
FILENAME = ctypes.WINFUNCTYPE(c_uint32, c_void_p, c_void_p, c_uint32)
LOADER_FUNCTIONS = frozenset({
    "LoadLibraryExW", "LoadLibraryW", "LoadLibraryExA", "LoadLibraryA", "GetProcAddress",
    "GetModuleHandleW", "GetModuleHandleA", "GetModuleFileNameW", "GetModuleFileNameA",
})


def _is_runtime(name):
    return name.replace("/", "\\").rsplit("\\", 1)[-1].lower().startswith(GAME_RUNTIME)


class Shims:
    def __init__(self, image):
        self.image = image
        self.log = image.log
        self.trace = os.environ.get("ANYXBOXX_TRACE") == "1"
        self.runtime = runtime.shared()
        self.runtime_handle = self.runtime.handles["xgameruntime.dll"]
        self._keep = []
        self.loader = {name: self._make(name) for name in LOADER_FUNCTIONS}

    def _keepalive(self, callback):
        self._keep.append(callback)   # sans référence, ctypes libérerait le trampoline
        return win32.callback_address(callback)

    # --- Chargeur de modules ----------------------------------------------

    def _make(self, name):
        return self._keepalive(getattr(self, f"_{name}")())

    def _load(self, name, real):
        if self.trace:
            self.log(f"[trace] LoadLibrary {name}")
        if _is_runtime(name):
            self.log(f"[natif] {name} → runtime GDK d'AnyXboxX (Xbox Series X)")
            return self.runtime_handle
        return real()

    def _LoadLibraryExW(self):
        return LOAD_W(lambda p, f, flags: self._load(ctypes.wstring_at(p), lambda: win32.LoadLibraryExW(p, f, flags)))

    def _LoadLibraryW(self):
        return LOAD_W1(lambda p: self._load(ctypes.wstring_at(p), lambda: win32.LoadLibraryExW(p, None, 0)))

    def _LoadLibraryExA(self):
        return LOAD_W(lambda p, f, flags: self._load(ctypes.string_at(p).decode(errors="replace"),
                                                      lambda: win32.LoadLibraryExA(p, f, flags)))

    def _LoadLibraryA(self):
        return LOAD_W1(lambda p: self._load(ctypes.string_at(p).decode(errors="replace"),
                                            lambda: win32.LoadLibraryExA(p, None, 0)))

    def _GetProcAddress(self):
        def get_proc(module, name):
            name = name or 0
            label = f"#{name}" if name <= 0xFFFF else ctypes.string_at(name).decode(errors="replace")
            if self.trace:
                self.log(f"[trace] GetProcAddress {module or 0:#x} {label}")
            if label in redirects.DXGI_FUNCTIONS and redirects.measuring_frames()                     and module == win32.GetModuleHandleW("dxgi.dll"):
                return self.runtime.export(redirects.EXPORT_PREFIX + label)   # compteur d'images
            address = win32.GetProcAddress(module, name)
            if not address and self.runtime.owns(module):
                self.log(f"[natif] {label} pas encore fourni par le runtime d'AnyXboxX")
                win32.SetLastError(win32.ERROR_PROC_NOT_FOUND)
            return address
        return PROC(get_proc)

    def _module_handle(self, pointer, decode, real):
        if not pointer:
            return self.image.base
        return self.runtime_handle if _is_runtime(decode(pointer)) else real(pointer)

    def _GetModuleHandleW(self):
        return HANDLE(lambda p: self._module_handle(p, ctypes.wstring_at, win32.GetModuleHandleW))

    def _GetModuleHandleA(self):
        return HANDLE(lambda p: self._module_handle(
            p, lambda q: ctypes.string_at(q).decode(errors="replace"), win32.GetModuleHandleA))

    def _file_name(self, module, buffer, size, wide, real):
        if module and module != self.image.base:
            return real(module, buffer, size)
        if not buffer or size == 0:
            return 0
        path = str(self.image.path)[:size - 1]
        data = path.encode("utf-16-le") + b"\0\0" if wide else path.encode("mbcs", errors="replace") + b"\0"
        ctypes.memmove(buffer, data, len(data))
        return len(path)

    def _GetModuleFileNameW(self):
        return FILENAME(lambda m, b, s: self._file_name(m, b, s, True, win32.GetModuleFileNameW))

    def _GetModuleFileNameA(self):
        return FILENAME(lambda m, b, s: self._file_name(m, b, s, False, win32.GetModuleFileNameA))


def _overwrite(pointer, data, capacity):
    if not pointer or len(data) > capacity:
        return False
    ctypes.memmove(pointer, data, len(data))
    return True


def adopt_command_line(exe_path):
    """Le processus prend la ligne de commande du jeu, comme sous Wine.

    GetCommandLineW/A renvoient des tampons propres au processus, que le CRT et
    toutes les DLL (même chargées par Windows, comme SDL3.dll) relisent. On les
    réécrit sur place : la ligne du jeu, `"chemin"`, est plus courte que celle de
    python qui le contient déjà. Renvoie False si un tampon est trop petit.
    """
    line = f'"{exe_path}"'
    wide, narrow = win32.GetCommandLineW(), win32.GetCommandLineA()
    wide_ok = _overwrite(wide, line.encode("utf-16-le") + b"\0\0",
                         2 * len(ctypes.wstring_at(wide)) + 2 if wide else 0)
    narrow_ok = _overwrite(narrow, line.encode("mbcs", errors="replace") + b"\0",
                           len(ctypes.string_at(narrow)) + 1 if narrow else 0)
    return wide_ok and narrow_ok
