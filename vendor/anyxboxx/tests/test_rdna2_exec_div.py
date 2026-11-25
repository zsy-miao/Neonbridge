"""Division IEEE : v_div_scale_f32, v_div_fmas_f32/f64, v_div_fixup_f32/f64.

Valeurs attendues calculées à la main d'après le pseudo-code d'AMD (« RDNA 3 Instruction Set
Architecture », mêmes descriptions dans la spécification XML RDNA 2) ; les choix supposés sont
listés dans emu/rdna2_exec_div.py. Le test GPU vérifie le HLSL contre l'interpréteur.
"""
import struct

import pytest

from emu import rdna2_exec, rdna2_gpu, rdna2_hlsl
from test_rdna2_exec import V, program, sop1, vop1, vop2, vop3
from test_rdna2_exec_memory import INPUT, global_
from test_rdna2_hlsl import compare, store_lane, with_lane_offset

LITERAL, VCC, EXEC_LO, NULL = 255, 106, 126, 125
ONE, TWO = 242, 244
gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc ou anyxboxx_gpu_run.exe absent")


def mov(vdst, bits):
    return vop1("v_mov_b32", vdst, LITERAL), bits & 0xFFFF_FFFF


def scale(numerator, denominator):
    """(échelle du dénominateur, échelle du numérateur, VCC du numérateur, VCC du dénominateur)."""
    wave = rdna2_exec.run(program(
        *mov(1, numerator), *mov(2, denominator),
        *vop3("v_div_scale_f32", 3, V + 2, V + 2, V + 1, sdst=10),
        *vop3("v_div_scale_f32", 4, V + 1, V + 2, V + 1, sdst=VCC)))
    return wave.v[3][0], wave.v[4][0], wave.vcc & 1, wave.s[10] & 1


@pytest.mark.parametrize("numerator, denominator, expected", [
    (0x40C0_0000, 0x4040_0000, (0x4040_0000, 0x40C0_0000, 0, 0)),     # 6 / 3 : rien à faire
    # 2^127 / 2^30 : écart d'exposants 254 - 157 ≥ 96, d × 2^64 = 2^94 (champ 221), n inchangé
    (0x7F00_0000, 0x4E80_0000, (0x6E80_0000, 0x7F00_0000, 1, 1)),
    # 2^-40 / 2^-149 (dénormal) : écart 87 < 96, S1 dénormal : les deux × 2^64 (2^-85 et 2^24)
    (0x2B80_0000, 0x0000_0001, (0x1500_0000, 0x4B80_0000, 0, 0)),
    # 1 / 2^127 : quotient 2^-127 dénormal, VCC ; seul le numérateur × 2^64 (champ 191)
    (0x3F80_0000, 0x7F00_0000, (0x7F00_0000, 0x5F80_0000, 1, 1)),
    # 2^-120 / 1 : numérateur minuscule (champ 7 ≤ 23), les deux × 2^64, VCC = 0
    (0x0380_0000, 0x3F80_0000, (0x5F80_0000, 0x2380_0000, 0, 0)),
    # max dénormal / 2 = (2^24 - 1) × 2^-150 : milieu, arrondi à 2^-126 (normal) ; numérateur minuscule
    (0x00FF_FFFF, 0x4000_0000, (0x6000_0000, 0x20FF_FFFF, 0, 0)),
    # (2^24 - 2) × 2^-150 = plus grand dénormal, exact : VCC, numérateur × 2^64
    (0x00FF_FFFE, 0x4000_0000, (0x4000_0000, 0x20FF_FFFE, 1, 1)),
    # 2^-149 / 2 = 2^-150 : milieu arrondi à 0 (pair), pas un dénormal ; numérateur minuscule
    (0x0000_0001, 0x4000_0000, (0x6000_0000, 0x1500_0000, 0, 0)),
    # 0 / 2 : NaN, VCC = 0 (pseudo-code d'AMD)
    (0x0000_0000, 0x4000_0000, (0x7FC0_0000, 0x7FC0_0000, 0, 0)),
])
def test_div_scale_branches(numerator, denominator, expected):
    assert scale(numerator, denominator) == expected


def fmas(a, b, c, vcc):
    wave = rdna2_exec.run(program(
        *mov(1, a), *mov(2, b), *mov(3, c), sop1("s_mov_b32", VCC, LITERAL), vcc,
        *vop3("v_div_fmas_f32", 4, V + 1, V + 2, V + 3)))
    return wave.v[4][0]


