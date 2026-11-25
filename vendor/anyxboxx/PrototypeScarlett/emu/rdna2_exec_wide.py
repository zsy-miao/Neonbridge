"""Instructions RDNA 2 à opérandes 64 bits de l'interpréteur : flottants double précision (VOP1 et
VOP3) et décalages 64 bits.

Une source « f64 », « i64 » ou « u64 » est une paire de VGPR ou de SGPR, ou une constante en ligne
(emu/rdna2_exec_cmp64.read64) ; un littéral lève Untranslated. Le résultat 64 bits s'écrit dans
vdst et vdst + 1. Modificateurs VOP3 : abs/neg sur le bit de signe des sources flottantes,
clamp/omod sur un résultat flottant. Les doubles Python suivent IEEE 754 (arrondi au plus près) ;
v_fma_f64 arrondit une seule fois (calcul exact en fractions). D'après la documentation publique
d'AMD (« RDNA 2 Instruction Set Architecture »).

Arrondis et v_fract_f64 rendent un NaN d'entrée silencieux, signe et charge utile gardés
(IEEE 754) ; les arrondis (trunc, ceil, floor, rndne) gardent le signe de zéro (trunc(-0.5) = -0.0).
v_fract_f64 suit la formule de l'ISA (S0 + -floor(S0)), bornée au plus grand double sous 1.0 comme
le modèle de l'instruction dans LLVM (ConstantFolding, amdgcn_fract : « fract(-small) » ne rend
jamais 1.0) ; lu dans LLVM, pas mesuré sur le matériel.

Écart connu : l'ISA donne v_rcp_f64, v_rsq_f64 et v_sqrt_f64 à 2^29 ULP près ; l'interpréteur rend
le résultat arrondi au plus près, dans cette marge mais pas forcément au bit près du matériel. Ces
trois instructions ne sont pas traduites en HLSL, rien ne compare encore leurs derniers bits.
"""
import math
import struct
from fractions import Fraction

from emu import rdna2_exec_cmp64
from emu.rdna2_exec_ops import INT32_MAX, INT32_MIN, MASK32, MASK64, _to_int, bits_float, float_bits, ldexp, signed

LITERAL = 255
SIGNS = {"f32": 1 << 31, "f64": 1 << 63}
QUIET_F64 = 1 << 51
ALMOST_ONE = 0x3FEF_FFFF_FFFF_FFFF                       # plus grand double sous 1.0
OUTPUT_SCALE = {1: 2.0, 2: 4.0, 3: 0.5}


def double(bits):
    return struct.unpack("<d", struct.pack("<Q", bits & MASK64))[0]


def double_bits(value):
    return struct.unpack("<Q", struct.pack("<d", value))[0]


def _quiet(x):
    """NaN rendu silencieux, signe et charge utile gardés."""
    return double(double_bits(x) | QUIET_F64)


def _rounded(function):
    """Fonction d'arrondi : ±infini rendu tel quel, NaN rendu silencieux ; le résultat a le signe de x
    (IEEE 754 roundToIntegral : trunc(-0.5) = -0.0, floor(-0.0) = -0.0)."""
    def operation(x):
        if math.isnan(x):
            return _quiet(x)
        return x if math.isinf(x) else math.copysign(float(function(x)), x)
    return operation


def _divide(numerator, x):
    if x == 0:
        return math.copysign(math.inf, x) * numerator
    return numerator / x


def _sqrt(x):
    return math.nan if x < 0 else math.sqrt(x)


def _rsq(x):
    if x == 0:
        return math.copysign(math.inf, x)
    return math.nan if x < 0 else 1.0 / math.sqrt(x)


def _fract(x):
    """S0 + -floor(S0) (ISA), au plus le plus grand double sous 1.0 ; NaN rendu silencieux, ±infini → NaN."""
    if math.isnan(x):
        return _quiet(x)
    if math.isinf(x):
        return math.nan
    return min(x - _rounded(math.floor)(x), double(ALMOST_ONE))


def _frexp_mantissa(x):
    return x if x == 0 or math.isinf(x) or math.isnan(x) else math.frexp(x)[0]


def _frexp_exponent(x):
    return 0 if math.isinf(x) or math.isnan(x) else math.frexp(x)[1]


def _fma(a, b, c):
    """a × b + c arrondi une seule fois ; valeurs spéciales par le calcul flottant ordinaire."""
    if not all(math.isfinite(v) for v in (a, b, c)):
        return a * b + c
    return float(Fraction(a) * Fraction(b) + Fraction(c))


def _pick(choose):
    """min/max : un NaN seul est ignoré (minNum/maxNum d'IEEE 754-2008)."""
    def operation(x, y):
        if math.isnan(x) or math.isnan(y):
            return y if math.isnan(x) else x
        return x if choose(x, y) else y
    return operation


