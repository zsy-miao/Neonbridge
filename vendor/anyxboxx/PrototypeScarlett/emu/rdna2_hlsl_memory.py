"""Traduction RDNA 2 → HLSL des instructions mémoire (SMEM, DS, GLOBAL/FLAT, MUBUF).

La mémoire du programme est le buffer brut `g_memory` (u0) : seules les adresses 32 bits basses
sont utilisées. La LDS est `g_lds` (groupshared). Sur RDNA 2 un wave avance en bloc : une
écriture est visible de toutes ses lignes à l'instruction suivante. En HLSL, les 32 threads
ne sont pas synchrones, d'où une barrière de groupe après chaque écriture.
"""
from emu import rdna2_exec_lanes, rdna2_exec_smem, rdna2_hlsl_format, rdna2_hlsl_smem, rdna2_swizzle as swz
from emu.rdna2_exec_atomic import STRIDE64
from emu.rdna2_exec_memory import (ADD_TID, LDS_ADD_TID, LDS_ATOMIC, LDS_PATTERN, M0, SCALAR_PATTERN, VECTOR_ATOMIC,
                                   VECTOR_PATTERN, WIDTHS, flat_offset)
from emu.rdna2_exec_ops import signed
from emu.rdna2_hlsl_ops import NULL, OFF, Untranslatable, hexa, source

LDS_WORDS = "8128u"                  # taille de g_lds (limite D3D12 de 32 Kio)
STORE_BARRIER = "AllMemoryBarrierWithGroupSync();"
LDS_BARRIER = "GroupMemoryBarrierWithGroupSync();"
ATOMIC_FUNCTIONS = {"add": "InterlockedAdd", "and": "InterlockedAnd", "or": "InterlockedOr",
                    "xor": "InterlockedXor", "umin": "InterlockedMin", "umax": "InterlockedMax",
                    "swap": "InterlockedExchange", "min_u32": "InterlockedMin", "max_u32": "InterlockedMax"}
BYTE_LOADS = {"ubyte": ("0xFFu", None), "ushort": ("0xFFFFu", None), "sbyte": ("0xFFu", 8), "sshort": ("0xFFFFu", 16),
              "u8": ("0xFFu", None), "i8": ("0xFFu", 8), "u16": ("0xFFFFu", None), "i16": ("0xFFFFu", 16)}