@pytest.mark.parametrize("a, b, c, vcc, expected", [
    (0x4000_0000, 0x4040_0000, 0x3F80_0000, 0, 0x40E0_0000),         # 2 × 3 + 1 = 7
    (0x3F80_0000, 0x3F80_0000, 0x4000_0000, 1, 0x6040_0000),         # (1 + 2) × 2^64 (|S2| ≥ 2) = 1.5 × 2^65
    (0x4000_0000, 0x4040_0000, 0x3F80_0000, 1, 0x20E0_0000),         # 7 × 2^-64 (|S2| < 2) = 1.75 × 2^-62
    # (2^-70 + 2^-86) × 2^-64 = 2^-134 + 2^-150 : milieu de deux dénormaux, pair → 2^15 × 2^-149
    (0x3F80_0000, 0x1480_0000, 0x1C80_0000, 1, 0x0000_8000),
    # produit (1 + 2^-23) × 2^-86 : 2^-109 de plus, au-delà du milieu → 2^15 + 1. Un arrondi
    # f32 avant l'échelle (ulp 2^-93 à 2^-70) perdrait ce 2^-109 et donnerait 0x8000.
    (0x3F80_0001, 0x1480_0000, 0x1C80_0000, 1, 0x0000_8001),
    # (1 + 2^-23) × 2^-24 × (1 - 2^-23) + (1 + 2^-23) = 1 + 2^-23 + 2^-24 - 2^-70 : sous le milieu,
    # → 1 + 2^-23. Arrondi d'abord à 53 bits, on tomberait sur le milieu, puis sur 1 + 2^-22 (pair).
    (0x3380_0001, 0x3F7F_FFFE, 0x3F80_0001, 0, 0x3F80_0001),
])
def test_div_fmas_scales_before_rounding_once(a, b, c, vcc, expected):
    assert fmas(a, b, c, vcc) == expected


def fixup(quotient, denominator, numerator, abs_=0, neg=0):
    wave = rdna2_exec.run(program(
        *mov(1, quotient), *mov(2, denominator), *mov(3, numerator),
        *vop3("v_div_fixup_f32", 4, V + 1, V + 2, V + 3, abs_=abs_, neg=neg)))
    return wave.v[4][0]


@pytest.mark.parametrize("quotient, denominator, numerator, expected", [
    (0x4040_0000, 0xC000_0000, 0x40C0_0000, 0xC040_0000),            # 6 / -2 : signe des sources, |q| = 3
    (0x3F80_0000, 0x0000_0000, 0xBF80_0000, 0xFF80_0000),            # -1 / +0 = -infini
    (0x3F80_0000, 0x0000_0000, 0x8000_0000, 0xFFC0_0000),            # 0 / 0 : NaN négatif
    (0x3F80_0000, 0x7F80_0000, 0xFF80_0000, 0xFFC0_0000),            # infini / infini
    (0x3F80_0000, 0xFF80_0000, 0x3F80_0000, 0x8000_0000),            # 1 / -infini = -0
    (0x3F80_0000, 0x3F80_0000, 0x7F80_0001, 0x7FC0_0001),            # NaN signalant rendu calme
    (0x3F80_0000, 0xFF80_0005, 0x3F80_0000, 0xFFC0_0005),            # NaN du dénominateur
    (0x7FC0_0000, 0xBF80_0000, 0x3F80_0000, 0xFF80_0000),            # quotient NaN : ±infini
    (0x3F80_0000, 0x5D80_0000, 0x0D80_0000, 0x0000_0000),            # 2^-100 / 2^60 : 27 - 187 < -150 → 0
])
def test_div_fixup_special_cases(quotient, denominator, numerator, expected):
    assert fixup(quotient, denominator, numerator) == expected


def test_div_fixup_source_modifiers():
    # |q| puis -d : q = -3, d = 2 → -2 ; n = 6 → signe négatif, |q| = 3
    assert fixup(0xC040_0000, 0x4000_0000, 0x40C0_0000, abs_=0b001, neg=0b010) == 0xC040_0000


def mov64(vdst, bits):
    return (*mov(vdst, bits), *mov(vdst + 1, bits >> 32))


