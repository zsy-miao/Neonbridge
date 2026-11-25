"""Sémantique des instructions RDNA 2 exécutées par emu/rdna2_exec.py.

Les tables associent un mnémonique à sa fonction, d'après la documentation publique d'AMD
(« RDNA 2 Instruction Set Architecture »). Les flottants sont calculés en double précision
puis arrondis en simple précision (les FMA une seule fois, voir _fma) ; les dénormaux ne sont pas
traités à part.
"""
import math
import struct

from emu import rdna2_exec_bits, rdna2_exec_cmp16

MASK32, MASK64 = 0xFFFF_FFFF, (1 << 64) - 1
EXEC = 126                                  # exec_lo ; exec_hi en 127
INT32_MIN, INT32_MAX = -(1 << 31), (1 << 31) - 1
POSITIVE_INFINITY, NEGATIVE_INFINITY = 0x7F80_0000, 0xFF80_0000
ALMOST_ONE_F32 = 0x3F7F_FFFF                # plus grand flottant sous 1.0


def float_bits(value):
    """Bits IEEE 754 simple précision ; ±infini si la valeur dépasse le format."""
    try:
        return struct.unpack("<I", struct.pack("<f", value))[0]
    except OverflowError:
        return POSITIVE_INFINITY if value > 0 else NEGATIVE_INFINITY


EXPONENT_F32, MANTISSA_F32, QUIET_F32 = 0x7F80_0000, 0x007F_FFFF, 0x0040_0000


def bits_float(bits):
    """Flottant d'un motif f32 ; un NaN signalant revient silencieux (bit 22 posé).

    CPython 3.12 et 3.13 le rendaient silencieux en convertissant le float en double ; CPython 3.14 garde
    le bit signalant (struct.unpack("<f") de 0x7F800001 puis struct.pack("<f") redonne 0x7F800001 au lieu
    de 0x7FC00001, mesuré sur 3.12.13, 3.13.13 et 3.14.10). Le bit est posé ici pour que toutes les
    versions suivent la même règle (celle de _first_nan et des tests de NaN).
    """
    bits &= MASK32
    if bits & EXPONENT_F32 == EXPONENT_F32 and bits & MANTISSA_F32:
        bits |= QUIET_F32
    return struct.unpack("<f", struct.pack("<I", bits))[0]


def signed(value, bits=32):
    value &= (1 << bits) - 1
    return value - (1 << bits) if value >> (bits - 1) else value


def _float_op(function):
    def operation(*args):
        try:
            return float_bits(function(*args))
        except ZeroDivisionError:
            return POSITIVE_INFINITY
        except ValueError:
            return float_bits(math.nan)
    return operation


def _to_int(value, low, high):
    """Conversion saturée : NaN → 0, ±infini et hors bornes → la borne la plus proche."""
    if math.isnan(value):
        return 0
    if math.isinf(value):
        return high if value > 0 else low
    return int(min(max(math.trunc(value), low), high))


def _exp2(x):
    """2^x ; au-delà du format double, +infini (Python lève OverflowError)."""
    try:
        return 2.0 ** x
    except OverflowError:
        return math.inf


def _signed_round(function):
    """Arrondi entier qui garde le signe de x (IEEE 754 roundToIntegral : trunc(-0.5) = -0.0) ; ±infini
    inchangé ; NaN : ValueError, rattrapée par _float_op."""
    return lambda x: x if math.isinf(x) else math.copysign(float(function(x)), x)


def _fract(x):
    """x + -floor(x) (ISA), au plus le plus grand flottant sous 1.0 comme le modèle de LLVM
    (ConstantFolding, amdgcn_fract) ; NaN pour ±infini, où floor ne donne pas d'entier."""
    if math.isinf(x):
        return math.nan
    return min(x - _signed_round(math.floor)(x), bits_float(ALMOST_ONE_F32))


def ldexp(x, exponent):
    """x × 2^exponent ; ±infini quand le résultat dépasse le format double."""
    try:
        return math.ldexp(x, exponent)
    except OverflowError:
        return math.copysign(math.inf, x)


def _reciprocal(x):
    return math.copysign(math.inf, x) if x == 0 else 1.0 / x


