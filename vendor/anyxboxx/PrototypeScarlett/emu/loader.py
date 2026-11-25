"""Chargement d'un exécutable PE32+ GDK dans la mémoire Unicorn."""
from dataclasses import dataclass, field
import struct

import pefile
from unicorn import UC_PROT_EXEC, UC_PROT_READ, UC_PROT_WRITE

from .memory import HEAP, PAGE, THUNKS, align

MAX_IMAGE = 256 * 1024 * 1024
DEFAULT_BASE = 0x1_4000_0000
MAX_TLS_CALLBACKS = 64
MAX_TLS_SIZE = 16 * 1024 * 1024
DIR_EXPORT, DIR_IMPORT, DIR_RELOC, DIR_TLS, DIR_DELAY, DIR_CLR = 0, 1, 5, 9, 13, 14
SCN_EXEC, SCN_READ, SCN_WRITE = 0x20000000, 0x40000000, 0x80000000
IMAGE_FILE_DLL = 0x2000


@dataclass(frozen=True)
class Import:
    dll: str
    name: str          # « #12 » pour un import par ordinal
    slot: int          # adresse de l'entrée IAT à remplir


@dataclass(frozen=True)
class Tls:
    template_start: int
    template_end: int
    zero_fill: int
    index_address: int
    callbacks: tuple


@dataclass(frozen=True)
class Image:
    base: int
    size: int
    entry: int
    imports: tuple
    exports: dict          # nom → adresse
    tls: Tls | None
    relocated: bool
    is_dll: bool = False
    ordinals: dict = field(default_factory=dict)    # ordinal → adresse
    forwarders: dict = field(default_factory=dict)  # nom ou « #n » → « DLL.Fonction »


def _check_headers(pe):
    header = pe.OPTIONAL_HEADER
    if pe.FILE_HEADER.Machine != 0x8664 or header.Magic != 0x20B:
        raise ValueError("Le programme doit être un exécutable x86-64 au format PE32+.")
    size = header.SizeOfImage
    entry_ok = (0 <= header.AddressOfEntryPoint < size if _is_dll(pe)
                else 0 < header.AddressOfEntryPoint < size)
    if not 0 < header.SizeOfHeaders <= size <= MAX_IMAGE or not entry_ok:
        raise ValueError("Image PE incompatible ou point d’entrée absent.")
    for index, feature in ((DIR_DELAY, "imports différés"), (DIR_CLR, ".NET")):
        if _directory(pe, index):
            raise ValueError(f"Fonctionnalité non émulée : {feature}.")


def _is_dll(pe):
    return bool(pe.FILE_HEADER.Characteristics & IMAGE_FILE_DLL)


def _directory(pe, index):
    dirs = pe.OPTIONAL_HEADER.DATA_DIRECTORY
    return index < len(dirs) and dirs[index].VirtualAddress != 0


def _has_relocations(pe):
    dirs = pe.OPTIONAL_HEADER.DATA_DIRECTORY
    return (len(dirs) > DIR_RELOC and dirs[DIR_RELOC].Size > 0
            and hasattr(pe, "DIRECTORY_ENTRY_BASERELOC"))


def default_allocate(preferred, size, relocatable):
    """Placement d'une image seule : base préférée, sinon DEFAULT_BASE."""
    if preferred is not None:
        return preferred
    if not relocatable:
        raise ValueError("Base d’image inutilisable et table de relocation absente ou illisible.")
    return DEFAULT_BASE


def _choose_base(pe, allocate):
    """Écarte une base préférée qui chevauche les zones de l'émulateur, puis délègue."""
    base = pe.OPTIONAL_HEADER.ImageBase
    size = align(pe.OPTIONAL_HEADER.SizeOfImage)
    usable = not (base % PAGE or base == 0 or base + size > THUNKS or base >= HEAP)
    return allocate(base if usable else None, size, _has_relocations(pe))


def _protection(flags):
    # Comme Windows : une section écrivable ou exécutable reste toujours lisible.
    readable = flags & (SCN_READ | SCN_WRITE | SCN_EXEC)
    return ((UC_PROT_READ if readable else 0)
            | (UC_PROT_WRITE if flags & SCN_WRITE else 0)
            | (UC_PROT_EXEC if flags & SCN_EXEC else 0))