def test_double_fmas_and_fixup():
    wave = rdna2_exec.run(program(
        *mov64(2, 0x4008_0000_0000_0000), *mov64(4, 0xC000_0000_0000_0000),        # 3.0, -2.0
        *mov64(6, 0x4018_0000_0000_0000), *mov64(8, 0x7FF0_0000_0000_0001),        # 6.0, NaN signalant
        sop1("s_mov_b32", VCC, LITERAL), 0,
        *vop3("v_div_fmas_f64", 10, TWO, V + 2, ONE),                              # 2 × 3 + 1 = 7
        sop1("s_mov_b32", VCC, LITERAL), 1,
        *vop3("v_div_fmas_f64", 12, ONE, ONE, TWO),                                # 3 × 2^128
        *vop3("v_div_fmas_f64", 14, TWO, V + 2, ONE),                              # 7 × 2^-128
        *vop3("v_div_fixup_f64", 16, V + 2, V + 4, V + 6),                         # 6 / -2 → -3
        *vop3("v_div_fixup_f64", 18, V + 2, 128, 128),                             # 0 / 0
        *vop3("v_div_fixup_f64", 20, V + 2, ONE, V + 8)))
    pairs = [(wave.v[r][0] | wave.v[r + 1][0] << 32) for r in range(10, 22, 2)]
    assert pairs == [0x401C_0000_0000_0000,                 # 7.0
                     0x4808_0000_0000_0000,                 # 1.5 × 2^129 : champ 1023 + 129 = 0x480
                     0x381C_0000_0000_0000,                 # 1.75 × 2^-126 : champ 897 = 0x381
                     0xC008_0000_0000_0000, 0xFFF8_0000_0000_0000, 0x7FF8_0000_0000_0001]


def division(numerator, denominator):
    """Séquence de LLVM (lowerFDIV32) pour numérateur / dénominateur."""
    wave = rdna2_exec.run(program(
        *mov(1, numerator), *mov(2, denominator),
        *vop3("v_div_scale_f32", 3, V + 2, V + 2, V + 1, sdst=10),
        *vop3("v_div_scale_f32", 4, V + 1, V + 2, V + 1, sdst=VCC),
        vop1("v_rcp_f32", 5, V + 3),
        *vop3("v_fma_f32", 6, V + 3, V + 5, ONE, neg=0b001),
        *vop3("v_fma_f32", 5, V + 6, V + 5, V + 5),
        vop2("v_mul_f32", 6, V + 4, 5),
        *vop3("v_fma_f32", 7, V + 3, V + 6, V + 4, neg=0b001),
        *vop3("v_fma_f32", 6, V + 7, V + 5, V + 6),
        *vop3("v_fma_f32", 7, V + 3, V + 6, V + 4, neg=0b001),
        *vop3("v_div_fmas_f32", 6, V + 7, V + 5, V + 6),
        *vop3("v_div_fixup_f32", 8, V + 6, V + 2, V + 1)))
    return wave.v[8][0]


@pytest.mark.parametrize("numerator, denominator, expected", [
    (0x3F80_0000, 0x4040_0000, 0x3EAA_AAAB),        # 1/3 arrondi au plus près
    (0x3F80_0000, 0x7F00_0000, 0x0040_0000),        # 1 / 2^127 = 2^-127, dénormal exact
    (0x7F40_0000, 0x4E80_0000, 0x7040_0000),        # 1.5 × 2^127 / 2^30 = 1.5 × 2^97 (champ 224)
    (0x40C0_0000, 0x0000_0000, 0x7F80_0000),        # 6 / 0 = +infini
])
def test_llvm_division_sequence(numerator, denominator, expected):
    assert division(numerator, denominator) == expected


def test_clamp_is_named():
    with pytest.raises(rdna2_exec.Untranslated, match="v_div_fixup_f32"):
        rdna2_exec.run(program(*vop3("v_div_fixup_f32", 4, ONE, ONE, ONE, clamp=1)))


def test_recompiler_names_double_fmas():
    with pytest.raises(rdna2_hlsl.Untranslatable, match="v_div_fmas_f64"):
        rdna2_hlsl.translate(program(*vop3("v_div_fmas_f64", 4, ONE, ONE, ONE)))


