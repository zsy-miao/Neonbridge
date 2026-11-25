"""Chargement de plusieurs modules PE liés dans une même machine.

Règle de résolution d'un import `dll!fonction` :
1. si `dll` est une bibliothèque système (kernel32, api-ms-win-*, xgameruntime…),
   la fonction vient toujours de la couche HLE ;
2. sinon, si un fichier portant ce nom existe dans le dossier du paquet, il est
   chargé comme module et l'import pointe vers son export (redirections suivies) ;
3. sinon, ou si l'export manque, l'import pointe vers la page HLE.
"""
from dataclasses import dataclass, field
from pathlib import Path
import struct

from .loader import read_pe
from .memory import align

SYSTEM_PREFIXES = ("api-ms-win-", "ext-ms-")
SYSTEM_DLLS = frozenset({
    "kernel32.dll", "kernelbase.dll", "kernelx.dll", "ntdll.dll", "ucrtbase.dll",
    "msvcrt.dll", "xgameruntime.dll", "user32.dll", "advapi32.dll", "ole32.dll",
})
MODULE_AREA = 0x6000_0000_0000
MODULE_AREA_END = 0x6800_0000_0000
MODULE_ALIGN = 0x10000
MAX_MODULES = 64
MAX_FORWARDS = 8


def dll_name(name):
    """Nom normalisé : minuscules, extension .dll ajoutée si absente."""
    name = name.lower()
    return name if name.endswith((".dll", ".exe")) else name + ".dll"


def is_system(name):
    name = dll_name(name)
    return name in SYSTEM_DLLS or name.startswith(SYSTEM_PREFIXES)


@dataclass
class Module:
    name: str
    path: Path
    image: object
    deps: list = field(default_factory=list)   # rempli pendant la liaison

    def contains(self, address):
        return self.image.base <= address < self.image.base + self.image.size


class ModuleSet:
    def __init__(self, cpu, search_dirs, thunk_for, protect):
        self.cpu = cpu
        self.thunk_for = thunk_for    # (dll, fonction) → adresse dans la page HLE
        self.protect = protect        # (image, protections) → None
        self.modules = {}             # nom normalisé → Module, dans l'ordre de chargement
        self.sources = {}             # « dll!fonction » → nom du module qui la fournit
        self._next_base = MODULE_AREA
        self._files = {}
        for folder in search_dirs:
            folder = Path(folder)
            if folder.is_dir():
                for candidate in folder.iterdir():
                    if candidate.is_file():
                        self._files.setdefault(candidate.name.lower(), candidate)

    # --- Recherche ------------------------------------------------------------

    def find_file(self, name):
        return None if is_system(name) else self._files.get(dll_name(name))

    def get(self, name):
        return self.modules.get(dll_name(name))

    def module_at(self, address):
        return next((m for m in self.modules.values() if m.contains(address)), None)

    def by_base(self, base):
        return next((m for m in self.modules.values() if m.image.base == base), None)

    # --- Chargement -----------------------------------------------------------

    def load(self, path):
        key = path.name.lower()
        if key in self.modules:
            return self.modules[key]
        if len(self.modules) >= MAX_MODULES:
            raise ValueError("Trop de modules chargés dans la machine.")
        image, protections = read_pe(path, self.cpu, allocate=self._allocate)
        module = Module(key, path, image)
        self.modules[key] = module   # enregistré avant la liaison : gère les imports croisés
        for item in image.imports:
            address = self.resolve(item.dll, item.name, module)
            self.cpu.mem_write(item.slot, struct.pack("<Q", address))
        self.protect(image, protections)
        return module

    def _overlap(self, base, size):
        return next((m for m in self.modules.values()
                     if base < m.image.base + m.image.size and m.image.base < base + size), None)

    def _allocate(self, preferred, size, relocatable):
        if preferred is not None and self._overlap(preferred, size) is None:
            return preferred
        if not relocatable:
            raise ValueError("Base d’image déjà occupée et aucune table de relocation.")
        base = self._next_base
        while (conflict := self._overlap(base, size)) is not None:
            base = align(conflict.image.base + conflict.image.size, MODULE_ALIGN)
        if base + size > MODULE_AREA_END:
            raise ValueError("Plus de place pour charger un module.")
        self._next_base = align(base + size, MODULE_ALIGN)
        return base

    # --- Liaison --------------------------------------------------------------

    def resolve(self, dll, name, importer=None, depth=0):
        path = self.find_file(dll)
        if path is not None and depth <= MAX_FORWARDS:
            target = self.load(path)
            if importer is not None and target is not importer and target.name not in importer.deps:
                importer.deps.append(target.name)
            address = self.export(target, name, importer, depth)
            if address:
                self.sources[f"{dll}!{name}"] = target.name
                return address
        return self.thunk_for(dll, name)

    def export(self, module, name, importer=None, depth=0):
        """Adresse d'un export (nom ou « #ordinal »), en suivant les redirections."""
        image = module.image
        forward = image.forwarders.get(name)
        if forward:
            dll, _, function = forward.partition(".")
            return self.resolve(dll_name(dll), function, importer, depth + 1)
        if name.startswith("#"):
            return image.ordinals.get(int(name[1:]))
        return image.exports.get(name)

    def init_order(self, root):
        """Dépendances d'abord (parcours en profondeur), `root` en dernier."""
        order, seen = [], set()

        def visit(module):
            if module.name in seen:
                return
            seen.add(module.name)
            for dep in module.deps:
                visit(self.modules[dep])
            order.append(module)

        visit(root)
        return order