def _log2(x):
    return -math.inf if x == 0 else math.log2(x)


# --- Scalaires --------------------------------------------------------------------------

def _sop2(operation, width=1, second_width=None, first_width=None):
    def execute(wave, f, literal):
        a = wave.read_scalar(f["ssrc0"], literal, first_width or width)
        b = wave.read_scalar(f["ssrc1"], literal, second_width or width)
        result, scc = operation(a, b, wave)
        wave.write_scalar(f["sdst"], result, width)
        if scc is not None:
            wave.scc = scc
    return execute


def _sop1(operation, width=1):
    def execute(wave, f, literal):
        result, scc = operation(wave.read_scalar(f["ssrc0"], literal, width), wave)
        if result is not None:
            wave.write_scalar(f["sdst"], result, width)
        if scc is not None:
            wave.scc = scc
    return execute


def _saveexec(combine, width=1, writes_new=False):
    """EXEC = combine(S0, EXEC) ; D = ancien EXEC (saveexec) ou nouvel EXEC (wrexec)."""
    mask = MASK64 if width == 2 else MASK32

    def execute(wave, f, literal):
        previous = wave.read_scalar(EXEC, literal, width)
        new = combine(wave.read_scalar(f["ssrc0"], literal, width), previous) & mask
        wave.write_scalar(EXEC, new, width)
        wave.write_scalar(f["sdst"], new if writes_new else previous, width)
        wave.scc = new != 0
    return execute


def _compare_scalar(test, width=1, is_signed=False, second_width=None):
    def execute(wave, f, literal):
        a = wave.read_scalar(f["ssrc0"], literal, width)
        b = wave.read_scalar(f["ssrc1"], literal, second_width or width)
        if is_signed:
            a, b = signed(a), signed(b)
        wave.scc = bool(test(a, b))
    return execute


def _sopk(operation):
    def execute(wave, f, _literal):
        immediate = signed(f["simm16"], 16)
        result, scc = operation(wave.s[f["sdst"]] if f["sdst"] < 128 else 0, immediate, f["simm16"], wave)
        if result is not None:
            wave.write_scalar(f["sdst"], result)
        if scc is not None:
            wave.scc = scc
    return execute


def _overflow(a, b, result):
    return not INT32_MIN <= signed(a) + b <= INT32_MAX if result is None else not INT32_MIN <= result <= INT32_MAX


def _logic(function, width=1):
    mask = MASK64 if width == 2 else MASK32
    return _sop2(lambda a, b, w: (function(a, b) & mask, function(a, b) & mask != 0), width)


COMPARISONS = {"eq": lambda a, b: a == b, "lg": lambda a, b: a != b, "gt": lambda a, b: a > b,
               "ge": lambda a, b: a >= b, "lt": lambda a, b: a < b, "le": lambda a, b: a <= b}
# Sans effet architectural dans ce modèle (wave séquentiel, sans cache ni compteur de performance) :
# invalidation du cache d'instructions, compteurs de performance, trace, attentes sur les compteurs.
SOPP_NO_EFFECT = ("s_icache_inv", "s_incperflevel", "s_decperflevel", "s_ttracedata_imm", "s_wait_idle",
                  "s_waitcnt_depctr")
NO_OPERATION = ("s_nop", "s_waitcnt", "s_barrier", "s_sleep", "s_setprio", "s_clause", "s_inst_prefetch",
                "s_waitcnt_vscnt", "s_waitcnt_vmcnt", "s_waitcnt_expcnt", "s_waitcnt_lgkmcnt", "s_ttracedata",
                *SOPP_NO_EFFECT)

