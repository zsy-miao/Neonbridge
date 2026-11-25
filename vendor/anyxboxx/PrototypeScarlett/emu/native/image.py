"""Chargeur PE natif façon Wine : l'image est mappée dans le processus et exécutée
directement par le processeur hôte.

ATTENTION : rien n'est isolé. Le code chargé a les mêmes droits que l'utilisateur.
N'exécuter que des programmes de confiance (par exemple les échantillons MIT).
"""
import ctypes
from pathlib import Path

import pefile

from ..hle.core import family
from ..modules import dll_name, is_system
from . import win32
from . import redirects
from . import runtime as runtime_dlls
from .shims import LOADER_FUNCTIONS, Shims

MAX_IMAGE = 512 * 1024 * 1024
DIR_IMPORT, DIR_EXCEPTION, DIR_RELOC, DIR_TLS, DIR_CLR = 1, 3, 5, 9, 14
RUNTIME_FUNCTION_SIZE = 12
SCN_EXEC, SCN_READ, SCN_WRITE = 0x20000000, 0x40000000, 0x80000000
PAGE = 0x1000


class UntranslatedImport(ValueError):
    """Import d'une DLL console que le runtime C++ ne fournit pas encore."""


def _align(value):
    return (value + PAGE - 1) // PAGE * PAGE


def _page_protection(flags):
    if flags & SCN_EXEC:
        return win32.PAGE_EXECUTE_READWRITE if flags & SCN_WRITE else win32.PAGE_EXECUTE_READ
    return win32.PAGE_READWRITE if flags & SCN_WRITE else win32.PAGE_READONLY


