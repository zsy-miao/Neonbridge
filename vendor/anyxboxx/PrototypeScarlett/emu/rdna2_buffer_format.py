"""Accès mémoire typés RDNA 2 (buffer_*_format, MUBUF ; tbuffer_*_format, MTBUF) : formats et conversions.

Partagé par l'interpréteur (emu/rdna2_exec_format.py) et le HLSL (emu/rdna2_hlsl_format.py).

Format : champ FORMAT (bits 18-12) du mot 3 du V# pour MUBUF, champ FORMAT (bits 25-19) de
l'instruction pour MTBUF ; valeurs du format unifié gfx10 (data/gfx10_formats.json, Mesa
src/amd/registers/gfx10-rsrc.json, enum GFX10_FORMAT ; mêmes valeurs que UfmtGFX10 de LLVM,
SIDefines.h). Mot 3 du V# : DST_SEL_X/Y/Z/W en bits 2-0, 5-3, 8-6, 11-9, ADD_TID_ENABLE en bit 23
(Mesa gfx10-rsrc.json, SQ_BUF_RSRC_WORD3) ; SWIZZLE_ENABLE en bit 31 du mot 1 (SQ_BUF_RSRC_WORD1).

Les largeurs du nom se lisent du bit de poids fort vers le bit 0 : 2_10_10_10 est x sur les bits
9-0 et w sur 31-30, 10_11_11 est x sur 10-0 et z sur 31-22 (Mesa src/amd/common/ac_formats.c,
ac_translate_buffer_dataformat : R10G10B10A2 → 2_10_10_10, R11G11B10_FLOAT → 10_11_11).

Conversions en lecture (valeur 32 bits écrite dans le VGPR) :
- UNORM sur n bits : c / (2^n - 1) ; SNORM : max(c / (2^(n-1) - 1), -1), donc -2^(n-1) donne -1
  (spécification Vulkan, « Fixed-Point Data Conversions »). Arrondi au plus proche du quotient
  exact (mesuré sur Radeon RX 6700 par Leonida, third_party/leonida/rdna2spirv.py, decode_format) ;
  le quotient n'est jamais à mi-chemin de deux float32 : son écriture binaire se répète sans fin.
- USCALED, SSCALED : l'entier converti en float32, exact (16 bits au plus) ; UINT : étendu par des
  zéros ; SINT : étendu par le signe (Leonida, mesuré sur gfx1036).
- FLOAT 32 bits : les bits tels quels ; FLOAT 16 bits : élargissement exact en float32 (dénormaux
  compris) ; un NaN garde signe et charge utile décalée, sans bit « quiet » ajouté (Leonida, mesuré
  sur gfx1036, formats 13, 29 et 71).
Écriture : seulement les formats UINT et SINT (sur 8, 16, 10, 11, 2 et 32 bits) et FLOAT 32 bits.
UINT sature la valeur 32 bits à 2^n - 1, SINT la borne à [-2^(n-1), 2^(n-1) - 1] (Leonida, mesuré
sur gfx1036). Les écritures UNORM, SNORM, SCALED et FLOAT 16 bits (arrondi non vérifié ici) et
les formats flottants 11/10 bits (10_11_11_FLOAT, 11_11_10_FLOAT) sont refusés en les nommant.
"""
import re
from typing import NamedTuple

from emu import rdna2_image
from emu.rdna2_exec_ops import MASK32, float_bits, signed

UNORM, SNORM, USCALED, SSCALED, UINT, SINT, FLOAT = range(1, 8)
KIND_NAMES = {"UNORM": UNORM, "SNORM": SNORM, "USCALED": USCALED, "SSCALED": SSCALED,
              "UINT": UINT, "SINT": SINT, "FLOAT": FLOAT}
INTEGER_KINDS = frozenset({UINT, SINT})
FORMAT_CODES = 128                              # champ de 7 bits
NAME_PATTERN = re.compile(r"^GFX10_FORMAT_((?:\d+_)+)([A-Z]+)$")
ONE_FLOAT, ONE_INTEGER = 0x3F800000, 1
MINUS_ONE = 0xBF800000
SIGN_BIT = 0x80000000
SELECT_ZERO, SELECT_ONE, SELECT_X = 0, 1, 4
SELECT_BITS, SELECT_MASK = 3, 0x7
WORD3_FORMAT_SHIFT, FORMAT_MASK = 12, 0x7F
ADD_TID_BIT, SWIZZLE_ENABLE_BIT = 23, 31
DWORD_BITS = 32

