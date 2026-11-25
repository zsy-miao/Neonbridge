"""Runtime C++ d'AnyXboxX (dossier runtime/) : les DLL qui remplacent celles de la console.

Chaque DLL est préchargée par son chemin complet. Windows retrouve ensuite un
module déjà chargé par son nom de base : toute DLL du jeu, même chargée par
Windows lui-même, qui demande `xmem.dll` ou `xgameruntime.dll` reçoit celle
d'AnyXboxX.
"""
import ctypes
import logging
import os
from pathlib import Path

from ..modules import dll_name
from ..tombox_paths import BUILD_COMMAND, TOMBOX_ENV, TOMBOX_INSTALL, TOMBOX_PREFERRED
from . import win32

RUNTIME_DLLS = ("xgameruntime.dll", "xmem.dll", "d3d12_xs.dll", "kernelx.dll", "anyxboxx_tls.dll", "XCRDAPI.dll")
GAME_RUNTIME = "xgameruntime"
D3D12_OVERRIDE = "ANYXBOXX_D3D12_XS"
TOMBOX_DIR = TOMBOX_ENV
REPOSITORY = Path(__file__).resolve().parents[3]
DEFAULT_DIR = REPOSITORY / "runtime" / "build" / "bin"
BUILD_HINT = ("cmake -S runtime -B runtime/build -A x64 && "
              "cmake --build runtime/build --config Release")
log = logging.getLogger(__name__)


# Dépendances de nos DLL : leur propre dossier puis System32, jamais le dossier courant.
SAFE_SEARCH = win32.LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | win32.LOAD_LIBRARY_SEARCH_SYSTEM32
_SHARED = {}


def runtime_dir():
    """Dossier du runtime, toujours absolu : le mode natif change de dossier courant."""
    return Path(os.environ.get("ANYXBOXX_RUNTIME") or DEFAULT_DIR).resolve()


def shared(folder=None):
    """Runtime unique par dossier : chaque image le réutilise au lieu de recharger les DLL."""
    folder = Path(folder).resolve() if folder else runtime_dir()
    if folder not in _SHARED:
        _SHARED[folder] = Runtime(folder)
    return _SHARED[folder]


def _runtime_name(dll):
    """Nom de DLL du runtime qui sert `dll`, ou None (xgameruntime*.dll compris)."""
    name = dll_name(dll.replace("/", "\\").rsplit("\\", 1)[-1])
    if name.startswith(GAME_RUNTIME):
        return "xgameruntime.dll"
    return name if name in RUNTIME_DLLS else None


def tombox_dir():
    """Dossier des DLL de TomBox : ANYXBOXX_TOMBOX_DIR, sinon runtime/build/tombox (tools/build_tombox.py)."""
    return Path(os.environ.get(TOMBOX_DIR) or TOMBOX_INSTALL).resolve()


def tombox_file(dll):
    """DLL console de TomBox du dossier de tombox_dir(), ou None si elle n'est pas compilée."""
    path = tombox_dir() / dll_name(dll)
    return path if path.is_file() else None


def missing_tombox():
    """DLL préférées de TomBox absentes du dossier : le runtime sert alors les nôtres."""
    return [name for name in TOMBOX_PREFERRED if tombox_file(name) is None]


def _dll_path(folder, name):
    """Chemin d'une DLL du runtime : ANYXBOXX_D3D12_XS, puis TomBox si elle sert plus, puis AnyXboxX."""
    alternative = os.environ.get(D3D12_OVERRIDE) if name == "d3d12_xs.dll" else None
    if alternative:
        return Path(alternative).resolve()
    tombox = tombox_file(name) if dll_name(name) in TOMBOX_PREFERRED else None
    return tombox or folder / name


class Runtime:
    def __init__(self, folder=None):
        folder = Path(folder).resolve() if folder else runtime_dir()
        handles = {}
        try:
            for name in RUNTIME_DLLS:
                handles[name] = self._load(_dll_path(folder, name))
        except ValueError:
            for handle in handles.values():   # échec partiel : rien ne reste chargé
                win32.FreeLibrary(handle)
            raise
        self.folder = folder
        self.handles = dict(handles)
        absent = missing_tombox()
        if absent:
            log.warning("DLL de TomBox absentes de %s (%s) : AnyXboxX sert ses propres DLL. Compilez-les : %s",
                        tombox_dir(), ", ".join(absent), BUILD_COMMAND)

    @staticmethod
    def _load(path):
        if not path.is_file():
            raise ValueError(f"Runtime C++ d'AnyXboxX introuvable ({path}). Compilez-le : {BUILD_HINT}")
        handle = win32.LoadLibraryExW(str(path), None, SAFE_SEARCH)
        if not handle:
            raise ValueError(f"Chargement de {path} impossible (erreur {ctypes.get_last_error()}).")
        return handle

    def handle_for(self, dll):
        name = _runtime_name(dll)
        return self.handles.get(name) if name else None

    def owns(self, module):
        return module in self.handles.values()

    def export(self, function, dll="xgameruntime.dll"):
        """Adresse d'un export propre à AnyXboxX dans une DLL du runtime (0 si absent)."""
        return win32.GetProcAddress(self.handles[dll],
                                    ctypes.c_char_p(function.encode())) or 0

    def address(self, dll, function):
        """Adresse de `function` dans la DLL du runtime qui sert `dll` (0 si absente)."""
        handle = self.handle_for(dll)
        if not handle:
            return 0
        symbol = int(function[1:]) if function.startswith("#") else ctypes.c_char_p(function.encode())
        return win32.GetProcAddress(handle, symbol) or 0