SCALAR = {
    "s_add_u32": _sop2(lambda a, b, w: (a + b, (a + b) >> 32 != 0)),
    "s_sub_u32": _sop2(lambda a, b, w: (a - b, a < b)),
    "s_addc_u32": _sop2(lambda a, b, w: (a + b + w.scc, (a + b + w.scc) >> 32 != 0)),
    "s_subb_u32": _sop2(lambda a, b, w: (a - b - w.scc, a < b + w.scc)),
    "s_add_i32": _sop2(lambda a, b, w: (a + b, _overflow(0, 0, signed(a) + signed(b)))),
    "s_sub_i32": _sop2(lambda a, b, w: (a - b, _overflow(0, 0, signed(a) - signed(b)))),
    "s_mul_i32": _sop2(lambda a, b, w: (signed(a) * signed(b), None)),
    "s_min_u32": _sop2(lambda a, b, w: (min(a, b), a < b)),
    "s_max_u32": _sop2(lambda a, b, w: (max(a, b), a > b)),
    "s_min_i32": _sop2(lambda a, b, w: (min(signed(a), signed(b)), signed(a) < signed(b))),
    "s_max_i32": _sop2(lambda a, b, w: (max(signed(a), signed(b)), signed(a) > signed(b))),
    "s_cselect_b32": _sop2(lambda a, b, w: (a if w.scc else b, None)),
    "s_cselect_b64": _sop2(lambda a, b, w: (a if w.scc else b, None), 2),
    "s_and_b32": _logic(lambda a, b: a & b), "s_or_b32": _logic(lambda a, b: a | b),
    "s_xor_b32": _logic(lambda a, b: a ^ b), "s_andn2_b32": _logic(lambda a, b: a & ~b),
    "s_orn2_b32": _logic(lambda a, b: a | ~b), "s_nand_b32": _logic(lambda a, b: ~(a & b)),
    "s_nor_b32": _logic(lambda a, b: ~(a | b)), "s_xnor_b32": _logic(lambda a, b: ~(a ^ b)),
    "s_and_b64": _logic(lambda a, b: a & b, 2), "s_or_b64": _logic(lambda a, b: a | b, 2),
    "s_xor_b64": _logic(lambda a, b: a ^ b, 2), "s_andn2_b64": _logic(lambda a, b: a & ~b, 2),
    "s_lshl_b32": _sop2(lambda a, b, w: ((a << (b & 31)) & MASK32, (a << (b & 31)) & MASK32 != 0)),
    "s_lshr_b32": _sop2(lambda a, b, w: (a >> (b & 31), a >> (b & 31) != 0)),
    "s_ashr_i32": _sop2(lambda a, b, w: (signed(a) >> (b & 31), signed(a) >> (b & 31) != 0)),
    "s_lshl_b64": _sop2(lambda a, b, w: ((a << (b & 63)) & MASK64, (a << (b & 63)) & MASK64 != 0), 2, 1),
    "s_lshr_b64": _sop2(lambda a, b, w: (a >> (b & 63), a >> (b & 63) != 0), 2, 1),
    "s_bfm_b32": _sop2(lambda a, b, w: (((1 << (a & 31)) - 1) << (b & 31), None)),
    "s_mov_b32": _sop1(lambda a, w: (a, None)),
    "s_mov_b64": _sop1(lambda a, w: (a, None), 2),
    "s_cmov_b32": _sop1(lambda a, w: (a if w.scc else None, None)),
    "s_not_b32": _sop1(lambda a, w: (~a & MASK32, ~a & MASK32 != 0)),
    "s_not_b64": _sop1(lambda a, w: (~a & MASK64, ~a & MASK64 != 0), 2),
    "s_brev_b32": _sop1(lambda a, w: (int(f"{a:032b}"[::-1], 2), None)),
    "s_bcnt1_i32_b32": _sop1(lambda a, w: (bin(a).count("1"), a != 0)),
    "s_ff1_i32_b32": _sop1(lambda a, w: ((a & -a).bit_length() - 1 if a else MASK32, None)),
    "s_flbit_i32_b32": _sop1(lambda a, w: (32 - a.bit_length() if a else MASK32, None)),
    "s_sext_i32_i8": _sop1(lambda a, w: (signed(a, 8), None)),
    "s_sext_i32_i16": _sop1(lambda a, w: (signed(a, 16), None)),
    "s_abs_i32": _sop1(lambda a, w: (abs(signed(a)), a != 0)),
    "s_movk_i32": _sopk(lambda d, k, raw, w: (k, None)),
    "s_cmovk_i32": _sopk(lambda d, k, raw, w: (k if w.scc else None, None)),
    "s_addk_i32": _sopk(lambda d, k, raw, w: (signed(d) + k, _overflow(0, 0, signed(d) + k))),
    "s_mulk_i32": _sopk(lambda d, k, raw, w: (signed(d) * k, None)),
    "s_bitcmp0_b32": _compare_scalar(lambda a, b: not a >> (b & 31) & 1),
    "s_bitcmp1_b32": _compare_scalar(lambda a, b: a >> (b & 31) & 1),
}
for _name, _test in COMPARISONS.items():
    SCALAR[f"s_cmp_{_name}_u32"] = _compare_scalar(_test)
    SCALAR[f"s_cmp_{_name}_i32"] = _compare_scalar(_test, is_signed=True)
    SCALAR[f"s_cmpk_{_name}_u32"] = _sopk(lambda d, k, raw, w, t=_test: (None, t(d, raw)))
    SCALAR[f"s_cmpk_{_name}_i32"] = _sopk(lambda d, k, raw, w, t=_test: (None, t(signed(d), k)))