# Champs de l'instruction (MUBUFe_gfx10 et MTBUFe_gfx10 de LLVM, BUFInstructions.td).
OFFSET_MASK, OFFEN_BIT, IDXEN_BIT, MUBUF_LDS_BIT = 0xFFF, 12, 13, 16
MTBUF_FORMAT_SHIFT, TFE_BIT = 19, 55
PATTERN = re.compile(r"^(t?)buffer_(load|store)_format_(x|xy|xyz|xyzw)$")


class FormatError(ValueError):
    """Accès typé hors du cas pris en charge (le message le nomme)."""


class Format(NamedTuple):
    code: int
    name: str
    kind: int
    widths: tuple       # largeurs des composantes, à partir du bit 0
    size: int           # octets d'un élément
    align: int          # alignement exigé de l'adresse (octets)

    @property
    def count(self):
        return len(self.widths)

    @property
    def storable(self):
        return self.kind in INTEGER_KINDS or (self.kind == FLOAT and set(self.widths) == {DWORD_BITS})


def _parse(code, full_name):
    found = NAME_PATTERN.match(full_name)
    if found is None or found.group(2) not in KIND_NAMES:
        return None
    widths = tuple(int(width) for width in found.group(1).rstrip("_").split("_"))[::-1]
    kind = KIND_NAMES[found.group(2)]
    if kind == FLOAT and set(widths) - {16, DWORD_BITS}:
        return None                             # flottants 11/10 bits : non pris en charge
    uniform = len(set(widths)) == 1 and widths[0] % 8 == 0
    return Format(code, full_name.removeprefix("GFX10_FORMAT_"), kind, widths, sum(widths) // 8,
                  widths[0] // 8 if uniform else 4)


FORMATS = {code: found for code, name in rdna2_image.FORMAT_NAMES.items() if code < FORMAT_CODES
           for found in [_parse(code, name)] if found is not None}


def format_for(code, store):
    """Format pris en charge (FormatError sinon), pour une lecture ou une écriture."""
    found = FORMATS.get(code)
    if found is None:
        raise FormatError(f"format {rdna2_image.FORMAT_NAMES.get(code, code)} non pris en charge")
    if store and not found.storable:
        raise FormatError(f"écriture au format {found.name} non prise en charge (conversion non vérifiée)")
    return found


# --- Instruction -------------------------------------------------------------------------

class Access(NamedTuple):
    typed: bool         # MTBUF : format dans l'instruction
    store: bool
    count: int          # composantes lues ou écrites (x = 1 … xyzw = 4)
    offset: int
    offen: int
    idxen: int
    vaddr: int
    vdata: int
    srsrc: int          # premier SGPR du V#
    soffset: int
    fixed: int          # format de l'instruction (MTBUF), sinon None


def supports(name):
    return PATTERN.match(name) is not None


def access(name, word):
    """Champs d'un accès typé (`word` : 64 bits) ; FormatError pour les options non prises en charge."""
    found = PATTERN.match(name)
    typed = bool(found.group(1))
    if not typed and word >> MUBUF_LDS_BIT & 1:
        raise FormatError(f"{name} avec lds non pris en charge")
    if word >> TFE_BIT & 1:
        raise FormatError(f"{name} avec tfe non pris en charge")
    fixed = (word >> MTBUF_FORMAT_SHIFT) & FORMAT_MASK if typed else None
    return Access(typed, found.group(2) == "store", len(found.group(3)), word & OFFSET_MASK,
                  word >> OFFEN_BIT & 1, word >> IDXEN_BIT & 1, (word >> 32) & 0xFF, (word >> 40) & 0xFF,
                  ((word >> 48) & 0x1F) * 4, (word >> 56) & 0xFF, fixed)


# --- Descripteur V# ----------------------------------------------------------------------

def selects(word3):
    return [(word3 >> (SELECT_BITS * i)) & SELECT_MASK for i in range(4)]


def check_descriptor(words):
    if words[1] >> SWIZZLE_ENABLE_BIT & 1 or words[3] >> ADD_TID_BIT & 1:
        raise FormatError("V# avec SWIZZLE_ENABLE ou ADD_TID_ENABLE non pris en charge")


def check_load_selects(fmt, word3, count):
    """MUBUF : DST_SEL 0, 1 ou une composante du format (X à W), pour chaque composante lue."""
    for component, select in enumerate(selects(word3)[:count]):
        if select in (SELECT_ZERO, SELECT_ONE):
            continue
        if select < SELECT_X or select - SELECT_X >= fmt.count:
            raise FormatError(f"DST_SEL {select} de la composante {component} non pris en charge "
                              f"avec le format {fmt.name}")


def store_selects_ok(fmt, word3):
    """MUBUF : écriture acceptée si DST_SEL nomme X, Y, Z, W dans l'ordre pour les composantes du
    format, avec DST_SEL_Z constant (0 ou 1) pour un format à deux composantes : Leonida mesure sur
    gfx1036 que DST_SEL_Z = Z ou W y change les composantes écrites (XYZW écrit x et w)."""
    found = selects(word3)
    if any(found[i] != SELECT_X + i for i in range(fmt.count)):
        return False
    return fmt.count != 2 or found[2] in (SELECT_ZERO, SELECT_ONE)


# --- Conversions -------------------------------------------------------------------------

def ratio_bits(numerator, denominator):
    """float32 (bits) le plus proche de numerator / denominator, 0 <= numerator <= denominator."""
    if numerator == 0:
        return 0
    shift = 0
    while numerator << shift < denominator << 23:
        shift += 1
    mantissa, remainder = divmod(numerator << shift, denominator)
    if 2 * remainder > denominator or (2 * remainder == denominator and mantissa & 1):
        mantissa += 1
    if mantissa >> 24:
        mantissa, shift = mantissa >> 1, shift - 1
    return (150 - shift) << 23 | (mantissa & 0x7FFFFF)


def unorm_bits(value, width):
    return ratio_bits(value, (1 << width) - 1)


def snorm_bits(value, width):
    number = signed(value, width)
    if number <= -((1 << (width - 1)) - 1):
        return MINUS_ONE
    magnitude = ratio_bits(abs(number), (1 << (width - 1)) - 1)
    return magnitude | SIGN_BIT if number < 0 else magnitude


def half_bits(half):
    """float16 → float32, exact ; infini et NaN gardent leur charge utile (décalée de 13 bits)."""
    sign, exponent, mantissa = (half & 0x8000) << 16, (half >> 10) & 0x1F, half & 0x3FF
    if exponent == 0x1F:
        return sign | 0x7F800000 | mantissa << 13
    if exponent:
        return sign | (exponent + 112) << 23 | mantissa << 13
    if mantissa == 0:
        return sign
    top = mantissa.bit_length() - 1
    return sign | (top + 103) << 23 | (mantissa << (23 - top)) & 0x7FFFFF


def _component(fmt, field, width):
    if fmt.kind == UNORM:
        return unorm_bits(field, width)
    if fmt.kind == SNORM:
        return snorm_bits(field, width)
    if fmt.kind == USCALED:
        return float_bits(float(field))           # exact : 16 bits au plus
    if fmt.kind == SSCALED:
        return float_bits(float(signed(field, width)))
    if fmt.kind == SINT:
        return signed(field, width) & MASK32
    if fmt.kind == FLOAT and width == 16:
        return half_bits(field)
    return field                                # UINT, FLOAT 32 bits


def decode(fmt, data):
    """Composantes (valeurs 32 bits) d'un élément de `fmt.size` octets."""
    packed, position, values = int.from_bytes(data, "little"), 0, []
    for width in fmt.widths:
        values.append(_component(fmt, (packed >> position) & ((1 << width) - 1), width))
        position += width
    return values


def _field(fmt, value, width):
    if width == DWORD_BITS:
        return value
    if fmt.kind == UINT:
        return min(value, (1 << width) - 1)
    low, high = -(1 << (width - 1)), (1 << (width - 1)) - 1
    return min(max(signed(value), low), high) & ((1 << width) - 1)


def encode(fmt, values):
    """Octets d'un élément à partir des valeurs 32 bits des VGPR (format inscriptible)."""
    packed, position = 0, 0
    for value, width in zip(values, fmt.widths):
        packed |= _field(fmt, value, width) << position
        position += width
    return packed.to_bytes(fmt.size, "little")


def select_one(fmt):
    """Valeur de DST_SEL = 1 : 1.0 pour les formats flottants, normalisés et SCALED, 1 sinon (Leonida,
    mesuré sur gfx1036 pour les 128 valeurs de FORMAT)."""
    return ONE_INTEGER if fmt.kind in INTEGER_KINDS else ONE_FLOAT
