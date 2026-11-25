"""Registre des fonctions émulées en haut niveau (HLE).

Comme WinDurango ou Speakeasy, l'émulateur n'exécute pas le système Xbox :
chaque import du programme pointe vers une entrée de la page HLE, et un
gestionnaire Python remplace la fonction système correspondante.
"""
import struct

from unicorn.x86_const import (
    UC_X86_REG_R8, UC_X86_REG_R9, UC_X86_REG_RCX, UC_X86_REG_RDX, UC_X86_REG_RSP,
    UC_X86_REG_XMM0, UC_X86_REG_XMM1, UC_X86_REG_XMM2, UC_X86_REG_XMM3,
)

S_OK = 0
E_NOTIMPL = 0x80004001
E_INVALIDARG = 0x80070057
E_OUTOFMEMORY = 0x8007000E
MASK64 = 0xFFFF_FFFF_FFFF_FFFF
MAX_STRING = 4096
ARG_REGISTERS = (UC_X86_REG_RCX, UC_X86_REG_RDX, UC_X86_REG_R8, UC_X86_REG_R9)
XMM_REGISTERS = (UC_X86_REG_XMM0, UC_X86_REG_XMM1, UC_X86_REG_XMM2, UC_X86_REG_XMM3)

REGISTRY = {}


def family(dll):
    """Regroupe les noms de DLL qui exposent les mêmes fonctions."""
    name = dll.lower().removesuffix(".dll")
    if name == "xgameruntime":
        return "gdk"
    if (name in ("kernel32", "kernelx", "kernelbase", "ntdll", "advapi32")
            or name.startswith(("api-ms-win-core-", "api-ms-win-eventing-"))):
        return "kernel"
    if name in ("ucrtbase", "msvcrt") or name.startswith(("vcruntime140", "api-ms-win-crt-")):
        return "crt"
    return name


def hle(group, *names):
    """Décorateur : enregistre un gestionnaire pour une ou plusieurs fonctions."""
    def register(handler):
        for name in names or (handler.__name__,):
            REGISTRY[(group, name)] = handler
        return handler
    return register


def lookup(dll, name):
    return REGISTRY.get((family(dll), name))


class Call:
    """Vue sur un appel en cours, selon la convention d'appel Windows x64."""

    def __init__(self, machine):
        self.machine = machine
        self.cpu = machine.cpu
        self.heap = machine.heap

    def arg(self, index):
        if index < len(ARG_REGISTERS):
            return self.cpu.reg_read(ARG_REGISTERS[index])
        # [rsp] = adresse de retour, puis 32 octets d'espace réservé.
        rsp = self.cpu.reg_read(UC_X86_REG_RSP)
        return self.read_u64(rsp + 8 + 32 + 8 * (index - len(ARG_REGISTERS)))

    def arg_double(self, index):
        """Argument flottant : XMM0 à XMM3 selon sa position."""
        raw = self.cpu.reg_read(XMM_REGISTERS[index]) & MASK64
        return struct.unpack("<d", struct.pack("<Q", raw))[0]

    def return_double(self, value):
        """Place un double dans XMM0 ; RAX garde la valeur renvoyée par le gestionnaire."""
        self.cpu.reg_write(UC_X86_REG_XMM0, struct.unpack("<Q", struct.pack("<d", value))[0])

    def read(self, address, size):
        return bytes(self.cpu.mem_read(address, size))

    def write(self, address, data):
        self.cpu.mem_write(address, bytes(data))

    def read_u64(self, address):
        return struct.unpack("<Q", self.read(address, 8))[0]

    def write_u32(self, address, value):
        self.write(address, struct.pack("<I", value & 0xFFFF_FFFF))

    def write_u64(self, address, value):
        self.write(address, struct.pack("<Q", value & MASK64))

    def cstr(self, address, limit=MAX_STRING):
        out = bytearray()
        while address and len(out) < limit:
            byte = self.read(address + len(out), 1)
            if byte == b"\0":
                break
            out += byte
        return out.decode("utf-8", errors="replace")

    def wstr(self, address, limit=MAX_STRING):
        out = bytearray()
        while address and len(out) < 2 * limit:
            pair = self.read(address + len(out), 2)
            if pair == b"\0\0":
                break
            out += pair
        return out.decode("utf-16-le", errors="replace")

    def write_text(self, buffer, capacity, text, used_ptr=0):
        """Copie une chaîne terminée par zéro, au format des API XSystemGet*."""
        data = text.encode() + b"\0"
        if used_ptr:
            self.write_u64(used_ptr, len(data))
        if not buffer or capacity < len(data):
            return E_INVALIDARG
        self.write(buffer, data)
        return S_OK
