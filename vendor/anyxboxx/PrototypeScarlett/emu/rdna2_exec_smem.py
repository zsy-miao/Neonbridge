"""Écritures scalaires en mémoire (SMEM) de l'interpréteur RDNA 2 : s_store_dword[x2,x4],
s_buffer_store_dword[x2,x4], s_atomic_* et s_buffer_atomic_*.

La documentation RDNA 2 d'AMD (spécification XML) ne décrit pas ces instructions ; LLVM les encode
pour gfx10 (tests MC, d'où emu/rdna2_ops.py). Sémantique d'après la spécification XML CDNA 1
(famille GCN 5, mêmes opcodes) et les atomiques vectoriels (emu/rdna2_exec_atomic.py) :
- adresse : SGPR[sbase:sbase+1] (64 bits) + offset signé 21 bits (+ SGPR soffset), comme s_load ;
  formes s_buffer_ : base du V# en SGPR[sbase:sbase+3], hors du buffer rien n'est écrit ;
- atomiques : SDATA[0] (32 bits, ou SDATA[0:1] pour _x2) est la donnée, cmpswap prend la valeur
  comparée dans le mot suivant ; avec glc (bit 16), l'ancienne valeur revient dans SDATA ;
  hors du buffer, rien n'est écrit ni rendu (comme MUBUF dans emu/rdna2_exec_memory.py).
Supposé : que le matériel RDNA 2 les exécute comme GCN 5. Les formes s_scratch_ (ouverture scratch
du wave, non modélisée) restent non exécutées.
"""
import re

from emu import rdna2_exec_atomic
from emu.rdna2_exec_ops import MASK32, signed

NULL = 125
DWORD = 4
GLC_BIT = 16
STORE = re.compile(r"^s_(buffer_)?store_(dword|dwordx2|dwordx4)$")
ATOMIC = re.compile(r"^s_(buffer_)?atomic_(swap|cmpswap|add|sub|smin|umin|smax|umax|and|or|xor|inc|dec)(_x2)?$")
WORDS = {"dword": 1, "dwordx2": 2, "dwordx4": 4}


def supports(name):
    return bool(STORE.match(name) or ATOMIC.match(name))


def fields(word):
    """(sdata, sbase, décalage en octets sans soffset, code soffset, glc)."""
    return ((word >> 6) & 0x7F, (word & 0x3F) * 2, signed((word >> 32) & 0x1FFFFF, 21), (word >> 57) & 0x7F,
            (word >> GLC_BIT) & 1)


def _place(wave, buffer, sbase, offset, size):
    """(adresse, dans le buffer) d'un accès de `size` octets."""
    if buffer:
        from emu.rdna2_exec_memory import descriptor

        base, _, records = descriptor(wave, sbase)
        return base + offset, 0 <= offset and offset + size <= records
    return (wave.s[sbase] | wave.s[sbase + 1] << 32) + offset, True


def _scalar_value(wave, first, words):
    return sum(wave.s[first + i] << (32 * i) for i in range(words))


def execute(wave, name, word):
    """Exécute une instruction dont `supports(name)` est vrai ; `word` : 64 bits."""
    sdata, sbase, offset, soffset, glc = fields(word)
    if soffset != NULL:
        offset += wave.s[soffset]
    store = STORE.match(name)
    if store:
        count = WORDS[store.group(2)]
        address, inside = _place(wave, store.group(1), sbase, offset, count * DWORD)
        if inside:
            wave.memory.write_words(address, wave.s[sdata:sdata + count])
        return
    found = ATOMIC.match(name)
    kind, width = found.group(2), 64 if found.group(3) else 32
    words = width // 32
    address, inside = _place(wave, found.group(1), sbase, offset, words * DWORD)
    if not inside:
        return
    old = int.from_bytes(wave.memory.read(address, words * DWORD), "little")
    data = _scalar_value(wave, sdata, words)
    compare = _scalar_value(wave, sdata + words, words) if kind == "cmpswap" else 0
    new = rdna2_exec_atomic.operation(kind, width)(old, data, compare)
    wave.memory.write(address, new.to_bytes(words * DWORD, "little"))
    if glc:                                       # l'ancienne valeur revient dans SDATA
        for i in range(words):
            wave.s[sdata + i] = old >> (32 * i) & MASK32
