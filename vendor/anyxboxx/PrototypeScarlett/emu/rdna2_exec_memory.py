"""Instructions mémoire RDNA 2 pour l'interpréteur de référence : SMEM, DS (LDS), FLAT/GLOBAL et MUBUF.

La mémoire globale est un espace d'adresses 64 bits éparse ; la LDS est un tableau de 64 Kio
propre au wave. Les buffers MUBUF et s_buffer_load passent par un descripteur V# de quatre
SGPR (adresse de base 48 bits, pas, nombre d'éléments) ; un accès hors limites lit 0 et
n'écrit rien, comme sur le GPU. Formats d'encodage et sémantique d'après la documentation
publique d'AMD (« RDNA 2 Instruction Set Architecture »).
"""
import re
import struct

from emu import rdna2_exec_atomic, rdna2_exec_lanes, rdna2_exec_smem
from emu.rdna2_exec_ops import MASK32, signed

PAGE = 4096
LDS_SIZE = 64 * 1024
NULL, OFF = 125, 0x7D
DWORD = 4
WIDTHS = {"dword": 4, "dwordx2": 8, "dwordx3": 12, "dwordx4": 16, "dwordx8": 32, "dwordx16": 64,
          "b32": 4, "b64": 8, "b96": 12, "b128": 16, "ubyte": 1, "sbyte": 1, "byte": 1, "u8": 1, "i8": 1,
          "b8": 1, "ushort": 2, "sshort": 2, "short": 2, "u16": 2, "i16": 2, "b16": 2}
SIGNED_TYPES = ("sbyte", "sshort", "i8", "i16")
ATOMICS = {
    "add": lambda old, value: old + value, "sub": lambda old, value: old - value,
    "swap": lambda old, value: value, "and": lambda old, value: old & value,
    "or": lambda old, value: old | value, "xor": lambda old, value: old ^ value,
    "umin": min, "umax": max, "min_u32": min, "max_u32": max,
    "smin": lambda old, value: min(signed(old), signed(value)),
    "smax": lambda old, value: max(signed(old), signed(value)),
    "min_i32": lambda old, value: min(signed(old), signed(value)),
    "max_i32": lambda old, value: max(signed(old), signed(value)),
}
LDS_SIMPLE_ATOMICS = ("add", "sub", "and", "or", "xor")
VECTOR_PATTERN = re.compile(r"^(global|flat|scratch|buffer)_(load|store)_([a-z0-9]+)$")
VECTOR_ATOMIC = re.compile(r"^(global|flat|buffer)_atomic_(add|sub|swap|and|or|xor|umin|umax|smin|smax)$")
SCALAR_PATTERN = re.compile(r"^s_(buffer_)?load_(dword|dwordx2|dwordx4|dwordx8|dwordx16)$")
LDS_PATTERN = re.compile(r"^ds_(read|write)(2st64|2)?_(b32|b64|b96|b128|u8|i8|u16|i16|b8|b16)$")
LDS_ATOMIC = re.compile(r"^ds_(add|sub|and|or|xor|min|max)(_rtn)?_(u32|i32|b32)$")
# Formes _d16 : la valeur est une moitié (16 bits) du VGPR. {nom: (sens, type, moitié haute)}.
LDS_HALVES = {
    **{f"ds_read_{kind}_d16{suffix}": ("read", kind, bool(suffix))
       for kind in ("u8", "i8", "u16") for suffix in ("", "_hi")},
    **{f"ds_write_{kind}_d16_hi": ("write", kind, True) for kind in ("b8", "b16")},
}
HALF_BITS, HALF_MASK = 16, 0xFFFF
# No VGPR address: LDS[LDS_BASE + {OFFSET1,OFFSET0} + M0[15:0] + TID*4] (ISA section 12.13, DS opcodes 176-177,
# p. 204). {name: direction}
LDS_ADD_TID = {"ds_write_addtid_b32": "write", "ds_read_addtid_b32": "read"}
M0 = 124
CACHE_CONTROL = frozenset({"s_dcache_inv", "s_gl1_inv", "s_dcache_wb", "buffer_gl0_inv", "buffer_gl1_inv",
                           "ds_nop"})