SCALAR["s_cmp_eq_u64"] = _compare_scalar(COMPARISONS["eq"], 2)
SCALAR["s_cmp_lg_u64"] = _compare_scalar(COMPARISONS["lg"], 2)
for _name in NO_OPERATION:
    SCALAR[_name] = lambda wave, f, literal: None
SCALAR.update(rdna2_exec_bits.scalar_operations(_sop1, _sop2, _logic, _saveexec, _compare_scalar))

BRANCHES = {
    "s_branch": lambda w: True,
    "s_cbranch_scc0": lambda w: not w.scc, "s_cbranch_scc1": lambda w: w.scc,
    "s_cbranch_vccz": lambda w: w.vcc == 0, "s_cbranch_vccnz": lambda w: w.vcc != 0,
    "s_cbranch_execz": lambda w: w.exec == 0, "s_cbranch_execnz": lambda w: w.exec != 0,
}

# --- Vectorielles : operation(a = src0, b = vsrc1, d = destination, littéral, bit vcc) ----

F = bits_float


def _unary_float(function):
    operation = _float_op(function)
    return lambda a, b, d, k, c: operation(F(a))


def _binary_float(function):
    operation = _float_op(function)
    return lambda a, b, d, k, c: operation(F(a), F(b))


def _mul_add(a, b, c):
    """v_mac_f32, v_mad_f32 : calcul gardé tel quel (somme double, puis arrondi en simple).

    L'ISA RDNA 2 les donne pour retirées (« Removed: [...] V_MAC_F32, V_MADMK_F32, V_MADAK_F32 (replaced
    by FMA equivalents) », p. 3) et ses tables d'opcodes VOP2 et VOP3 (chapitre 12) n'ont ni V_MAC_F32 ni
    V_MAD_F32 : elles viennent des encodages gfx10 (RDNA 1) des tests de LLVM, qui les réserve au
    prédicat HasMadMacF32Insts et y associe V_MAD_F32 à `any_fmad`, la multiplication-addition non
    fusionnée (VOP3Instructions.td). Leur arrondi exact n'est pas modélisé ici.
    """
    return a * b + c


_TWO_53 = 2.0 ** 53


def _first_nan(a, b, c, total):
    """NaN de a × b + c : le premier opérande NaN (déjà rendu silencieux par la conversion en double),
    sinon (∞ × 0, ∞ − ∞) le NaN silencieux positif 0x7FC00000. Supposé, pas encore mesuré sur RDNA 2."""
    for operand in (a, b, c):
        if operand != operand:
            return operand
    return total if total == total else math.nan


