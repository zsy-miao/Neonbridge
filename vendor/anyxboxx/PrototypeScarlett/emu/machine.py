"""Machine émulée : processeur Unicorn, mémoire, TEB/PEB et répartition HLE."""
from pathlib import Path
import struct
import time

from unicorn import UC_ARCH_X86, UC_HOOK_CODE, UC_HOOK_MEM_INVALID, UC_MODE_64, Uc, UcError
from unicorn.x86_const import UC_X86_REG_GS_BASE, UC_X86_REG_RAX, UC_X86_REG_RIP, UC_X86_REG_RSP

from . import hle
from .hle.core import ARG_REGISTERS, MASK64
from .memory import PAGE, RW, RX, STACK, STACK_SIZE, SYSTEM, THUNK_PAGES, THUNKS, Heap
from .modules import ModuleSet

SLOT = 64                       # taille d'une entrée de la page HLE
TIMEOUT_US = 2_000_000
MAX_INSTRUCTIONS = 20_000_000
TEB, PEB = SYSTEM, SYSTEM + 0x2000
TLS_SLOTS = 64
FIRST_HANDLE = 0x1000
PROCESS_HEAP = 0xFEED_0000
DLL_PROCESS_ATTACH = 1
TLS_ALIGNMENT = 64  # couvre alignas(64) ; le PE peut demander moins


