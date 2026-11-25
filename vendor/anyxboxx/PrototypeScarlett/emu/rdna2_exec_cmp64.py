"""Comparaisons vectorielles 64 bits (f64, i64, u64) et v_cmp_class_f32/f64 de l'interpréteur RDNA 2.

Une source 64 bits est une paire de VGPR (v[n], v[n+1]) ou de SGPR, ou une constante en ligne :
entier signé de -16 à 64 étendu sur 64 bits, ou flottant (0.5, 1.0…) en double précision pour
les sources f64. Un littéral 32 bits, ou une constante flottante sur une source entière 64 bits,
lève Untranslated : leur extension n'est pas encore vérifiée.

v_cmp_class teste la classe IEEE de S0 contre le masque S1 (bit 0 sNaN, 1 qNaN, 2 -infini,
3 -normal, 4 -dénormal, 5 -0, 6 +0, 7 +dénormal, 8 +normal, 9 +infini). Sémantique d'après la
documentation publique d'AMD (« RDNA 2 Instruction Set Architecture », VOPC).
"""
import struct

from emu.rdna2_exec_ops import FLOAT_TESTS, INTEGER_TESTS, MASK64, signed

VGPR_FIRST, LITERAL, EXEC_LO, VCC_LO = 256, 255, 126, 106
INLINE_INTEGERS = range(128, 209)
INLINE_DOUBLES = {240: 0.5, 241: -0.5, 242: 1.0, 243: -1.0, 244: 2.0, 245: -2.0, 246: 4.0, 247: -4.0,
                  248: 0.15915494309189532}       # 1/(2π) = 0x3FC45F306DC9C882, valeur exacte de l'ISA (§6.2)
SIGN32, SIGN64 = 1 << 31, 1 << 63
# (bits d'exposant, bits de mantisse) par largeur
FORMATS = {16: (5, 10), 32: (8, 23), 64: (11, 52)}


def double(bits):
    return struct.unpack("<d", struct.pack("<Q", bits & MASK64))[0]


def double_bits(value):
    return struct.unpack("<Q", struct.pack("<d", value))[0]


def float_class(bits, width):
    """Indice du bit de classe IEEE (0 à 9) d'un flottant de `width` bits."""
    exponent_bits, mantissa_bits = FORMATS[width]
    negative = bits >> (width - 1) & 1
    exponent = bits >> mantissa_bits & ((1 << exponent_bits) - 1)
    mantissa = bits & ((1 << mantissa_bits) - 1)
    if exponent == (1 << exponent_bits) - 1:
        if mantissa:
            return 1 if mantissa >> (mantissa_bits - 1) & 1 else 0       # bit de tête posé : NaN calme
        return 2 if negative else 9
    if exponent == 0:
        if mantissa == 0:
            return 5 if negative else 6
        return 4 if negative else 7
    return 3 if negative else 8


def _tests():
    """{nom: (test(a, b), écrit EXEC, type des sources)}."""
    table = {}
    for prefix, writes_exec in (("v_cmp", False), ("v_cmpx", True)):
        for name, test in FLOAT_TESTS.items():
            table[f"{prefix}_{name}_f64"] = (lambda a, b, t=test: t(double(a), double(b)), writes_exec, "f64")
        for name, test in INTEGER_TESTS.items():
            table[f"{prefix}_{name}_u64"] = (lambda a, b, t=test: t(a, b), writes_exec, "u64")
            table[f"{prefix}_{name}_i64"] = (lambda a, b, t=test: t(signed(a, 64), signed(b, 64)),
                                             writes_exec, "i64")
        table[f"{prefix}_class_f32"] = (lambda a, b: b >> float_class(a, 32) & 1, writes_exec, "class32")
        table[f"{prefix}_class_f64"] = (lambda a, b: b >> float_class(a, 64) & 1, writes_exec, "class64")
    return table


COMPARE64 = _tests()


def _untranslated(message):
    from emu.rdna2_exec import Untranslated

    return Untranslated(message)


def read64(wave, code, lane, is_float):
    """Source 64 bits d'une ligne (paire de registres ou constante en ligne)."""
    if code >= VGPR_FIRST:
        register = code - VGPR_FIRST
        return wave.v[register][lane] | wave.v[register + 1][lane] << 32
    if code < 128:
        return wave.read_scalar(code, None, 2)
    if code in INLINE_INTEGERS:
        return wave.read_scalar(code, None, 2) & MASK64
    if code in INLINE_DOUBLES and is_float:
        return double_bits(INLINE_DOUBLES[code])
    raise _untranslated(f"opérande 64 bits {code} non pris en charge")


def _sources(wave, kind, codes, lane, literal):
    """(S0, S1) d'une ligne selon le type de la comparaison."""
    if kind == "class32":
        return wave.read_lane(codes[0], lane, literal), wave.read_lane(codes[1], lane, literal)
    if kind == "class64":
        return read64(wave, codes[0], lane, True), wave.read_lane(codes[1], lane, literal)
    is_float = kind == "f64"
    return read64(wave, codes[0], lane, is_float), read64(wave, codes[1], lane, is_float)


def _modify(value, index, kind, abs_bits, neg_bits):
    """Modificateurs VOP3 abs/neg : bit de signe des sources flottantes (S0 seule pour class)."""
    if kind in ("u64", "i64") or (kind.startswith("class") and index == 1):
        return value
    sign = SIGN32 if kind == "class32" else SIGN64
    if abs_bits >> index & 1:
        value &= ~sign
    if neg_bits >> index & 1:
        value ^= sign
    return value


def execute(wave, name, codes, literal, destination=VCC_LO, abs_bits=0, neg_bits=0):
    """Comparaison sur les lignes actives ; masque écrit dans EXEC (v_cmpx) ou `destination`."""
    test, writes_exec, kind = COMPARE64[name]
    wide = {"class32": (), "class64": codes[:1]}.get(kind, codes[:2])      # sources lues sur 64 bits
    if LITERAL in wide:
        raise _untranslated(f"littéral dans une comparaison 64 bits : {name}")
    mask = 0
    for lane in wave.active():
        a, b = _sources(wave, kind, codes, lane, literal)
        a, b = _modify(a, 0, kind, abs_bits, neg_bits), _modify(b, 1, kind, abs_bits, neg_bits)
        mask |= bool(test(a, b)) << lane
    wave.write_mask(EXEC_LO if writes_exec else destination, mask)