OTHER = frozenset({"s_memtime", "s_memrealtime", *rdna2_exec_lanes.NAMES})
# No VGPR address: memory_Addr = sgpr_addr(64) + inst_offset(12) + tid*4 (ISA section 12.18.3, opcodes 22-23,
# p. 230). The ISA gives no address without SADDR: SADDR = off is refused. {name: plain access}
ADD_TID = {"global_load_dword_addtid": "global_load_dword", "global_store_dword_addtid": "global_store_dword"}


class Memory:
    """Mémoire globale éparse (pages de 4 Kio créées à la première écriture)."""

    def __init__(self):
        self.pages = {}

    def read(self, address, size):
        out = bytearray()
        while size:
            page, offset = divmod(address, PAGE)
            count = min(size, PAGE - offset)
            out += self.pages.get(page, bytes(PAGE))[offset:offset + count]
            address, size = address + count, size - count
        return bytes(out)

    def write(self, address, data):
        while data:
            page, offset = divmod(address, PAGE)
            count = min(len(data), PAGE - offset)
            self.pages.setdefault(page, bytearray(PAGE))[offset:offset + count] = data[:count]
            address, data = address + count, data[count:]

    def write_words(self, address, words):
        self.write(address, struct.pack(f"<{len(words)}I", *(w & MASK32 for w in words)))

    def read_words(self, address, count):
        return list(struct.unpack(f"<{count}I", self.read(address, count * DWORD)))


def supports(name):
    """Vrai si l'interpréteur exécute cette instruction mémoire."""
    vector = VECTOR_PATTERN.match(name)
    return bool(name in CACHE_CONTROL or name in OTHER or name in ADD_TID or vector and vector.group(3) in WIDTHS
                or VECTOR_ATOMIC.match(name) or SCALAR_PATTERN.match(name)
                or LDS_PATTERN.match(name) or LDS_ATOMIC.match(name) or name in LDS_HALVES or name in LDS_ADD_TID
                or rdna2_exec_atomic.vector(name) or rdna2_exec_atomic.lds(name)
                or rdna2_exec_atomic.lds_exchange2(name) or rdna2_exec_smem.supports(name))


def _to_words(data):
    padded = data + bytes(-len(data) % DWORD)
    return list(struct.unpack(f"<{len(padded) // DWORD}I", padded))


def _extend(data, kind):
    value = int.from_bytes(data, "little")
    return signed(value, len(data) * 8) & MASK32 if kind in SIGNED_TYPES else value