def _load_lines(target, address, kind, buffer="g_memory.Load"):
    """Chargement de WIDTHS[kind] octets à `address` dans les VGPR à partir de `target`."""
    if kind in BYTE_LOADS:
        mask, sign_bits = BYTE_LOADS[kind]
        value = f"(({buffer}(({address}) & ~3u) >> ((({address}) & 3u) * 8u)) & {mask})"
        if sign_bits:
            value = f"asuint((asint({value}) << {32 - sign_bits}) >> {32 - sign_bits})"
        return [f"v[{target}] = {value};"]
    return [f"v[{target + i}] = {buffer}(({address}) + {4 * i}u);" for i in range(WIDTHS[kind] // 4)]


def _scalar(name, word):
    found = SCALAR_PATTERN.match(name)
    sdata, sbase = (word >> 6) & 0x7F, (word & 0x3F) * 2
    offset = hexa(signed((word >> 32) & 0x1FFFFF, 21))
    soffset = (word >> 57) & 0x7F
    extra = "" if soffset == NULL else f" + s[{soffset}]"
    count = WIDTHS[found.group(2)] // 4
    if found.group(1):              # s_buffer_load : base du V#, borne en s[base + 2]
        inside = f"(o + {4 * count}u <= s[{sbase + 2}])"
        loads = " ".join(f"s[{sdata + i}] = {inside} ? g_memory.Load(s[{sbase}] + o + {4 * i}u) : 0u;"
                         for i in range(count))
        return [f"{{ uint o = {offset}{extra}; {loads} }}"]
    loads = " ".join(f"s[{sdata + i}] = g_memory.Load(a + {4 * i}u);" for i in range(count))
    return [f"{{ uint a = s[{sbase}] + {offset}{extra}; {loads} }}"]


def _flat(family, name, word, active):
    offset = flat_offset(family, word)
    if offset is None:
        raise Untranslatable(f"{name}: FLAT offset with bit 11 set (the ISA requires a positive offset)")
    offset = hexa(offset)
    addr, data, saddr, vdst = (word >> 32) & 0xFF, (word >> 40) & 0xFF, (word >> 48) & 0x7F, (word >> 56) & 0xFF
    if name in ADD_TID:                 # SADDR + offset + lane × 4, no VGPR address (emu/rdna2_exec_memory.py)
        if saddr == OFF:
            raise Untranslatable(f"{name} with SADDR off: address not given by the RDNA 2 ISA")
        return _vector_access(ADD_TID[name], f"(s[{saddr}] + {offset} + lane * 4u)", data, vdst, active,
                              glc=0, inside="true")
    base = f"s[{saddr}] + v[{addr}]" if family == "GLOBAL" and saddr != OFF else f"v[{addr}]"
    address = f"({base} + {offset})"
    return _vector_access(name, address, data, vdst, active, glc=(word >> 16) & 1, inside="true")


def _buffer(name, word, active):
    offset, offen, idxen = word & 0xFFF, (word >> 12) & 1, (word >> 13) & 1
    if idxen:
        raise Untranslatable(f"non traduite en HLSL : {name} avec idxen")
    vaddr, vdata, resource = (word >> 32) & 0xFF, (word >> 40) & 0xFF, ((word >> 48) & 0x1F) * 4
    soffset = (word >> 56) & 0xFF
    extra = "" if soffset in (NULL, 128) else f" + {source(soffset)}"
    byte_offset = f"({offset}u{f' + v[{vaddr}]' if offen else ''})"
    found = VECTOR_PATTERN.match(name)
    size = WIDTHS[found.group(3)] if found else 4
    inside = f"({byte_offset} + {size}u <= s[{resource + 2}])"
    address = f"(s[{resource}]{extra} + {byte_offset})"
    return _vector_access(name, address, vdata, vdata, active, glc=(word >> 14) & 1, inside=inside)


def _vector_access(name, address, data, vdst, active, glc, inside):
    atomic = VECTOR_ATOMIC.match(name)
    if atomic:
        function = ATOMIC_FUNCTIONS.get(atomic.group(2))
        if function is None:
            raise Untranslatable(f"non traduite en HLSL : {name}")
        keep = f" v[{vdst}] = old;" if glc else ""
        return [f"if (({active}) && {inside}) {{ uint old; g_memory.{function}({address}, v[{data}], old);{keep} }}",
                STORE_BARRIER]
    found = VECTOR_PATTERN.match(name)
    kind = found.group(3)
    if found.group(2) == "store":
        if WIDTHS[kind] < 4:
            raise Untranslatable(f"non traduite en HLSL : {name} (écriture partielle d'un mot)")
        stores = " ".join(f"g_memory.Store({address} + {4 * i}u, v[{data + i}]);" for i in range(WIDTHS[kind] // 4))
        return [f"if (({active}) && {inside}) {{ {stores} }}", STORE_BARRIER]
    loads = " ".join(_load_lines(vdst, address, kind))
    zeros = " ".join(f"v[{vdst + i}] = 0u;" for i in range(max(1, WIDTHS[kind] // 4)))
    return [f"if ({active}) {{ if ({inside}) {{ {loads} }} else {{ {zeros} }} }}"]


def _lds(name, word, active):
    offset0, offset1 = word & 0xFF, (word >> 8) & 0xFF
    addr, data0, data1, vdst = ((word >> shift) & 0xFF for shift in (32, 40, 48, 56))
    combined = offset1 << 8 | offset0
    if name in rdna2_exec_lanes.NAMES:
        return _lanes(name, addr, data0, vdst, combined, active)
    if name in LDS_ADD_TID:             # M0[15:0] + offset + lane × 4, no VGPR address (emu/rdna2_exec_memory.py)
        index = f"(((s[{M0}] & 0xFFFFu) + {combined}u + lane * 4u) >> 2) % {LDS_WORDS}"
        if LDS_ADD_TID[name] == "write":
            return [f"if ({active}) {{ g_lds[{index}] = v[{data0}]; }}", LDS_BARRIER]
        return [LDS_BARRIER, f"if ({active}) {{ v[{vdst}] = g_lds[{index}]; }}"]
    atomic = LDS_ATOMIC.match(name)
    if atomic:
        operation = atomic.group(1)
        key = operation if operation in ("add", "and", "or", "xor") else f"{operation}_{atomic.group(3)}"
        function = ATOMIC_FUNCTIONS.get(key)
        if function is None:
            raise Untranslatable(f"non traduite en HLSL : {name}")
        keep = f" v[{vdst}] = old;" if atomic.group(2) else ""
        index = f"((v[{addr}] + {combined}u) >> 2) % {LDS_WORDS}"
        return [f"if ({active}) {{ uint old; {function}(g_lds[{index}], v[{data0}], old);{keep} }}", LDS_BARRIER]
    found = LDS_PATTERN.match(name)
    size = WIDTHS[found.group(3)]
    if size < 4:
        raise Untranslatable(f"non traduite en HLSL : {name} (LDS sous le mot)")
    words = size // 4
    if found.group(2):              # read2/write2[st64] : décalages en éléments (× 64 pour st64)
        step = size * (STRIDE64 if found.group(2) == "2st64" else 1)
        places = [(offset0 * step, data0, vdst), (offset1 * step, data1, vdst + words)]
    else:
        places = [(combined, data0, vdst)]
    lines = []
    for offset, source_register, target in places:
        for i in range(words):
            index = f"((v[{addr}] + {offset + 4 * i}u) >> 2) % {LDS_WORDS}"
            if found.group(1) == "write":
                lines.append(f"g_lds[{index}] = v[{source_register + i}];")
            else:
                lines.append(f"v[{target + i}] = g_lds[{index}];")
    statement = f"if ({active}) {{ {' '.join(lines)} }}"
    return [statement, LDS_BARRIER] if found.group(1) == "write" else [LDS_BARRIER, statement]


def _lanes(name, addr, data0, vdst, offset, active):
    """ds_swizzle/permute/bpermute : une ligne source inactive donne 0 (emu/rdna2_exec_lanes.py)."""
    data = f"(({active}) ? v[{data0}] : 0u)"
    target = f"(v[{addr}] + {offset}u) >> 2"
    if name == "ds_permute_b32":
        value = f"ForwardLane(lane, {active}, v[{data0}], {target})"
    elif name == "ds_bpermute_b32":
        value = f"ReadLane(lane, {data}, {target})"
    else:
        try:
            rdna2_exec_lanes.swizzle_source(offset, 0)
        except NotImplementedError as error:
            raise Untranslatable(f"non traduite en HLSL : {error}") from error
        value = f"ReadLane(lane, {data}, {_swizzle_source(offset)})"
    return [f"{{ uint value = {value}; if ({active}) v[{vdst}] = value; }}"]


def _swizzle_source(offset):
    """Expression HLSL de la ligne lue par ds_swizzle_b32 (QUAD_PERM ou BITMASK_PERM)."""
    if offset & swz.QUAD_PERM_ENC_MASK == swz.QUAD_PERM_ENC:
        return f"((lane & ~3u) + ((0x{offset & 0xFF:X}u >> ((lane & 3u) * 2u)) & 3u))"
    and_mask, or_mask, xor_mask = (offset >> shift & swz.BITMASK_MASK for shift in (0, 5, 10))
    return f"(((lane & {and_mask}u) | {or_mask}u) ^ {xor_mask}u)"


def memory_statements(family, name, word, active):
    """Instructions HLSL d'une instruction mémoire RDNA 2 (`word` : 64 bits)."""
    if name in ("s_dcache_inv", "s_gl1_inv", "s_dcache_wb", "buffer_gl0_inv", "buffer_gl1_inv", "ds_nop"):
        return []
    if family == "SMEM" and rdna2_exec_smem.supports(name):
        return rdna2_hlsl_smem.statements(name, word)
    if family == "SMEM" and SCALAR_PATTERN.match(name):
        return _scalar(name, word)
    if family == "DS" and (LDS_PATTERN.match(name) or LDS_ATOMIC.match(name) or name in rdna2_exec_lanes.NAMES
                           or name in LDS_ADD_TID):
        return _lds(name, word, active)
    if family in ("MUBUF", "MTBUF") and rdna2_hlsl_format.supports(name):
        return rdna2_hlsl_format.statements(name, word, active)
    if family == "MUBUF" and (VECTOR_PATTERN.match(name) or VECTOR_ATOMIC.match(name)):
        return _buffer(name, word, active)
    if family in ("GLOBAL", "FLAT") and (VECTOR_PATTERN.match(name) or VECTOR_ATOMIC.match(name) or name in ADD_TID):
        return _flat(family, name, word, active)
    raise Untranslatable(f"non traduite en HLSL : {name} ({family})")


__all__ = ["memory_statements"]