class NativeImage:
    def __init__(self, path, search_dirs=None, log=print):
        self.path = Path(path).resolve()
        self.log = log
        self.search = {}
        for folder in search_dirs or (self.path.parent,):
            folder = Path(folder)
            if folder.is_dir():
                for candidate in folder.iterdir():
                    self.search.setdefault(candidate.name.lower(), candidate)
        self._function_table = None
        self.base = None
        self.shims = None
        with pefile.PE(str(self.path), fast_load=True) as pe:
            self._check(pe)
            pe.parse_data_directories(directories=[DIR_IMPORT, DIR_RELOC])
            try:
                self._map(pe)
                self.shims = Shims(self)
                self._bind(pe)
                self._protect(pe)
                self._register_exceptions(pe)
                self._install_tls(pe)
            except BaseException:
                self.close()   # sinon l'image reste réservée et bloque sa base
                raise
            self.entry = self.base + pe.OPTIONAL_HEADER.AddressOfEntryPoint
            self.is_dll = bool(pe.FILE_HEADER.Characteristics & 0x2000)

    # --- Chargement ---------------------------------------------------------

    @staticmethod
    def _check(pe):
        header = pe.OPTIONAL_HEADER
        if pe.FILE_HEADER.Machine != 0x8664 or header.Magic != 0x20B:
            raise ValueError("Exécution native : seul le format PE32+ x86-64 est accepté.")
        if not 0 < header.SizeOfImage <= MAX_IMAGE:
            raise ValueError("Exécution native : taille d’image invalide.")
        dirs = header.DATA_DIRECTORY
        for index, feature in ((DIR_CLR, ".NET"),):
            if index < len(dirs) and dirs[index].VirtualAddress:
                raise ValueError(f"Exécution native : {feature} pas encore pris en charge.")

    def _map(self, pe):
        header = pe.OPTIONAL_HEADER
        size = _align(header.SizeOfImage)
        self.size = size
        self.base = win32.VirtualAlloc(header.ImageBase, size, win32.MEM_COMMIT_RESERVE, win32.PAGE_READWRITE)
        if not self.base:
            if not getattr(pe, "DIRECTORY_ENTRY_BASERELOC", None):
                raise ValueError("Base préférée occupée et aucune table de relocation.")
            self.base = win32.VirtualAlloc(None, size, win32.MEM_COMMIT_RESERVE, win32.PAGE_READWRITE)
        if not self.base:
            raise MemoryError("VirtualAlloc a échoué pour l’image native.")
        # Les adresses d'IAT sont relevées avant la relocation, qui modifie pefile.
        self._imports = [
            (library.dll.decode(errors="replace"),
             item.name.decode(errors="replace") if item.name else f"#{item.ordinal}",
             item.address - header.ImageBase)
            for library in getattr(pe, "DIRECTORY_ENTRY_IMPORT", [])
            for item in library.imports
        ]
        data = pe.get_memory_mapped_image(ImageBase=self.base)
        ctypes.memmove(self.base, data, min(len(data), size))

    def _bind(self, pe):
        """Lie chaque import ; les fonctions console pas encore traduites sont listées ensemble."""
        self.bound = {}
        untranslated = []
        for dll, name, iat_rva in self._imports:
            try:
                address = self.resolve(dll, name)
            except UntranslatedImport:
                untranslated.append(f"{dll}!{name}")
                continue
            win32.write_u64(self.base + iat_rva, address)
            self.bound[f"{dll}!{name}"] = address
        if untranslated:
            raise UntranslatedImport("Exécution native : fonctions console pas encore traduites par "
                                     f"AnyXboxX ({len(untranslated)}) : " + ", ".join(untranslated))

    def resolve(self, dll, name):
        runtime = self.shims.runtime
        if runtime.handle_for(dll):
            address = runtime.address(dll, name)
            if not address:
                raise UntranslatedImport(f"{dll}!{name}")
            return address
        replacement = redirects.redirect(runtime, dll, name)
        if replacement:
            return replacement
        if family(dll) == "kernel" and name in LOADER_FUNCTIONS:
            return self.shims.loader[name]
        module = self._load_module(dll)
        symbol = int(name[1:]) if name.startswith("#") else ctypes.c_char_p(name.encode())
        address = win32.GetProcAddress(module, symbol)
        if not address:
            raise ValueError(f"Exécution native : {dll}!{name} introuvable.")
        return address

    def _load_module(self, dll):
        local = None if is_system(dll) else self.search.get(dll_name(dll))
        if local is not None:
            handle = win32.LoadLibraryExW(str(local), None, win32.LOAD_WITH_ALTERED_SEARCH_PATH)
            if handle:
                redirects.patch_module(self.shims.runtime, handle, local, self.shims.loader)   # registre, DXGI…
        elif not is_system(dll) and runtime_dlls.tombox_file(dll) is not None:
            handle = win32.LoadLibraryExW(str(runtime_dlls.tombox_file(dll)), None, runtime_dlls.SAFE_SEARCH)
        else:
            handle = win32.LoadLibraryExW(dll, None, 0)
        if not handle:
            raise ValueError(f"Exécution native : DLL {dll} introuvable "
                             f"(erreur {ctypes.get_last_error()}).")
        return handle

    def _protect(self, pe):
        old = ctypes.c_ulong()
        win32.VirtualProtect(self.base, _align(pe.OPTIONAL_HEADER.SizeOfHeaders),
                             win32.PAGE_READONLY, ctypes.byref(old))
        for section in pe.sections:
            length = _align(max(section.Misc_VirtualSize, section.SizeOfRawData))
            if length:
                win32.VirtualProtect(self.base + section.VirtualAddress, length,
                                     _page_protection(section.Characteristics), ctypes.byref(old))
        win32.FlushInstructionCache(win32.GetCurrentProcess(), self.base, self.size)

    def _register_exceptions(self, pe):
        """Déclare .pdata à Windows : sans elle, exceptions C++ et SEH ne trouvent pas leurs gestionnaires."""
        directory = pe.OPTIONAL_HEADER.DATA_DIRECTORY[DIR_EXCEPTION]
        count = directory.Size // RUNTIME_FUNCTION_SIZE
        if directory.VirtualAddress and count:
            table = self.base + directory.VirtualAddress
            if win32.RtlAddFunctionTable(table, count, self.base):
                self._function_table = table

    # --- Exécution ----------------------------------------------------------

    def call_entry(self, *arguments):
        """Appelle le point d'entrée (DllMain(base, 1, 0) pour une DLL)."""
        if self.is_dll and not arguments:
            arguments = (self.base, 1, 0)
        prototype = ctypes.CFUNCTYPE(ctypes.c_int64, *([ctypes.c_uint64] * len(arguments)))
        return prototype(self.entry)(*arguments)

    def _install_tls(self, pe):
        """TLS implicite (thread_local) : confiée au runtime C++, qui équipe chaque thread."""
        directory = pe.OPTIONAL_HEADER.DATA_DIRECTORY[DIR_TLS]
        if not directory.VirtualAddress:
            return
        install = ctypes.WINFUNCTYPE(ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p)(
            self.shims.runtime.export("AnyXboxX_InstallImageTls", "anyxboxx_tls.dll"))
        if not install(self.base, self.base + directory.VirtualAddress):
            raise ValueError("Exécution native : installation de la TLS implicite impossible.")
        self._tls_installed = True

    def close(self):
        if getattr(self, "_tls_installed", False):
            remove = ctypes.WINFUNCTYPE(None, ctypes.c_void_p)(self.shims.runtime.export("AnyXboxX_RemoveImageTls", "anyxboxx_tls.dll"))
            remove(self.base)
            self._tls_installed = False
        if self._function_table:
            win32.RtlDeleteFunctionTable(self._function_table)
            self._function_table = None
        if self.base:
            win32.VirtualFree(self.base, 0, win32.MEM_RELEASE)
            self.base = None