def _shift(kind):
    def operation(amount, value):
        amount &= 63
        if kind == "lshl":
            return value << amount
        if kind == "lshr":
            return value >> amount
        return signed(value, 64) >> amount
    return operation


# nom : (types des sources, type du résultat, fonction des valeurs décodées)
OPERATIONS = {
    "v_cvt_i32_f64": (("f64",), "i32", lambda x: _to_int(x, INT32_MIN, INT32_MAX)),
    "v_cvt_u32_f64": (("f64",), "u32", lambda x: _to_int(x, 0, MASK32)),
    "v_cvt_f64_i32": (("i32",), "f64", float),
    "v_cvt_f64_u32": (("u32",), "f64", float),
    "v_cvt_f32_f64": (("f64",), "f32", lambda x: x),
    "v_cvt_f64_f32": (("f32",), "f64", lambda x: x),
    "v_trunc_f64": (("f64",), "f64", _rounded(math.trunc)),
    "v_ceil_f64": (("f64",), "f64", _rounded(math.ceil)),
    "v_floor_f64": (("f64",), "f64", _rounded(math.floor)),
    "v_rndne_f64": (("f64",), "f64", _rounded(round)),
    "v_fract_f64": (("f64",), "f64", _fract),
    "v_rcp_f64": (("f64",), "f64", lambda x: _divide(1.0, x)),
    "v_rsq_f64": (("f64",), "f64", _rsq),
    "v_sqrt_f64": (("f64",), "f64", _sqrt),
    "v_frexp_exp_i32_f64": (("f64",), "i32", _frexp_exponent),
    "v_frexp_mant_f64": (("f64",), "f64", _frexp_mantissa),
    "v_add_f64": (("f64", "f64"), "f64", lambda x, y: x + y),
    "v_mul_f64": (("f64", "f64"), "f64", lambda x, y: x * y),
    "v_min_f64": (("f64", "f64"), "f64", _pick(lambda x, y: x < y)),
    "v_max_f64": (("f64", "f64"), "f64", _pick(lambda x, y: x > y)),
    "v_fma_f64": (("f64", "f64", "f64"), "f64", _fma),
    "v_ldexp_f64": (("f64", "i32"), "f64", lambda x, n: ldexp(x, n)),
    "v_lshlrev_b64": (("u32", "u64"), "u64", _shift("lshl")),
    "v_lshrrev_b64": (("u32", "u64"), "u64", _shift("lshr")),
    "v_ashrrev_i64": (("u32", "u64"), "u64", _shift("ashr")),
}
WIDE_SOURCES = frozenset({"f64", "u64"})


def _untranslated(message):
    from emu.rdna2_exec import Untranslated

    return Untranslated(message)


def _read(wave, code, kind, lane, literal):
    if kind in WIDE_SOURCES:
        if code == LITERAL:
            raise _untranslated("littéral sur une source 64 bits")
        return rdna2_exec_cmp64.read64(wave, code, lane, kind == "f64")
    return wave.read_lane(code, lane, literal)


def _decode(bits, kind):
    if kind == "f64":
        return double(bits)
    if kind == "f32":
        return bits_float(bits)
    return signed(bits) if kind == "i32" else bits


def _encode(value, kind):
    if kind == "f64":
        return double_bits(value)
    if kind == "f32":
        return float_bits(value)
    return int(value) & (MASK64 if kind == "u64" else MASK32)


def _modify(bits, kind, index, abs_bits, neg_bits):
    if kind not in SIGNS:
        return bits
    if abs_bits >> index & 1:
        bits &= ~SIGNS[kind]
    if neg_bits >> index & 1:
        bits ^= SIGNS[kind]
    return bits


def _output(value, kind, clamp, omod):
    if kind not in SIGNS or not (clamp or omod) or math.isnan(value):
        return value
    value *= OUTPUT_SCALE.get(omod, 1.0)
    return min(max(value, 0.0), 1.0) if clamp else value


def execute(wave, name, codes, vdst, literal, abs_bits=0, neg_bits=0, clamp=0, omod=0):
    """Exécute une instruction de OPERATIONS sur les lignes actives."""
    kinds, result_kind, function = OPERATIONS[name]
    for lane in wave.active():
        values = []
        for index, kind in enumerate(kinds):
            bits = _modify(_read(wave, codes[index], kind, lane, literal), kind, index, abs_bits, neg_bits)
            values.append(_decode(bits, kind))
        result = _encode(_output(function(*values), result_kind, clamp, omod), result_kind)
        wave.v[vdst][lane] = result & MASK32
        if result_kind in WIDE_SOURCES:
            wave.v[vdst + 1][lane] = result >> 32 & MASK32