def _store_bytes(wave, register, lane, size):
    words = [wave.v[register + i][lane] for i in range((size + DWORD - 1) // DWORD)]
    return struct.pack(f"<{len(words)}I", *words)[:size]


def _load_into(wave, register, lane, data, kind):
    if len(data) < DWORD:
        wave.v[register][lane] = _extend(data, kind)
        return
    for index, word in enumerate(_to_words(data)):
        wave.v[register + index][lane] = word


def execute(wave, family, name, word):
    """Exécute une instruction mémoire dont `supports(name)` est vrai ; `word` : 64 bits."""
    if name in CACHE_CONTROL:
        return
    if family == "SMEM" and rdna2_exec_smem.supports(name):
        rdna2_exec_smem.execute(wave, name, word)
    elif family == "SMEM":
        _scalar(wave, name, word)
    elif family == "DS":
        _lds(wave, name, word)
    elif family == "MUBUF":
        _buffer(wave, name, word)
    else:
        _flat(wave, family, name, word)


# --- SMEM : chargements scalaires --------------------------------------------------------

def descriptor(wave, first):
    """Descripteur de buffer V# : (adresse de base 48 bits, pas en octets, nombre d'éléments)."""
    base = wave.s[first] | (wave.s[first + 1] & 0xFFFF) << 32
    stride = (wave.s[first + 1] >> 16) & 0x3FFF
    return base, stride, wave.s[first + 2]


def _scalar(wave, name, word):
    sdata = (word >> 6) & 0x7F
    if name in ("s_memtime", "s_memrealtime"):
        wave.write_scalar(sdata, wave.steps, 2)
        return
    sbase = (word & 0x3F) * 2
    offset = signed((word >> 32) & 0x1FFFFF, 21)
    soffset = (word >> 57) & 0x7F
    if soffset != NULL:
        offset += wave.s[soffset]
    found = SCALAR_PATTERN.match(name)
    size = WIDTHS[found.group(2)]
    if found.group(1):
        base, _, records = descriptor(wave, sbase)
        inside = 0 <= offset and offset + size <= records
        data = wave.memory.read(base + offset, size) if inside else bytes(size)
    else:
        data = wave.memory.read((wave.s[sbase] | wave.s[sbase + 1] << 32) + offset, size)
    for index, value in enumerate(_to_words(data)):
        wave.s[sdata + index] = value


# --- DS : mémoire partagée LDS ------------------------------------------------------------

def _lds(wave, name, word):
    offset0, offset1 = word & 0xFF, (word >> 8) & 0xFF
    addr, data0, data1, vdst = ((word >> shift) & 0xFF for shift in (32, 40, 48, 56))
    if name in rdna2_exec_lanes.NAMES:
        rdna2_exec_lanes.execute(wave, name, addr, data0, vdst, offset1 << 8 | offset0)
        return
    if name in LDS_ADD_TID:
        return _lds_add_tid(wave, name, offset1 << 8 | offset0, data0, vdst)
    atomic = LDS_ATOMIC.match(name)
    found = LDS_PATTERN.match(name)
    extended, exchange2 = rdna2_exec_atomic.lds(name), rdna2_exec_atomic.lds_exchange2(name)
    for lane in wave.active():
        base = wave.v[addr][lane]
        if name in LDS_HALVES:
            _lds_half(wave, name, base + (offset1 << 8 | offset0), data0, vdst, lane)
            continue
        if atomic:
            _lds_atomic(wave, atomic, base + (offset1 << 8 | offset0), data0, vdst, lane)
            continue
        if extended:
            _lds_atomic_ext(wave, extended, base + (offset1 << 8 | offset0), (data0, data1), vdst, lane)
            continue
        if exchange2:
            _lds_exchange2(wave, exchange2, base, (offset0, offset1), (data0, data1), vdst, lane)
            continue
        size = WIDTHS[found.group(3)]
        if found.group(2):          # read2/write2[st64] : deux accès, décalages en éléments (× 64 pour st64)
            step = size * (rdna2_exec_atomic.STRIDE64 if found.group(2) == "2st64" else 1)
            places = [(base + offset0 * step, data0, vdst), (base + offset1 * step, data1, vdst + size // DWORD)]
        else:
            places = [(base + (offset1 << 8 | offset0), data0, vdst)]
        for address, source, target in places:
            address %= LDS_SIZE
            if found.group(1) == "write":
                wave.lds[address:address + size] = _store_bytes(wave, source, lane, size)
            else:
                _load_into(wave, target, lane, bytes(wave.lds[address:address + size]), found.group(3))


def _lds_add_tid(wave, name, offset, data0, vdst):
    base = (wave.s[M0] & HALF_MASK) + offset
    for lane in wave.active():
        address = (base + lane * DWORD) % LDS_SIZE
        if LDS_ADD_TID[name] == "write":
            wave.lds[address:address + DWORD] = _store_bytes(wave, data0, lane, DWORD)
        else:
            _load_into(wave, vdst, lane, bytes(wave.lds[address:address + DWORD]), "b32")


def _lds_half(wave, name, address, data0, vdst, lane):
    """Accès _d16 : une lecture remplit une moitié du VGPR (étendue à 16 bits) et garde l'autre ;
    une écriture _d16_hi prend l'octet ou le demi-mot dans la moitié haute de la donnée.

    Source : DSInstructions.td de LLVM (sortie liée à l'entrée, prédicat D16PreservesUnusedBits).
    """
    direction, kind, high = LDS_HALVES[name]
    size, shift = WIDTHS[kind], HALF_BITS if high else 0
    address %= LDS_SIZE
    if direction == "write":
        value = (wave.v[data0][lane] >> shift) & ((1 << 8 * size) - 1)
        wave.lds[address:address + size] = value.to_bytes(size, "little")
        return
    half = _extend(bytes(wave.lds[address:address + size]), kind) & HALF_MASK
    kept = wave.v[vdst][lane] & ~(HALF_MASK << shift) & MASK32
    wave.v[vdst][lane] = kept | half << shift


def _lds_atomic(wave, found, address, data0, vdst, lane):
    address %= LDS_SIZE
    old = int.from_bytes(wave.lds[address:address + DWORD], "little")
    operation = found.group(1)
    key = operation if operation in LDS_SIMPLE_ATOMICS else f"{operation}_{found.group(3)}"
    new = ATOMICS[key](old, wave.v[data0][lane]) & MASK32
    wave.lds[address:address + DWORD] = new.to_bytes(DWORD, "little")
    if found.group(2):
        wave.v[vdst][lane] = old


def _lane_value(wave, register, lane, width):
    """Valeur de `width` bits dans les VGPR register, register + 1… d'une ligne."""
    return sum(wave.v[register + i][lane] << (32 * i) for i in range(width // 32))


def _set_lane_value(wave, register, lane, value, width):
    for i in range(width // 32):
        wave.v[register + i][lane] = value >> (32 * i) & MASK32


def _lds_atomic_ext(wave, spec, address, sources, vdst, lane):
    width, operation, returns = spec
    size, address = width // 8, address % LDS_SIZE
    old = int.from_bytes(wave.lds[address:address + size], "little")
    a, b = (_lane_value(wave, register, lane, width) for register in sources)
    wave.lds[address:address + size] = operation(old, a, b).to_bytes(size, "little")
    if returns:
        _set_lane_value(wave, vdst, lane, old, width)


def _lds_exchange2(wave, spec, base, offsets, sources, vdst, lane):
    """ds_wrxchg2[st64]_rtn : deux échanges, anciennes valeurs dans vdst puis vdst + largeur."""
    width, stride = spec
    size = width // 8
    for index, (offset, source) in enumerate(zip(offsets, sources)):
        address = (base + offset * size * stride) % LDS_SIZE
        old = int.from_bytes(wave.lds[address:address + size], "little")
        wave.lds[address:address + size] = _lane_value(wave, source, lane, width).to_bytes(size, "little")
        _set_lane_value(wave, vdst + index * width // 32, lane, old, width)


# --- FLAT / GLOBAL et MUBUF : mémoire vectorielle ------------------------------------------

FLAT_OFFSET_SIGN = 0x800


def flat_offset(family, word):
    """Byte offset of a FLAT, GLOBAL or SCRATCH instruction, or None when it is not defined.

    ISA section 9.1 (p. 83): "Scratch, Global: 12-bit signed byte offset. Flat: must be positive."; llvm-mc 20
    (gfx1030) only accepts an 11-bit unsigned offset on FLAT and disassembles bit 11 as +2048. What the
    hardware does with bit 11 set on FLAT is not documented, so the callers refuse it by name.
    """
    if family == "FLAT":
        return None if word & FLAT_OFFSET_SIGN else word & (FLAT_OFFSET_SIGN - 1)
    return signed(word & 0xFFF, 12)


def _flat(wave, family, name, word):
    offset = flat_offset(family, word)
    if offset is None:
        from emu.rdna2_exec import Untranslated
        raise Untranslated(f"{name}: FLAT offset with bit 11 set (the ISA requires a positive offset)")
    addr, data, saddr, vdst = (word >> 32) & 0xFF, (word >> 40) & 0xFF, (word >> 48) & 0x7F, (word >> 56) & 0xFF
    if name in ADD_TID:
        return _add_tid(wave, name, offset, data, saddr, vdst)
    for lane in wave.active():
        if family == "GLOBAL" and saddr != OFF:
            address = (wave.s[saddr] | wave.s[saddr + 1] << 32) + wave.v[addr][lane]
        else:
            address = wave.v[addr][lane] | wave.v[addr + 1][lane] << 32
        _access(wave, name, address + offset, data, vdst, lane, returns=(word >> 16) & 1)


def _add_tid(wave, name, offset, data, saddr, vdst):
    if saddr == OFF:
        from emu.rdna2_exec import Untranslated
        raise Untranslated(f"{name} with SADDR off: address not given by the RDNA 2 ISA")
    base = wave.s[saddr] | wave.s[saddr + 1] << 32
    for lane in wave.active():
        _access(wave, ADD_TID[name], base + offset + lane * DWORD, data, vdst, lane)


def _buffer(wave, name, word):
    offset, offen, idxen = word & 0xFFF, (word >> 12) & 1, (word >> 13) & 1
    vaddr, vdata = (word >> 32) & 0xFF, (word >> 40) & 0xFF
    base, stride, records = descriptor(wave, ((word >> 48) & 0x1F) * 4)
    soffset_code = (word >> 56) & 0xFF
    soffset = 0 if soffset_code == NULL else wave.read_scalar(soffset_code, None)
    plain, extended = VECTOR_PATTERN.match(name), rdna2_exec_atomic.vector(name)
    size = WIDTHS[plain.group(3)] if plain else extended[0] // 8 if extended else DWORD
    for lane in wave.active():
        index = wave.v[vaddr][lane] if idxen else 0
        byte_offset = offset + (wave.v[vaddr + idxen][lane] if offen else 0)
        inside = index < records if stride else byte_offset + size <= records
        _access(wave, name, base + soffset + index * stride + byte_offset, vdata, vdata, lane, inside,
                returns=(word >> 14) & 1)


def _access(wave, name, address, data, vdst, lane, inside=True, returns=False):
    extended = rdna2_exec_atomic.vector(name)
    if extended:
        if inside:
            _vector_atomic_ext(wave, extended, address, data, vdst, lane, returns)
        return
    atomic = VECTOR_ATOMIC.match(name)
    if atomic:
        if inside:
            old = wave.memory.read_words(address, 1)[0]
            wave.memory.write_words(address, [ATOMICS[atomic.group(2)](old, wave.v[data][lane])])
            if returns:                             # glc : la valeur d'avant est renvoyée
                wave.v[vdst][lane] = old
        return
    found = VECTOR_PATTERN.match(name)
    size = WIDTHS[found.group(3)]
    if found.group(2) == "store":
        if inside:
            wave.memory.write(address, _store_bytes(wave, data, lane, size))
    else:
        loaded = wave.memory.read(address, size) if inside else bytes(size)
        _load_into(wave, vdst, lane, loaded, found.group(3))


def _vector_atomic_ext(wave, spec, address, data, vdst, lane, returns):
    """cmpswap, inc/dec, csub, flottants et formes _x2 : DATA[0] puis DATA[1] (cmpswap) en VGPR."""
    width, operation, words = spec
    size = width // 8
    old = int.from_bytes(wave.memory.read(address, size), "little")
    a = _lane_value(wave, data, lane, width)
    b = _lane_value(wave, data + width // 32, lane, width) if words > width // 32 else 0
    wave.memory.write(address, operation(old, a, b).to_bytes(size, "little"))
    if returns:                                   # glc : la valeur d'avant est renvoyée
        _set_lane_value(wave, vdst, lane, old, width)
