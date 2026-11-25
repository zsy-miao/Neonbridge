"""Fabrique de petits PE32+ x86-64 pour les tests : aucun compilateur nécessaire.

Le module produit une seule section RWX à l'adresse relative 0x1000 :
code (0x400 octets), table d'imports, table d'exports puis relocations.
"""
import struct

SECTION_RVA = 0x1000
FILE_ALIGN = 0x200
HEADERS_SIZE = 0x400
CODE_SIZE = 0x400
DLL_BASE = 0x1_8000_0000
EXE_BASE = 0x1_4000_0000
ORDINAL_FLAG = 0x8000_0000_0000_0000
REL_BASED_DIR64 = 10


def _align(value, unit):
    return (value + unit - 1) // unit * unit


def rva(offset):
    """Adresse relative d'un octet du code (décalage depuis le début de la section)."""
    return SECTION_RVA + offset


class Section:
    def __init__(self):
        self.body = bytearray(CODE_SIZE)

    def put(self, data, alignment=8):
        start = _align(len(self.body), alignment)
        self.body += bytes(start - len(self.body)) + data
        return start


def _imports(section, imports):
    groups = {}
    for dll, function in imports:
        groups.setdefault(dll, []).append(function)
    if not groups:
        return (0, 0), {}
    table = section.put(bytes(20 * (len(groups) + 1)), 4)
    iat = {}
    for index, (dll, functions) in enumerate(groups.items()):
        lookup = section.put(bytes(8 * (len(functions) + 1)))
        address = section.put(bytes(8 * (len(functions) + 1)))
        for slot, function in enumerate(functions):
            if isinstance(function, int):
                entry = ORDINAL_FLAG | function
            else:
                entry = rva(section.put(b"\0\0" + function.encode() + b"\0", 2))
            struct.pack_into("<Q", section.body, lookup + 8 * slot, entry)
            struct.pack_into("<Q", section.body, address + 8 * slot, entry)
            iat[(dll, function)] = rva(address + 8 * slot)
        name = section.put(dll.encode() + b"\0", 2)
        struct.pack_into("<IIIII", section.body, table + 20 * index,
                         rva(lookup), 0, 0, rva(name), rva(address))
    return (rva(table), 20 * (len(groups) + 1)), iat


def _exports(section, exports, name):
    if not exports:
        return (0, 0)
    start = section.put(bytes(40), 4)
    count = len(exports)
    functions = section.put(bytes(4 * count), 4)
    names = section.put(bytes(4 * count), 4)
    ordinals = section.put(bytes(2 * count), 2)
    dll_name = section.put(name.encode() + b"\0", 1)
    for slot, (export, index) in enumerate(sorted((e[0], i) for i, e in enumerate(exports))):
        struct.pack_into("<I", section.body, names + 4 * slot,
                         rva(section.put(export.encode() + b"\0", 1)))
        struct.pack_into("<H", section.body, ordinals + 2 * slot, index)
    for index, (_, target) in enumerate(exports):
        if isinstance(target, str):
            value = rva(section.put(target.encode() + b"\0", 1))
        else:
            value = rva(target)
        struct.pack_into("<I", section.body, functions + 4 * index, value)
    end = len(section.body)
    struct.pack_into("<IIHHIIIIIII", section.body, start, 0, 0, 0, 0, rva(dll_name), 1,
                     count, count, rva(functions), rva(names), rva(ordinals))
    return (rva(start), end - start)


def _relocations(section, pointers):
    if not pointers:
        return (0, 0)
    entries = [(REL_BASED_DIR64 << 12) | offset for offset in sorted(pointers)]
    if len(entries) % 2:
        entries.append(0)
    block = struct.pack("<II", SECTION_RVA, 8 + 2 * len(entries))
    block += struct.pack(f"<{len(entries)}H", *entries)
    return (rva(section.put(block, 4)), len(block))


def _headers(*, dll, base, entry, size_of_image, raw_size, directories, relocatable):
    characteristics = 0x2022 if dll else 0x0022
    dll_characteristics = 0x0160 if relocatable else 0x0100
    optional = struct.pack(
        "<HBBIIIIIQIIHHHHHHIIIIHHQQQQII", 0x20B, 14, 0, raw_size, 0, 0,
        rva(entry) if entry is not None else 0, SECTION_RVA, base, 0x1000, FILE_ALIGN,
        6, 0, 0, 0, 6, 0, 0, size_of_image, HEADERS_SIZE, 0, 3, dll_characteristics,
        0x100000, 0x1000, 0x100000, 0x1000, 0, 16)
    optional += b"".join(struct.pack("<II", *d) for d in directories)
    coff = struct.pack("<HHIIIHH", 0x8664, 1, 0, 0, 0, len(optional), characteristics)
    section = struct.pack("<8sIIIIIIHHI", b".text", raw_size, SECTION_RVA, raw_size,
                          HEADERS_SIZE, 0, 0, 0, 0, 0xE0000020)
    header = bytearray(HEADERS_SIZE)
    header[0:2] = b"MZ"
    struct.pack_into("<I", header, 0x3C, 0x80)
    pe = b"PE\0\0" + coff + optional + section
    header[0x80:0x80 + len(pe)] = pe
    return bytes(header)


