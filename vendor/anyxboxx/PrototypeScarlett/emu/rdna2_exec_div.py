"""Division IEEE de RDNA 2 dans l'interpréteur : v_div_scale_f32/f64, v_div_fmas_f32/f64, v_div_fixup_f32/f64.

Les compilateurs (LLVM : SIISelLowering, lowerFDIV32/FDIV64) divisent n / d en quatre temps :
v_div_scale met d et n à l'échelle (et pose VCC s'il faudra corriger le quotient), v_rcp et des
FMA de Newton-Raphson calculent le quotient, v_div_fmas fait la dernière FMA et défait l'échelle
quand VCC est posé, v_div_fixup traite les cas spéciaux (0, infini, NaN).

Sources : pseudo-code de la documentation publique d'AMD (« RDNA 3 Instruction Set Architecture »,
le PDF RDNA 2 n'est pas lisible par machine ; descriptions identiques dans la spécification XML
RDNA 2), recoupé par deux mesures publiques sur matériel : la documentation de CLRX (GCN) et les
tests matériel de tinygrad (RDNA 3, test/amd/hw/test_vop3.py). Choix supposés, faute de mesure
RDNA 2 :
- v_div_fmas : facteur 2^64 si |S2| ≥ 2, sinon 2^-64 (2^±128 en f64), mesuré par CLRX et tinygrad ;
  le pseudo-code d'AMD écrit 2^32 (2^64 en f64), incohérent avec l'échelle 2^64 de v_div_scale.
  Le facteur est appliqué avant l'unique arrondi (sinon un quotient dénormal serait arrondi deux fois) ;
- v_div_scale, 0 en S1 ou S2 : NaN et VCC = 0 (pseudo-code ; tinygrad mesure VCC = 1 sur RDNA 3,
  sans effet sur la division : le NaN traverse v_div_fmas et v_div_fixup traite 0) ;
- v_div_scale, branche « 1/S1 dénormal » : en f32 elle compare à un dénormal f64 et n'est jamais
  prise (lecture littérale) ; en f64, la branche combinée multiplie un dénominateur déjà proche du
  maximum par 2^128 (dépassement) : la branche entière (|S1| > 2^1022, fini) lève Untranslated, la
  branche seule (S0 × 2^-128) comprise. v_div_scale_f64 (VOP3 0x16E) suit sinon le code f32 avec les
  constantes f64 du pseudo-code (écart 768, échelle 2^128, numérateur minuscule si exposant ≤ 53) ;
- v_div_fixup : S0 NaN donne ±infini (CLRX et tinygrad ; absent du pseudo-code) ; UNDERFLOW = ±0,
  seule valeur cohérente avec l'arrondi au plus près sous la condition exposant(S2) - exposant(S1) < -150.
Les dénormaux sont conservés (mode IEEE, celui où LLVM émet cette séquence) ; clamp et omod lèvent
Untranslated.
"""
import math
import struct
from dataclasses import dataclass
from fractions import Fraction

from emu import rdna2_exec_cmp64
from emu.rdna2_exec_wide import _modify

LITERAL = 255


@dataclass(frozen=True)
class Format:
    kind: str
    width: int
    mantissa: int          # bits de mantisse stockés
    bias: int
    scale: int             # exposant de l'échelle de v_div_scale et v_div_fmas
    near_max: int          # écart d'exposants de la branche « n/d proche du maximum »
    tiny: int              # exposant (biaisé) maximal d'un numérateur « minuscule »
    pack: str

    @property
    def max_exponent(self):
        return (1 << (self.width - 1 - self.mantissa)) - 1

    @property
    def sign(self):
        return 1 << (self.width - 1)

    @property
    def quiet(self):
        return 1 << (self.mantissa - 1)

    @property
    def negative_nan(self):
        return self.sign | self.max_exponent << self.mantissa | self.quiet

    @property
    def nan(self):
        return self.max_exponent << self.mantissa | self.quiet

    @property
    def infinity(self):
        return self.max_exponent << self.mantissa


