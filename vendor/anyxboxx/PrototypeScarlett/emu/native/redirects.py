"""Fonctions système du jeu redirigées vers le runtime C++ d'AnyXboxX.

xgameruntime.lib, lié dans le jeu et ses DLL, décide du type de console en lisant
le registre (HKLM\\...\\CurrentVersion\\OEM, valeur ConsoleMode). Ces lectures sont
servies par le registre simulé du runtime C++ (runtime/xgameruntime/registry.cpp),
qui annonce une Xbox Series X et laisse passer tout le reste.

Les imports de l'exécutable sont liés directement ; ceux des DLL du dossier du jeu,
chargées par Windows, sont corrigés dans leur table d'imports (IAT) après chargement.
"""
import ctypes
import os

import pefile

from ..hle.core import family
from ..modules import dll_name
from . import win32

REGISTRY_FUNCTIONS = frozenset({"RegOpenKeyW", "RegOpenKeyExW", "RegQueryValueExW", "RegCloseKey"})
REGISTRY_PREFIX = "api-ms-win-core-registry-"
# Compteur d'images (frame_counter.cpp) : seulement si la mesure est demandée.
DXGI_FUNCTIONS = frozenset({"CreateDXGIFactory", "CreateDXGIFactory1", "CreateDXGIFactory2"})
EXPORT_PREFIX = "AnyXboxX_"
DIR_IMPORT = 1
POINTER_SIZE = 8


def _is_registry_dll(dll):
    name = dll_name(dll)
    return name == "advapi32.dll" or name.startswith(REGISTRY_PREFIX)


def redirect(runtime, dll, function, loader=None):
    """Adresse de remplacement de `dll!function` (runtime C++ ou chargeur d'AnyXboxX), ou 0.

    `loader` : fonctions du chargeur (shims) données aux DLL du jeu, pour que leurs
    GetProcAddress dynamiques passent aussi par AnyXboxX (ex. SDL3 et DXGI).
    """
    if loader is not None and function == "GetProcAddress" and family(dll) == "kernel":
        return loader["GetProcAddress"]
    if function in REGISTRY_FUNCTIONS and _is_registry_dll(dll):
        return runtime.export(EXPORT_PREFIX + function)
    if function in DXGI_FUNCTIONS and dll_name(dll) == "dxgi.dll" and measuring_frames():
        return runtime.export(EXPORT_PREFIX + function)
    return 0


def measuring_frames():
    return os.environ.get("ANYXBOXX_FPS") == "1" or bool(os.environ.get("ANYXBOXX_FPS_CSV"))


def _write_pointer(address, value):
    old = ctypes.c_ulong()
    if not win32.VirtualProtect(address, POINTER_SIZE, win32.PAGE_READWRITE, ctypes.byref(old)):
        raise ValueError(f"IAT non modifiable à {address:#x} (erreur {ctypes.get_last_error()}).")
    try:
        win32.write_u64(address, value)
    finally:
        win32.VirtualProtect(address, POINTER_SIZE, old.value, ctypes.byref(old))


def patch_module(runtime, handle, path, loader=None):
    """Corrige l'IAT d'une DLL du jeu déjà chargée par Windows. Renvoie le nombre d'entrées."""
    with pefile.PE(str(path), fast_load=True) as pe:
        pe.parse_data_directories(directories=[DIR_IMPORT])
        image_base = pe.OPTIONAL_HEADER.ImageBase
        patched = 0
        for entry in getattr(pe, "DIRECTORY_ENTRY_IMPORT", ()):
            dll = entry.dll.decode(errors="replace")
            for item in entry.imports:
                if item.name is None:
                    continue
                target = redirect(runtime, dll, item.name.decode(errors="replace"), loader)
                if target:
                    _write_pointer(handle + item.address - image_base, target)
                    patched += 1
    return patched