def _fma(a, b, c):
    """a × b + c arrondi une seule fois en simple précision.

    ISA RDNA 2 (AMD, novembre 2020, p. 169) : « V_FMA_F32 Fused single precision multiply add. 0.5ULP
    accuracy, denormals are supported » ; V_FMAC_F32, V_FMAMK_F32 et V_FMAAK_F32 (p. 134) sont dites
    « fused » aussi. Le produit de deux f32 (48 bits) est exact en double, pas la somme : arrondie au
    plus proche, puis en simple, elle peut tomber pile entre deux f32 et partir du mauvais côté (double
    arrondi, signalé sur #133 par akimfomin avec PortBridge). La somme est donc arrondie « à l'impair »
    (Boldo et Melquiond, 2008) : erreur exacte par TwoSum (Knuth), et si la somme est inexacte avec un
    dernier bit pair, passage au double voisin du côté de la valeur exacte. Avec 53 bits contre 24 + 2,
    l'arrondi en simple qui suit (float_bits) est celui de la valeur exacte, sous-normaux et
    débordement compris.
    """
    product = a * b
    total = product + c
    if not abs(total) < math.inf:
        return _first_nan(a, b, c, total)
    back = total - product
    error = (product - (total - back)) + (c - back)
    if error and not math.frexp(total)[0] * _TWO_53 % 2.0:
        return math.nextafter(total, math.inf if error > 0 else -math.inf)
    return total


def _legacy_fma(x, y, z):
    """« DX9 rules, 0.0 * x = 0.0 » (ISA RDNA 2, p. 167) ; sinon la FMA arrondie une seule fois."""
    return z if x == 0 or y == 0 else _fma(x, y, z)


def _round_even(x):
    return x if math.isinf(x) or math.isnan(x) else math.copysign(float(round(x)), x)


def _frexp_exponent(x):
    """Exposant e de x = m × 2^e, 0.5 ≤ |m| < 1 ; 0 pour ±0, ±infini et NaN."""
    return 0 if math.isinf(x) or math.isnan(x) else math.frexp(x)[1]


def _frexp_mantissa(bits):
    """Mantisse m de x = m × 2^e (signe conservé) ; ±0, ±infini et NaN rendus tels quels."""
    x = F(bits)
    return bits if x == 0 or math.isinf(x) or math.isnan(x) else float_bits(math.frexp(x)[0])


def _round_to_int(bits, rounding):
    """v_cvt_flr/rpi_i32_f32 mesurés sur RDNA 2 (gfx1036, VIctory #605) : `rounding` appliqué à la
    valeur exacte (rpi = floor(x + 0.5) sans arrondi intermédiaire), saturation, et un NaN donne
    0x7FFFFFFF ou 0x80000000 selon son bit de signe (au contraire de v_cvt_i32_f32, où NaN → 0)."""
    x = F(bits)
    if math.isnan(x):
        return INT32_MIN if bits >> 31 else INT32_MAX
    return _to_int(float(rounding(x)) if math.isfinite(x) else x, INT32_MIN, INT32_MAX)


def _sat_pk_u8_i16(a):
    """Les deux i16 de S0 saturés sur [0, 255], dans les octets 0 et 1 ; D[31:16] mis à 0."""
    low, high = (min(max(signed(a >> shift & 0xFFFF, 16), 0), BYTE_MAX) for shift in (0, 16))
    return high << 8 | low


def _leading_signs(value):
    """Position (depuis le haut) du premier bit différent du signe ; -1 pour 0 et -1."""
    value = ~value & MASK32 if value >> 31 else value
    return 32 - value.bit_length() if value else MASK32