F32 = Format("f32", 32, 23, 127, 64, 96, 23, "<f")
F64 = Format("f64", 64, 52, 1023, 128, 768, 53, "<d")
OPERATIONS = {"v_div_scale_f32": F32, "v_div_scale_f64": F64, "v_div_fmas_f32": F32, "v_div_fmas_f64": F64,
              "v_div_fixup_f32": F32, "v_div_fixup_f64": F64}
INTEGER = {32: "<I", 64: "<Q"}


def _untranslated(message):
    from emu.rdna2_exec import Untranslated

    return Untranslated(message)


def value(bits, fmt):
    return struct.unpack(fmt.pack, struct.pack(INTEGER[fmt.width], bits))[0]


def exponent(bits, fmt):
    """Champ d'exposant biaisé (0 pour ±0 et les dénormaux, maximum pour infini et NaN)."""
    return bits >> fmt.mantissa & fmt.max_exponent


def is_nan(bits, fmt):
    return exponent(bits, fmt) == fmt.max_exponent and bits & ((1 << fmt.mantissa) - 1) != 0


def is_denormal(bits, fmt):
    return exponent(bits, fmt) == 0 and bits & ((1 << fmt.mantissa) - 1) != 0


def round_exact(exact, fmt, negative_zero=False):
    """Bits du nombre exact `exact` (Fraction) arrondi au plus près pair, dénormaux et ±infini compris."""
    if exact == 0:
        return fmt.sign if negative_zero else 0
    sign = fmt.sign if exact < 0 else 0
    magnitude = abs(exact)
    power = magnitude.numerator.bit_length() - magnitude.denominator.bit_length()
    if Fraction(2) ** power > magnitude:
        power -= 1
    power = max(power, 1 - fmt.bias)                       # en dessous : pas des dénormaux
    shift = fmt.mantissa - power
    scaled = magnitude * Fraction(2) ** shift
    integer, remainder = divmod(scaled.numerator, scaled.denominator)
    if 2 * remainder > scaled.denominator or (2 * remainder == scaled.denominator and integer & 1):
        integer += 1
    if integer >> (fmt.mantissa + 1):                      # arrondi jusqu'à la puissance de 2 suivante
        integer >>= 1
        power += 1
    if power > fmt.bias:
        return sign | fmt.infinity
    if integer >> fmt.mantissa == 0:                       # dénormal
        return sign | integer
    return sign | (power + fmt.bias) << fmt.mantissa | (integer - (1 << fmt.mantissa))


def _scaled(bits, power, fmt):
    """ldexp(S0, power) : NaN et infini inchangés, arrondi exact sinon."""
    if exponent(bits, fmt) == fmt.max_exponent:
        return bits
    number = value(bits, fmt)
    return round_exact(Fraction(number) * Fraction(2) ** power, fmt, negative_zero=bool(bits & fmt.sign))


def _quotient_is_denormal(numerator, denominator, fmt):
    """S2 / S1 arrondi est un dénormal non nul (S1, S2 finis et non nuls)."""
    quotient = round_exact(Fraction(value(numerator, fmt)) / Fraction(value(denominator, fmt)), fmt)
    return is_denormal(quotient & ~fmt.sign, fmt)


def div_scale(s0, s1, s2, fmt):
    """(D, bit de VCC) de v_div_scale : S0 à mettre à l'échelle, S1 dénominateur, S2 numérateur."""
    x0, x1, x2 = (value(bits, fmt) for bits in (s0, s1, s2))
    if x2 == 0 or x1 == 0:
        return fmt.nan, 0
    if exponent(s2, fmt) - exponent(s1, fmt) >= fmt.near_max:        # n / d proche du maximum
        return (_scaled(s0, fmt.scale, fmt) if x0 == x1 else s0), 1
    if is_denormal(s1, fmt):
        return _scaled(s0, fmt.scale, fmt), 0
    if fmt.width == 64 and math.isfinite(x1) and abs(x1) > 2.0 ** (fmt.bias - 1):
        raise _untranslated("v_div_scale_f64 : 1/S1 dénormal, pseudo-code d'AMD incohérent")
    finite = math.isfinite(x1) and math.isfinite(x2)
    if finite and _quotient_is_denormal(s2, s1, fmt):
        return (_scaled(s0, fmt.scale, fmt) if x0 == x2 else s0), 1
    if exponent(s2, fmt) <= fmt.tiny:                                 # numérateur minuscule
        return _scaled(s0, fmt.scale, fmt), 0
    return s0, 0