def build_pe(path, code_fn, *, imports=(), exports=(), pointers=None, dll=True,
             base=None, entry=0, name=None, tls=None):
    """Écrit un PE dans `path`.

    code_fn(iat) -> octets du code ; iat[(dll, fonction)] = adresse relative de l'entrée IAT.
    exports : [(nom, décalage du code)] ou [(nom, "DLL.Fonction")] pour une redirection.
    pointers : {décalage: décalage cible} — qwords absolus corrigés par relocation.
    entry : décalage du point d'entrée, ou None pour une DLL sans DllMain.
    tls : décalage d'un IMAGE_TLS_DIRECTORY64 (40 octets) écrit dans le code ; ses
          adresses absolues se déclarent dans `pointers`.
    """
    pointers = pointers or {}
    base = base if base is not None else (DLL_BASE if dll else EXE_BASE)
    section = Section()
    import_dir, iat = _imports(section, imports)
    export_dir = _exports(section, exports, name or path.name)
    reloc_dir = _relocations(section, pointers)
    code = code_fn(iat)
    if len(code) > CODE_SIZE:
        raise ValueError("Code de test trop long.")
    section.body[:len(code)] = code
    for offset, target in pointers.items():
        struct.pack_into("<Q", section.body, offset, base + rva(target))
    raw_size = _align(len(section.body), FILE_ALIGN)
    section.body += bytes(raw_size - len(section.body))
    directories = [(0, 0)] * 16
    directories[0], directories[1], directories[5] = export_dir, import_dir, reloc_dir
    if tls is not None:
        directories[9] = (rva(tls), 40)
    header = _headers(dll=dll, base=base, entry=entry,
                      size_of_image=SECTION_RVA + _align(raw_size, 0x1000), raw_size=raw_size,
                      directories=directories, relocatable=bool(pointers))
    path.write_bytes(header + bytes(section.body))
    return path


class Asm:
    """Mini-assembleur x86-64 : chaque méthode ajoute une instruction."""

    def __init__(self):
        self.code = bytearray()

    @property
    def here(self):
        return len(self.code)

    def raw(self, data):
        self.code += data
        return self

    def at(self, offset):
        """Complète avec des zéros jusqu'à `offset` pour y placer la routine suivante."""
        if self.here > offset:
            raise ValueError("Chevauchement dans le code de test.")
        self.code += bytes(offset - self.here)
        return self

    def _disp(self, target_rva, length):
        return struct.pack("<i", target_rva - (rva(self.here) + length))

    def rip(self, opcode, offset, immediate=b""):
        """Instruction à opérande [rip+disp32] visant `offset` du code : opcode, disp32, puis l'immédiat."""
        return self.raw(opcode + self._disp(rva(offset), len(opcode) + 4 + len(immediate)) + immediate)

    def call_iat(self, iat_rva):
        return self.raw(b"\xff\x15" + self._disp(iat_rva, 6))

    def mov_ecx(self, value):
        return self.raw(b"\xb9" + struct.pack("<I", value))

    def mov_eax(self, value):
        return self.raw(b"\xb8" + struct.pack("<I", value))

    def mov_ecx_eax(self):
        return self.raw(b"\x89\xc1")

    def mov_rax_from(self, offset):
        return self.raw(b"\x48\x8b\x05" + self._disp(rva(offset), 7))

    def store_dword(self, offset, value):
        return self.raw(b"\xc7\x05" + self._disp(rva(offset), 10) + struct.pack("<I", value))

    def lea_rcx(self, offset):
        return self.raw(b"\x48\x8d\x0d" + self._disp(rva(offset), 7))

    def lea_rdx(self, offset):
        return self.raw(b"\x48\x8d\x15" + self._disp(rva(offset), 7))

    def zero_rcx(self):
        return self.raw(b"\x31\xc9")

    def zero_rdx(self):
        return self.raw(b"\x31\xd2")

    def mov_edx(self, value):
        return self.raw(b"\xba" + struct.pack("<I", value))

    def mov_r8d(self, value):
        return self.raw(b"\x41\xb8" + struct.pack("<I", value))

    def mov_r9d(self, value):
        return self.raw(b"\x41\xb9" + struct.pack("<I", value))

    def lea_r8(self, offset):
        return self.raw(b"\x4c\x8d\x05" + self._disp(rva(offset), 7))

    def lea_r9(self, offset):
        return self.raw(b"\x4c\x8d\x0d" + self._disp(rva(offset), 7))

    def stack_dword(self, disp8, value):
        """mov dword [rsp+disp8], valeur : le 5e argument d'un appel Windows x64."""
        return self.raw(b"\xc7\x44\x24" + bytes([disp8]) + struct.pack("<I", value))

    def nonzero_rax_to_eax(self):
        """eax = 1 si rax est non nul, sinon 0 : le code de sortie dit si un pointeur est rendu."""
        return self.raw(b"\x48\x85\xc0\x0f\x95\xc0\x0f\xb6\xc0")

    def zero_r8(self):
        return self.raw(b"\x45\x31\xc0")

    def mov_rcx_rax(self):
        return self.raw(b"\x48\x89\xc1")

    def call_rax(self):
        return self.raw(b"\xff\xd0")

    def frame(self):
        return self.raw(b"\x48\x83\xec\x28")

    def unframe(self):
        return self.raw(b"\x48\x83\xc4\x28")

    def ret(self):
        return self.raw(b"\xc3")

    def build(self):
        return bytes(self.code)