VECTOR = {
    "v_mov_b32": lambda a, b, d, k, c: a,
    "v_readfirstlane_b32": lambda a, b, d, k, c: a,
    "v_not_b32": lambda a, b, d, k, c: ~a,
    "v_bfrev_b32": lambda a, b, d, k, c: int(f"{a & MASK32:032b}"[::-1], 2),
    "v_ffbl_b32": lambda a, b, d, k, c: (a & -a).bit_length() - 1 if a else MASK32,
    "v_ffbh_u32": lambda a, b, d, k, c: 32 - a.bit_length() if a else MASK32,
    "v_cvt_f32_i32": lambda a, b, d, k, c: float_bits(float(signed(a))),
    "v_cvt_f32_u32": lambda a, b, d, k, c: float_bits(float(a)),
    "v_cvt_u32_f32": lambda a, b, d, k, c: _to_int(F(a), 0, MASK32),
    "v_cvt_i32_f32": lambda a, b, d, k, c: _to_int(F(a), INT32_MIN, INT32_MAX),
    "v_cvt_f32_ubyte0": lambda a, b, d, k, c: float_bits(float(a & 0xFF)),
    "v_cvt_f32_ubyte1": lambda a, b, d, k, c: float_bits(float(a >> 8 & 0xFF)),
    "v_cvt_f32_ubyte2": lambda a, b, d, k, c: float_bits(float(a >> 16 & 0xFF)),
    "v_cvt_f32_ubyte3": lambda a, b, d, k, c: float_bits(float(a >> 24 & 0xFF)),
    "v_rcp_f32": _unary_float(_reciprocal),
    "v_rsq_f32": _unary_float(lambda x: _reciprocal(math.sqrt(x))),
    "v_sqrt_f32": _unary_float(math.sqrt),
    "v_exp_f32": _unary_float(_exp2),
    "v_log_f32": _unary_float(_log2),
    "v_sin_f32": _unary_float(lambda x: math.sin(x * 2 * math.pi)),
    "v_cos_f32": _unary_float(lambda x: math.cos(x * 2 * math.pi)),
    "v_floor_f32": _unary_float(_signed_round(math.floor)),
    "v_ceil_f32": _unary_float(_signed_round(math.ceil)),
    "v_trunc_f32": _unary_float(_signed_round(math.trunc)),
    "v_rndne_f32": _unary_float(_round_even),
    "v_fract_f32": _unary_float(_fract),
    "v_add_f32": _binary_float(lambda x, y: x + y),
    "v_sub_f32": _binary_float(lambda x, y: x - y),
    "v_subrev_f32": _binary_float(lambda x, y: y - x),
    "v_mul_f32": _binary_float(lambda x, y: x * y),
    "v_mul_legacy_f32": _binary_float(lambda x, y: 0.0 if x == 0.0 or y == 0.0 else x * y),
    "v_min_f32": _binary_float(min),
    "v_max_f32": _binary_float(max),
    "v_fmac_f32": lambda a, b, d, k, c: _float_op(_fma)(F(a), F(b), F(d)),
    "v_mac_f32": lambda a, b, d, k, c: _float_op(_mul_add)(F(a), F(b), F(d)),
    "v_fmamk_f32": lambda a, b, d, k, c: _float_op(_fma)(F(a), F(k), F(b)),
    "v_fmaak_f32": lambda a, b, d, k, c: _float_op(_fma)(F(a), F(b), F(k)),
    "v_cndmask_b32": lambda a, b, d, k, c: b if c else a,
    "v_add_nc_u32": lambda a, b, d, k, c: a + b,
    "v_sub_nc_u32": lambda a, b, d, k, c: a - b,
    "v_subrev_nc_u32": lambda a, b, d, k, c: b - a,
    "v_and_b32": lambda a, b, d, k, c: a & b,
    "v_or_b32": lambda a, b, d, k, c: a | b,
    "v_xor_b32": lambda a, b, d, k, c: a ^ b,
    "v_xnor_b32": lambda a, b, d, k, c: ~(a ^ b),
    "v_lshlrev_b32": lambda a, b, d, k, c: b << (a & 31),
    "v_lshrrev_b32": lambda a, b, d, k, c: b >> (a & 31),
    "v_ashrrev_i32": lambda a, b, d, k, c: signed(b) >> (a & 31),
    "v_min_u32": lambda a, b, d, k, c: min(a, b),
    "v_max_u32": lambda a, b, d, k, c: max(a, b),
    "v_min_i32": lambda a, b, d, k, c: min(signed(a), signed(b)),
    "v_max_i32": lambda a, b, d, k, c: max(signed(a), signed(b)),
    "v_mul_u32_u24": lambda a, b, d, k, c: (a & 0xFFFFFF) * (b & 0xFFFFFF),
    "v_mul_hi_u32_u24": lambda a, b, d, k, c: (a & 0xFFFFFF) * (b & 0xFFFFFF) >> 32,
    "v_mul_i32_i24": lambda a, b, d, k, c: signed(a, 24) * signed(b, 24),
    "v_mul_hi_i32_i24": lambda a, b, d, k, c: signed(a, 24) * signed(b, 24) >> 32,
    "v_cvt_flr_i32_f32": lambda a, b, d, k, c: _round_to_int(a, math.floor),
    "v_cvt_rpi_i32_f32": lambda a, b, d, k, c: _round_to_int(a, lambda x: math.floor(x + 0.5)),   # exact en double
    "v_cvt_off_f32_i4": lambda a, b, d, k, c: float_bits(signed(a, 4) / 16),
    "v_rcp_iflag_f32": _unary_float(_reciprocal),
    "v_ffbh_i32": lambda a, b, d, k, c: _leading_signs(a),
    "v_frexp_exp_i32_f32": lambda a, b, d, k, c: _frexp_exponent(F(a)),
    "v_frexp_mant_f32": lambda a, b, d, k, c: _frexp_mantissa(a),
    "v_fmac_legacy_f32": lambda a, b, d, k, c: _float_op(_legacy_fma)(F(a), F(b), F(d)),
    "v_madmk_f32": lambda a, b, d, k, c: _float_op(_mul_add)(F(a), F(k), F(b)),
    "v_madak_f32": lambda a, b, d, k, c: _float_op(_mul_add)(F(a), F(b), F(k)),
    "v_sat_pk_u8_i16": lambda a, b, d, k, c: _sat_pk_u8_i16(a),
}
# Résultat de 16 bits (spécification XML RDNA 2 : destination OperandSize 16). Mesuré sur RDNA 2
# (gfx1036, VIctory #499, 72 entrées, VOP1 et VOP3) : D[31:16] est mis à 0, contrairement à la règle
# générale de LLVM (GCNSubtarget::zeroesHigh16BitsOfDest). op_sel et clamp (VOP3) et SDWA, non
# mesurés, lèvent Untranslated.
SIXTEEN_BIT_RESULTS = frozenset({"v_sat_pk_u8_i16"})
BYTE_MAX = 0xFF
VECTOR_NO_OPERATION = frozenset({"v_nop", "v_pipeflush", "v_clrexcp"})