def div_fmas(s0, s1, s2, vcc, fmt):
    """S0 × S1 + S2 arrondi une fois, multiplié par 2^±scale (signe selon |S2| ≥ 2) si VCC."""
    x0, x1, x2 = (value(bits, fmt) for bits in (s0, s1, s2))
    power = (fmt.scale if abs(x2) >= 2.0 else -fmt.scale) if vcc else 0
    if not all(math.isfinite(x) for x in (x0, x1, x2)):         # NaN ou ±infini, l'échelle n'y change rien
        special = x0 * x1 + x2
        return fmt.nan if math.isnan(special) else (fmt.sign if special < 0 else 0) | fmt.infinity
    exact = Fraction(x0) * Fraction(x1) + Fraction(x2)
    return round_exact(exact * Fraction(2) ** power, fmt, negative_zero=math.copysign(1.0, x0 * x1 + x2) < 0)


def div_fixup(s0, s1, s2, fmt):
    """Quotient S0 corrigé selon le dénominateur S1 et le numérateur S2 (cas spéciaux de la division)."""
    sign = (s1 ^ s2) & fmt.sign
    magnitude = ~fmt.sign & ((1 << fmt.width) - 1)
    x1, x2 = value(s1, fmt), value(s2, fmt)
    if is_nan(s2, fmt):
        return s2 | fmt.quiet
    if is_nan(s1, fmt):
        return s1 | fmt.quiet
    if x1 == 0 and x2 == 0 or math.isinf(x1) and math.isinf(x2):
        return fmt.negative_nan
    if x1 == 0 or math.isinf(x2):
        return sign | fmt.infinity
    if math.isinf(x1) or x2 == 0:
        return sign
    if exponent(s2, fmt) - exponent(s1, fmt) < -(fmt.bias + fmt.mantissa):   # -150 / -1075
        return sign
    if is_nan(s0, fmt):
        return sign | fmt.infinity
    return sign | (s0 & magnitude)


def _read(wave, code, fmt, lane, literal):
    if fmt.width == 32:
        return wave.read_lane(code, lane, literal)
    if code == LITERAL:
        raise _untranslated("littéral sur une source 64 bits")
    return rdna2_exec_cmp64.read64(wave, code, lane, True)


def execute(wave, name, fields, literal):
    """Exécute une instruction de OPERATIONS (encodage VOP3, `fields` : emu.rdna2_exec_vop3.Fields)."""
    fmt = OPERATIONS[name]
    if fields.clamp or fields.omod:
        raise _untranslated(f"{name} avec clamp/omod")
    is_scale = name.startswith("v_div_scale")
    abs_bits = 0 if is_scale else fields.abs                  # VOP3b : le champ abs porte SDST
    vcc_mask = 0
    for lane in wave.active():
        s0, s1, s2 = (_modify(_read(wave, code, fmt, lane, literal), fmt.kind, index, abs_bits, fields.neg)
                      for index, code in enumerate(fields.sources))
        if is_scale:
            result, bit = div_scale(s0, s1, s2, fmt)
            vcc_mask |= bit << lane
        elif name.startswith("v_div_fmas"):
            result = div_fmas(s0, s1, s2, wave.vcc >> lane & 1, fmt)
        else:
            result = div_fixup(s0, s1, s2, fmt)
        wave.v[fields.vdst][lane] = result & 0xFFFF_FFFF
        if fmt.width == 64:
            wave.v[fields.vdst + 1][lane] = result >> 32
    if is_scale:
        wave.write_scalar(fields.sdst, vcc_mask)              # lignes inactives à 0, comme les retenues
