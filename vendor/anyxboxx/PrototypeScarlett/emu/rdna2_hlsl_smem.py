"""Traduction HLSL des écritures scalaires en mémoire (emu/rdna2_exec_smem.py).

Une instruction scalaire s'exécute une fois par wave : seul le thread 0 écrit ou fait l'atomique,
l'ancienne valeur est diffusée aux 32 copies de s[] par ReadLane, puis une barrière rend l'écriture
visible du groupe. Atomiques 32 bits : fonctions Interlocked* de RWByteAddressBuffer (inc et dec par
une boucle de comparaison-échange) ; les formes _x2 (atomiques 64 bits, Shader Model 6.6) restent
nommées par Untranslatable. Comme pour s_load, seules les adresses 32 bits basses sont utilisées.
"""
from emu.rdna2_exec_smem import ATOMIC, STORE, WORDS, fields
from emu.rdna2_hlsl_ops import NULL, Untranslatable, hexa

STORE_BARRIER = "AllMemoryBarrierWithGroupSync();"
INTERLOCKED = {"add": "InterlockedAdd", "swap": "InterlockedExchange", "and": "InterlockedAnd",
               "or": "InterlockedOr", "xor": "InterlockedXor", "umin": "InterlockedMin", "umax": "InterlockedMax"}
SIGNED = {"smin": "InterlockedMin", "smax": "InterlockedMax"}
# inc / dec : nouvelle valeur à partir de l'ancienne (o) et de la donnée (x), comme rdna2_exec_atomic.
LOOPED = {"inc": "o >= x ? 0u : o + 1u", "dec": "(o == 0u || o > x) ? x : o - 1u"}


def _place(buffer, sbase, offset, soffset, size):
    """(adresse, condition « dans le buffer ») en HLSL."""
    extra = "" if soffset == NULL else f" + s[{soffset}]"
    if buffer:
        inside = f"(asint(o) >= 0 && o + {size}u <= s[{sbase + 2}])"
        return f"s[{sbase}] + o", inside, f"uint o = {hexa(offset)}{extra}; "
    return f"s[{sbase}] + o", "true", f"uint o = {hexa(offset)}{extra}; "


def _atomic_body(kind, sdata):
    data = f"s[{sdata}]"
    if kind in INTERLOCKED:
        return f"g_memory.{INTERLOCKED[kind]}(a, {data}, old);"
    if kind == "sub":
        return f"g_memory.InterlockedAdd(a, 0u - {data}, old);"
    if kind in SIGNED:
        return f"int r; g_memory.{SIGNED[kind]}(a, asint({data}), r); old = asuint(r);"
    if kind == "cmpswap":
        return f"g_memory.InterlockedCompareExchange(a, s[{sdata + 1}], {data}, old);"
    return (f"uint x = {data}, o = g_memory.Load(a); [loop] for (;;) {{ uint seen; "
            f"g_memory.InterlockedCompareExchange(a, o, {LOOPED[kind]}, seen); if (seen == o) break; o = seen; }} "
            f"old = o;")


def statements(name, word):
    """Instructions HLSL d'une écriture scalaire (`word` : 64 bits)."""
    sdata, sbase, offset, soffset, glc = fields(word)
    store = STORE.match(name)
    if store:
        count = WORDS[store.group(2)]
        address, inside, prefix = _place(store.group(1), sbase, offset, soffset, 4 * count)
        writes = " ".join(f"g_memory.Store(a + {4 * i}u, s[{sdata + i}]);" for i in range(count))
        return [f"{{ {prefix}uint a = {address}; if (lane == 0u && {inside}) {{ {writes} }} }}", STORE_BARRIER]
    found = ATOMIC.match(name)
    if found is None or found.group(3):
        raise Untranslatable(f"non traduite en HLSL : {name} (atomique scalaire 64 bits)")
    address, inside, prefix = _place(found.group(1), sbase, offset, soffset, 4)
    keep = f" if ({inside}) s[{sdata}] = old;" if glc else ""
    return [f"{{ {prefix}uint a = {address}; uint old = 0u; "
            f"if (lane == 0u && {inside}) {{ {_atomic_body(found.group(2), sdata)} }} "
            f"old = ReadLane(lane, old, 0u);{keep} }}", STORE_BARRIER]