# --- Comparaisons vectorielles : (test, écrit EXEC au lieu de VCC) -----------------------

FLOAT_TESTS = {
    "f": lambda x, y: False, "lt": lambda x, y: x < y, "eq": lambda x, y: x == y, "le": lambda x, y: x <= y,
    "gt": lambda x, y: x > y, "lg": lambda x, y: x < y or x > y, "ge": lambda x, y: x >= y,
    "o": lambda x, y: not (math.isnan(x) or math.isnan(y)), "u": lambda x, y: math.isnan(x) or math.isnan(y),
    "nge": lambda x, y: not x >= y, "nlg": lambda x, y: not (x < y or x > y), "ngt": lambda x, y: not x > y,
    "nle": lambda x, y: not x <= y, "neq": lambda x, y: not x == y, "nlt": lambda x, y: not x < y,
    "tru": lambda x, y: True,
}
INTEGER_TESTS = {"f": lambda x, y: False, "lt": lambda x, y: x < y, "eq": lambda x, y: x == y,
                 "le": lambda x, y: x <= y, "gt": lambda x, y: x > y, "ne": lambda x, y: x != y,
                 "ge": lambda x, y: x >= y, "t": lambda x, y: True}

VECTOR_COMPARE = {}
for _prefix, _writes_exec in (("v_cmp", False), ("v_cmpx", True)):
    for _name, _test in FLOAT_TESTS.items():
        VECTOR_COMPARE[f"{_prefix}_{_name}_f32"] = (lambda a, b, t=_test: t(F(a), F(b)), _writes_exec)
    for _name, _test in INTEGER_TESTS.items():
        VECTOR_COMPARE[f"{_prefix}_{_name}_u32"] = (lambda a, b, t=_test: t(a, b), _writes_exec)
        VECTOR_COMPARE[f"{_prefix}_{_name}_i32"] = (lambda a, b, t=_test: t(signed(a), signed(b)), _writes_exec)
VECTOR_COMPARE.update(rdna2_exec_cmp16.COMPARE16)       # i16/u16: S0[15:0] vs S1[15:0]