# Une ligne par cas : (numérateur, dénominateur, troisième source de v_div_fmas)
LANES = [
    (0x40C0_0000, 0x4040_0000, 0x4000_0000), (0x7F00_0000, 0x4E80_0000, 0x4000_0000),
    (0x7F40_0000, 0x4E80_0000, 0xBF80_0000), (0x3F80_0000, 0x7F00_0000, 0x3F80_0000),
    (0x3F80_0001, 0x7F7F_FFFF, 0x0000_0000), (0x0380_0000, 0x3F80_0000, 0x1C80_0000),
    (0x0040_0000, 0x4000_0000, 0x0080_0001), (0x3F80_0000, 0x0000_0001, 0x5F80_0000),
    (0x2B80_0000, 0x0000_0003, 0x8000_0000), (0x0000_0000, 0x3F80_0000, 0x3F80_0000),
    (0x3F80_0000, 0x0000_0000, 0x3F80_0000), (0x7F80_0000, 0x4000_0000, 0x4040_0000),
    (0x7FC0_0000, 0x3F80_0000, 0x3F80_0000), (0x3F80_0000, 0x7F80_0000, 0xFF80_0000),
    (0xC0B0_0000, 0x3E80_0000, 0x1C80_0000), (0x4040_0000, 0xC0E0_0000, 0x8000_0001),
    (0x3F80_0001, 0x1480_0000, 0x1C80_0000), (0x3F80_0000, 0x1480_0000, 0x1C80_0000),
    (0x3380_0001, 0x3F7F_FFFE, 0x3F80_0001), (0x00FF_FFFF, 0x4000_0000, 0x3F80_0000),
    (0x00FF_FFFE, 0x4000_0000, 0x3F80_0000), (0x0000_0001, 0x4000_0000, 0x3F80_0000),
]


def lane_value(index, column):
    """Cas de LANES, puis bits pseudo-aléatoires d'exposants variés (normaux) pour les autres lignes."""
    if index < len(LANES):
        return LANES[index][column]
    seed = (index * 2654435761 + column * 40503) & 0xFFFF_FFFF
    exponent = 1 + (seed >> 8) % 253
    return (seed & 0x8000_0000) | exponent << 23 | (seed * 69069) & 0x7F_FFFF


def div_memory():
    image = bytearray(INPUT + 3 * 128)
    for column in range(3):
        words = [lane_value(lane, column) for lane in range(32)]
        image[INPUT + column * 128:INPUT + (column + 1) * 128] = struct.pack("<32I", *words)
    return bytes(image)


def div_program():
    ops = [
        *global_("global_load_dword", addr=31, vdst=1, saddr=0),
        *global_("global_load_dword", addr=31, vdst=2, saddr=0, offset=128),
        *global_("global_load_dword", addr=31, vdst=9, saddr=0, offset=256),
        *vop3("v_div_scale_f32", 3, V + 2, V + 2, V + 1, sdst=10),
        *vop3("v_div_scale_f32", 4, V + 1, V + 2, V + 1, sdst=VCC),
        vop1("v_mov_b32", 11, 10), vop1("v_mov_b32", 12, VCC),
        *vop3("v_div_fmas_f32", 5, V + 1, V + 2, V + 9),                     # VCC de v_div_scale
        sop1("s_mov_b32", VCC, LITERAL), 0xA5A7_F00F,
        *vop3("v_div_fmas_f32", 13, V + 1, V + 2, V + 9),                    # VCC choisi
        *vop3("v_div_fmas_f32", 14, V + 9, V + 2, V + 1, neg=0b010),
        *vop3("v_div_fixup_f32", 6, V + 9, V + 2, V + 1),
        *vop3("v_div_fixup_f32", 7, V + 9, V + 2, V + 1, abs_=0b001, neg=0b010),
        *vop3("v_div_fixup_f64", 15, V + 1, V + 9, V + 2, neg=0b100),        # paires v1:v2, v9:v10, v2:v3
        *vop3("v_div_fixup_f64", 17, V + 4, V + 1, V + 11),
    ]
    stores = []
    for slot, register in enumerate([3, 4, 5, 6, 7, 11, 12, 13, 14, 15, 16, 17, 18]):
        stores += store_lane(register, slot)
    return with_lane_offset(*ops, *stores)


def test_hlsl_translates_division():
    hlsl = rdna2_hlsl.translate(div_program())
    for token in ("DivScaleF32(", "DivFmasF32(", "DivFixupF32(", "DivFixupF64("):
        assert token in hlsl


@gpu
def test_division_matches_interpreter_on_gpu():
    # Les aides HLSL de la division, f32 comprises, calculent en double pour n'arrondir qu'une fois :
    # sans doubles (mesuré : Intel UHD 8086:a78b), le DXIL est refusé ; limite connue, pas d'échec.
    if not rdna2_gpu.supports_doubles():
        pytest.skip("l'adaptateur n'exécute pas de doubles, nécessaires à la traduction de la division")
    expected, actual = compare(div_program(), memory=div_memory())
    assert actual == expected