class Machine:
    def __init__(self, path, title_id=0, log=None, search_dirs=None):
        self.cpu = Uc(UC_ARCH_X86, UC_MODE_64)
        self.heap = Heap(self.cpu)
        self.title_id = title_id
        self.logs = []
        self._sink = log
        self.started = time.perf_counter()
        self.last_error = 0
        self.tls_slots = {}
        self.handles = {}
        self.runtime_ready = False
        self.process_heap = PROCESS_HEAP
        self.missing_calls = {}
        self.crt_streams = None
        self.c_locale = None
        self.ctype_table = None
        self.locale_names = None
        self.crt_cells = {}
        self.exception_filter = 0
        self._next_handle = FIRST_HANDLE
        self._pending = None
        self._fault = None
        self.exit_code = None

        self.system_modules = {}           # pseudo-handle → nom de DLL système
        self._map_thunks()
        path = Path(path)
        self.modules = ModuleSet(self.cpu, search_dirs or (path.parent,),
                                 self.thunk_for, self._protect_image)
        self.main = self.modules.load(path)
        self.image = self.main.image
        self.image_base = self.image.base
        self._build_system()
        self._init_tls()
        self.cpu.hook_add(UC_HOOK_CODE, self._on_thunk, begin=THUNKS,
                          end=THUNKS + THUNK_PAGES * PAGE - 1)
        self.cpu.hook_add(UC_HOOK_MEM_INVALID, self._on_fault)

    # --- Construction -------------------------------------------------------

    def _protect_image(self, image, protections):
        start = 0
        for index in range(1, len(protections) + 1):
            if index == len(protections) or protections[index] != protections[start]:
                self.cpu.mem_protect(image.base + start * PAGE,
                                     (index - start) * PAGE, protections[start])
                start = index

    def _map_thunks(self):
        """Page HLE en lecture-exécution ; les entrées sont écrites à la demande."""
        self.cpu.mem_map(THUNKS, THUNK_PAGES * PAGE, RX)
        self.stop = THUNKS                      # entrée 0 : retour vers l'émulateur
        self.cpu.mem_write(self.stop, b"\xc3")
        self.handlers = {}
        self.statuses = {}
        self._slots = {}

    def thunk_for(self, dll, name):
        """Adresse de l'entrée HLE de `dll!name`, créée au premier besoin."""
        key = (dll.lower(), name)
        address = self._slots.get(key)
        if address is not None:
            return address
        if len(self._slots) + 1 >= THUNK_PAGES * PAGE // SLOT:
            raise ValueError("Trop de fonctions importées pour la page HLE.")
        address = THUNKS + (len(self._slots) + 1) * SLOT
        kind, value = hle.resolve(dll, name)
        self.cpu.mem_write(address, value if kind == "code" else b"\xc3")
        if kind != "code":
            self.handlers[address] = (f"{dll}!{name}", value)
        self._slots[key] = address
        self.statuses[f"{dll}!{name}"] = kind
        return address

    def _build_system(self):
        self.cpu.mem_map(STACK, STACK_SIZE, RW)
        self.stack_top = STACK + STACK_SIZE - 0x100 - 8   # RSP ≡ 8 (mod 16) à l'entrée
        self.cpu.mem_map(SYSTEM, 0x3000, RW)
        self.tls_array = self.heap.alloc(8 * TLS_SLOTS)
        teb = {0x08: STACK + STACK_SIZE, 0x10: STACK, 0x30: TEB, 0x40: 0x4,
               0x48: 0x1000, 0x58: self.tls_array, 0x60: PEB}
        for offset, value in teb.items():
            self.cpu.mem_write(TEB + offset, struct.pack("<Q", value))
        self.cpu.mem_write(PEB + 0x10, struct.pack("<Q", self.image.base))
        self.cpu.mem_write(PEB + 0x30, struct.pack("<Q", self.process_heap))
        self.cpu.reg_write(UC_X86_REG_GS_BASE, TEB)

    def _init_tls(self):
        """Un bloc TLS par module qui en déclare, à l'index suivant du tableau."""
        index = 0
        for module in self.modules.modules.values():
            if module.image.tls is not None:
                if index >= TLS_SLOTS:
                    raise ValueError("Trop de modules avec stockage TLS.")
                self._init_module_tls(module.image.tls, index)
                index += 1

    def _init_module_tls(self, tls, index):
        template = b""
        if tls.template_end > tls.template_start:
            template = bytes(self.cpu.mem_read(tls.template_start,
                                               tls.template_end - tls.template_start))
        block = self.heap.alloc(len(template) + tls.zero_fill or 1, alignment=TLS_ALIGNMENT)
        if not block:
            raise ValueError("Mémoire insuffisante pour le bloc TLS.")
        if template:
            self.cpu.mem_write(block, template)
        self.cpu.mem_write(self.tls_array + 8 * index, struct.pack("<Q", block))
        if tls.index_address:
            self.cpu.mem_write(tls.index_address, struct.pack("<I", index))

    # --- Hooks --------------------------------------------------------------

    def _on_thunk(self, uc, address, size, user_data):
        entry = self.handlers.get(address)
        if entry is None:
            return
        name, handler = entry
        try:
            if handler is None:
                result = self._missing(name)
            else:
                result = handler(hle.Call(self))
            uc.reg_write(UC_X86_REG_RAX, (result or 0) & MASK64)
        except Exception as error:  # noqa: BLE001 — Unicorn relancerait l'exception brute
            self._pending = ValueError(f"{name} : {error}")
            uc.emu_stop()

    def _missing(self, name):
        count = self.missing_calls.get(name, 0)
        self.missing_calls = {**self.missing_calls, name: count + 1}
        if count == 0:
            self.log(f"Appel non émulé : {name} → E_NOTIMPL")
        return hle.E_NOTIMPL

    def _on_fault(self, uc, access, address, size, value, user_data):
        self._fault = (address, uc.reg_read(UC_X86_REG_RIP))
        return False

    # --- Services pour les gestionnaires HLE ---------------------------------

    def log(self, text):
        self.logs.append(text)
        if self._sink:
            self._sink(text)

    def new_handle(self, kind):
        handle = self._next_handle
        self._next_handle += 4
        self.handles[handle] = kind
        return handle

    def close_handle(self, handle):
        return self.handles.pop(handle, None) is not None

    def request_exit(self, code):
        self.exit_code = code
        self.cpu.emu_stop()

    # --- Exécution ----------------------------------------------------------

    def call(self, address, *arguments):
        if len(arguments) > len(ARG_REGISTERS):
            raise ValueError("Le prototype passe au plus quatre arguments.")
        if self.exit_code is not None:
            raise ValueError("Le programme émulé est déjà terminé.")
        self._pending = self._fault = None
        self.cpu.mem_write(self.stack_top, struct.pack("<Q", self.stop))
        self.cpu.reg_write(UC_X86_REG_RSP, self.stack_top)
        for register, value in zip(ARG_REGISTERS, arguments, strict=False):
            self.cpu.reg_write(register, value & MASK64)
        try:
            self.cpu.emu_start(address, self.stop, timeout=TIMEOUT_US, count=MAX_INSTRUCTIONS)
        except UcError as error:
            if self._fault is None and self._pending is None:
                raise ValueError(f"Erreur processeur : {error}") from error
        return self._result()

    def _result(self):
        if self._pending is not None:
            raise self._pending
        if self._fault is not None:
            target, rip = self._fault
            raise ValueError(f"Accès mémoire invalide à {target:#x} (instruction {rip:#x}).")
        if self.exit_code is not None:
            return self.exit_code
        if self.cpu.reg_read(UC_X86_REG_RIP) != self.stop:
            raise ValueError("Exécution interrompue : budget d’instructions ou de temps épuisé.")
        return self.cpu.reg_read(UC_X86_REG_RAX)

    def start(self):
        """Initialise les dépendances puis lance le module principal.

        Pour une DLL principale, renvoie le résultat de DllMain ; pour un
        exécutable, la valeur de retour de son point d'entrée.
        """
        result = 0
        for module in self.modules.init_order(self.main):
            result = self._initialise(module)
            if self.exit_code is not None:
                return self.exit_code
            if module is not self.main and module.image.is_dll and result & 0xFFFF_FFFF != 1:
                raise ValueError(f"{module.name} : DllMain a renvoyé FALSE, chargement annulé.")
        return result

    def _initialise(self, module):
        image = module.image
        for callback in image.tls.callbacks if image.tls else ():
            self.call(callback, image.base, DLL_PROCESS_ATTACH, 0)
            if self.exit_code is not None:
                return self.exit_code
        if not image.entry:
            return 1                       # DLL sans DllMain
        if image.is_dll:
            return self.call(image.entry, image.base, DLL_PROCESS_ATTACH, 0)
        return self.call(image.entry)
