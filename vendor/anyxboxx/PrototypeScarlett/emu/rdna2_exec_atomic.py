"""Sémantique des opérations atomiques RDNA 2 (mémoire vectorielle et LDS) de l'interpréteur.

Fonctions pures `f(ancienne, donnée0, donnée1) → nouvelle` sur des entiers de 32 ou 64 bits ;
les accès mémoire restent dans emu/rdna2_exec_memory.py. D'après la documentation publique
d'AMD (« RDNA 2 Instruction Set Architecture », chapitres FLAT, MUBUF et DS) :

- cmpswap (GLOBAL/FLAT/MUBUF) : DATA[0] = valeur écrite, DATA[1] = valeur comparée ;
  cmpst (DS) : l'ordre est inverse, DATA0 = valeur comparée, DATA1 = valeur écrite ;
- inc : 0 si ancienne ≥ DATA, sinon ancienne + 1 ; dec : DATA si ancienne = 0 ou > DATA,
  sinon ancienne - 1 ; csub : ancienne - DATA si ancienne ≥ DATA, sinon 0 ;
- wrap : ancienne - DATA0 si ancienne ≥ DATA0, sinon ancienne + DATA1 ;
- mskor : (ancienne & ~DATA0) | DATA1 ;
- fcmpswap/cmpst_f : égalité flottante (+0 = -0, NaN jamais égal) ;
- fmin/fmax : si une seule valeur est NaN, l'autre est gardée (minNum/maxNum d'IEEE 754-2008).
  Supposé d'après la description des V_MIN/V_MAX ; à confirmer sur le matériel.
"""
import math
import re
import struct

FLOAT_FORMATS = {32: "<f", 64: "<d"}
INTEGER_FORMATS = {32: "<I", 64: "<Q"}

VECTOR_ATOMIC_EXT = re.compile(
    r"^(global|flat|buffer)_atomic_(cmpswap|inc|dec|fcmpswap|fmin|fmax|csub"
    r"|(?:swap|cmpswap|add|sub|smin|umin|smax|umax|and|or|xor|inc|dec|fcmpswap|fmin|fmax)_x2)$")
LDS_ATOMIC_EXT = re.compile(
    r"^ds_(add|sub|rsub|inc|dec|min|max|and|or|xor|mskor|cmpst|wrxchg|wrap)(_rtn)?_(u32|b32|f32|u64|i64|b64|f64)$")
LDS_EXCHANGE2 = re.compile(r"^ds_wrxchg2(st64)?_rtn_(b32|b64)$")
STRIDE64 = 64                                # « st64 » : décalages multipliés par 64 éléments


def _float(value, width):
    return struct.unpack(FLOAT_FORMATS[width], struct.pack(INTEGER_FORMATS[width], value))[0]


def _bits(value, width):
    try:
        return struct.unpack(INTEGER_FORMATS[width], struct.pack(FLOAT_FORMATS[width], value))[0]
    except OverflowError:                    # dépassement de la simple précision : ±infini
        return _bits(math.copysign(math.inf, value), width)


def _signed(value, width):
    value &= (1 << width) - 1
    return value - (1 << width) if value >> (width - 1) else value


def _float_pick(choose, width):
    """fmin/fmax : NaN ignoré s'il est seul, sinon choose(ancienne, donnée)."""
    def operation(old, data, _):
        x, y = _float(old, width), _float(data, width)
        if math.isnan(x) or math.isnan(y):
            return data if math.isnan(x) and not math.isnan(y) else old
        return data if choose(y, x) else old
    return operation


def operation(kind, width):
    """f(ancienne, donnée0, donnée1) → nouvelle valeur (non masquée) pour une opération atomique."""
    sign = lambda value: _signed(value, width)            # noqa: E731
    flt = lambda value: _float(value, width)              # noqa: E731
    table = {
        "add": lambda o, a, b: o + a, "sub": lambda o, a, b: o - a, "rsub": lambda o, a, b: a - o,
        "swap": lambda o, a, b: a, "wrxchg": lambda o, a, b: a,
        "and": lambda o, a, b: o & a, "or": lambda o, a, b: o | a, "xor": lambda o, a, b: o ^ a,
        "umin": lambda o, a, b: min(o, a), "umax": lambda o, a, b: max(o, a),
        "smin": lambda o, a, b: min(sign(o), sign(a)), "smax": lambda o, a, b: max(sign(o), sign(a)),
        "inc": lambda o, a, b: 0 if o >= a else o + 1,
        "dec": lambda o, a, b: a if o == 0 or o > a else o - 1,
        "csub": lambda o, a, b: o - a if o >= a else 0,
        "wrap": lambda o, a, b: o - a if o >= a else o + b,
        "mskor": lambda o, a, b: (o & ~a) | b,
        "cmpswap": lambda o, a, b: a if o == b else o,
        "cmpst": lambda o, a, b: b if o == a else o,
        "fcmpswap": lambda o, a, b: a if flt(o) == flt(b) else o,
        "fcmpst": lambda o, a, b: b if flt(o) == flt(a) else o,
        "fadd": lambda o, a, b: _bits(flt(o) + flt(a), width),
        "fmin": _float_pick(lambda new, old: new < old, width),
        "fmax": _float_pick(lambda new, old: new > old, width),
    }
    function = table[kind]
    return lambda old, a, b: function(old, a, b) & ((1 << width) - 1)


def vector(name):
    """(largeur en bits, opération, nombre de VGPR de données) d'un atomique GLOBAL/FLAT/MUBUF, ou None."""
    found = VECTOR_ATOMIC_EXT.match(name)
    if not found:
        return None
    kind = found.group(2)
    width = 64 if kind.endswith("_x2") else 32
    kind = kind.removesuffix("_x2")
    words = width // 32 * (2 if kind in ("cmpswap", "fcmpswap") else 1)
    return width, operation(kind, width), words


LDS_KINDS = {("min", "u"): "umin", ("max", "u"): "umax", ("min", "i"): "smin", ("max", "i"): "smax",
             ("min", "f"): "fmin", ("max", "f"): "fmax", ("add", "f"): "fadd", ("cmpst", "f"): "fcmpst"}
LDS_FLOAT_KINDS = frozenset({"min", "max", "add", "cmpst"})


def lds(name):
    """(largeur, opération, renvoie l'ancienne valeur) d'un atomique DS non encore couvert, ou None."""
    found = LDS_ATOMIC_EXT.match(name)
    if not found:
        return None
    kind, returns, kind_type = found.groups()
    letter, width = kind_type[0], int(kind_type[1:])
    if letter == "f" and kind not in LDS_FLOAT_KINDS:
        return None
    if (kind, letter) in LDS_KINDS:
        kind = LDS_KINDS[kind, letter]
    elif kind in ("min", "max"):
        return None
    if kind in ("wrxchg", "wrap") and not returns:
        return None
    return width, operation(kind, width), bool(returns)


def lds_exchange2(name):
    """(largeur, pas des décalages en éléments) de ds_wrxchg2[st64]_rtn_b32/b64, ou None."""
    found = LDS_EXCHANGE2.match(name)
    if not found:
        return None
    return int(found.group(2)[1:]), STRIDE64 if found.group(1) else 1