def _map(cpu, pe, base, image_bytes):
    size = align(pe.OPTIONAL_HEADER.SizeOfImage)
    cpu.mem_map(base, size, UC_PROT_READ | UC_PROT_WRITE)
    cpu.mem_write(base, image_bytes[:size])
    protections = [UC_PROT_READ] * (size // PAGE)
    for section in pe.sections:
        length = align(max(section.Misc_VirtualSize, section.SizeOfRawData))
        first = section.VirtualAddress // PAGE
        if section.VirtualAddress % PAGE or section.VirtualAddress + length > size:
            raise ValueError("Section PE incompatible.")
        for page in range(first, first + length // PAGE):
            protections[page] = _protection(section.Characteristics)
    return protections


def _inside(address, base, size, length=8):
    return base <= address and address + length <= base + size


def _imports(pe, delta, base, size):
    """À appeler AVANT get_memory_mapped_image, qui relocalise les adresses de pefile."""
    if _directory(pe, DIR_IMPORT) and not hasattr(pe, "DIRECTORY_ENTRY_IMPORT"):
        raise ValueError("Table des imports PE illisible.")
    found = []
    for library in getattr(pe, "DIRECTORY_ENTRY_IMPORT", []):
        dll = library.dll.decode(errors="replace")
        for item in library.imports:
            name = item.name.decode(errors="replace") if item.name else f"#{item.ordinal}"
            slot = item.address + delta
            if not _inside(slot, base, size):
                raise ValueError(f"Entrée d’import {dll}!{name} hors de l’image.")
            found.append(Import(dll, name, slot))
    return tuple(found)


def _exports(pe, base):
    """Renvoie (exports par nom, exports par ordinal, redirections)."""
    directory = getattr(pe, "DIRECTORY_ENTRY_EXPORT", None)
    names, ordinals, forwarders = {}, {}, {}
    for symbol in directory.symbols if directory is not None else ():
        name = symbol.name.decode(errors="replace") if symbol.name else None
        if symbol.forwarder:
            target = symbol.forwarder.decode(errors="replace")
            forwarders[f"#{symbol.ordinal}"] = target
            if name:
                forwarders[name] = target
        elif symbol.address:
            ordinals[symbol.ordinal] = base + symbol.address
            if name:
                names[name] = base + symbol.address
    return names, ordinals, forwarders


def _tls_fields(pe, delta):
    """Lit le répertoire TLS avant relocation et ramène ses adresses à la base choisie."""
    entry = getattr(pe, "DIRECTORY_ENTRY_TLS", None)
    if entry is None:
        return None
    tls = entry.struct
    shift = lambda value: value + delta if value else 0  # noqa: E731
    return (shift(tls.StartAddressOfRawData), shift(tls.EndAddressOfRawData),
            tls.SizeOfZeroFill, shift(tls.AddressOfIndex), shift(tls.AddressOfCallBacks))


def _tls(fields, image, base):
    if fields is None:
        return None
    start, end, zero_fill, index, callbacks_at = fields
    size = len(image)
    if start or end:
        if not (_inside(start, base, size, 0) and start <= end and _inside(end, base, size, 0)):
            raise ValueError("Modèle TLS hors de l’image.")
    if (end - start) + zero_fill > MAX_TLS_SIZE:
        raise ValueError("Bloc TLS trop grand pour le prototype.")
    if index and not _inside(index, base, size, 4):
        raise ValueError("Index TLS hors de l’image.")
    callbacks = []
    cursor = callbacks_at
    while cursor and len(callbacks) < MAX_TLS_CALLBACKS:
        offset = cursor - base
        if not 0 <= offset <= len(image) - 8:
            raise ValueError("Table des callbacks TLS hors de l’image.")
        target = struct.unpack_from("<Q", image, offset)[0]
        if not target:
            break
        callbacks.append(target)
        cursor += 8
    return Tls(start, end, zero_fill, index, tuple(callbacks))


def read_pe(path, cpu=None, allocate=default_allocate):
    """Analyse le PE ; si `cpu` est fourni, mappe l'image et renvoie ses protections.

    `allocate(base_préférée_ou_None, taille, relocalisable)` choisit la base finale.
    """
    if path.stat().st_size > MAX_IMAGE:
        raise ValueError("Ce prototype accepte des exécutables de 256 Mio au maximum.")
    with pefile.PE(str(path), fast_load=True) as pe:
        _check_headers(pe)
        pe.parse_data_directories(directories=[DIR_EXPORT, DIR_IMPORT, DIR_RELOC, DIR_TLS])
        base = _choose_base(pe, allocate)
        size = align(pe.OPTIONAL_HEADER.SizeOfImage)
        delta = base - pe.OPTIONAL_HEADER.ImageBase
        imports = _imports(pe, delta, base, size)
        tls_fields = _tls_fields(pe, delta)
        # pefile applique les relocations (et modifie ses propres structures)
        # quand la base diffère : tout ce qui précède doit être lu avant.
        image_bytes = pe.get_memory_mapped_image(ImageBase=base)
        protections = _map(cpu, pe, base, image_bytes) if cpu is not None else None
        names, ordinals, forwarders = _exports(pe, base)
        entry_rva = pe.OPTIONAL_HEADER.AddressOfEntryPoint
        image = Image(
            base=base,
            size=size,
            entry=base + entry_rva if entry_rva else 0,
            imports=imports,
            exports=names,
            tls=_tls(tls_fields, image_bytes, base),
            relocated=delta != 0,
            is_dll=_is_dll(pe),
            ordinals=ordinals,
            forwarders=forwarders,
        )
    return image, protections
