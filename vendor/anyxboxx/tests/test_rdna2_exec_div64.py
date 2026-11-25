"""Division IEEE en double : v_div_scale_f64, puis la séquence de LLVM (lowerFDIV64) entière.

Valeurs attendues calculées à la main d'après le pseudo-code d'AMD (« RDNA 3 Instruction Set
Architecture », mêmes descriptions dans la spécification XML RDNA 2) : comme en f32 avec un écart
d'exposants de 768, une échelle de 2^128 et un numérateur minuscule si son exposant biaisé est ≤ 53.
Exposants biaisés en f64 : champ = 1023 + puissance de 2, bits = champ << 52.
"""
import pytest

from emu import rdna2_exec, rdna2_hlsl
from test_rdna2_exec import V, program, vop1, vop3
from test_rdna2_exec_div import ONE, VCC, mov64

SDST = 10


def power(exponent):
    """Bits f64 de 2^exponent (normal)."""
    return (1023 + exponent) << 52


def scale64(numerator, denominator):
    """(échelle du dénominateur, échelle du numérateur, VCC du numérateur, bit SDST du dénominateur)."""
    wave = rdna2_exec.run(program(
        *mov64(2, numerator), *mov64(4, denominator),
        *vop3("v_div_scale_f64", 6, V + 4, V + 4, V + 2, sdst=SDST),
        *vop3("v_div_scale_f64", 8, V + 2, V + 4, V + 2, sdst=VCC)))
    pair = [wave.v[r][0] | wave.v[r + 1][0] << 32 for r in (6, 8)]
    return pair[0], pair[1], wave.vcc & 1, wave.s[SDST] & 1


@pytest.mark.parametrize("numerator, denominator, expected", [
    # 6 / 3 : écart 1025 - 1024 < 768, quotient 2 normal, exposant du numérateur 1025 > 53 : rien
    (0x4018_0000_0000_0000, 0x4008_0000_0000_0000, (0x4008_0000_0000_0000, 0x4018_0000_0000_0000, 0, 0)),
    # 2^1023 / 2^200 : écart 2046 - 1223 = 823 ≥ 768, VCC ; d × 2^128 = 2^328 (champ 1351 = 0x547)
    (power(1023), power(200), (0x5470_0000_0000_0000, 0x7FE0_0000_0000_0000, 1, 1)),
    # 2^-900 / 2^-1074 (dénormal) : écart 123 - 0 < 768, S1 dénormal : les deux × 2^128, VCC = 0 ;
    # 2^-946 (champ 77 = 0x04D) et 2^-772 (champ 251 = 0x0FB)
    (power(-900), 0x0000_0000_0000_0001, (0x04D0_0000_0000_0000, 0x0FB0_0000_0000_0000, 0, 0)),
    # 2^-30 / 2^1000 = 2^-1030, dénormal : VCC ; seul le numérateur × 2^128 = 2^98 (champ 1121 = 0x461)
    (power(-30), power(1000), (0x7E70_0000_0000_0000, 0x4610_0000_0000_0000, 1, 1)),
    # 2^-1000 / 1 : quotient normal, numérateur minuscule (champ 23 ≤ 53) : les deux × 2^128, VCC = 0 ;
    # 2^128 (champ 1151 = 0x47F) et 2^-872 (champ 151 = 0x097)
    (power(-1000), power(0), (0x47F0_0000_0000_0000, 0x0970_0000_0000_0000, 0, 0)),
    # 1 / 2^1022 : 1/S1 = 2^-1022, le plus petit normal, pas dénormal ; rien
    (power(0), power(1022), (0x7FD0_0000_0000_0000, 0x3FF0_0000_0000_0000, 0, 0)),
    # 0 / 2 : NaN, VCC = 0 (pseudo-code d'AMD)
    (0, power(1), (0x7FF8_0000_0000_0000, 0x7FF8_0000_0000_0000, 0, 0)),
])
def test_div_scale_f64_branches(numerator, denominator, expected):
    assert scale64(numerator, denominator) == expected


def test_div_scale_f64_source_modifier():
    # -6 / 3 : neg sur S0 seul ; le dénominateur 3 est rendu changé de signe, VCC = 0
    wave = rdna2_exec.run(program(
        *mov64(2, 0x4018_0000_0000_0000), *mov64(4, 0x4008_0000_0000_0000),
        *vop3("v_div_scale_f64", 6, V + 4, V + 4, V + 2, neg=0b001, sdst=SDST)))
    assert (wave.v[6][0], wave.v[7][0], wave.s[SDST]) == (0, 0xC008_0000, 0)


def test_one_over_denormal_reciprocal_is_refused_by_name():
    # 1 / 2^1023 : 1/S1 = 2^-1023 dénormal, branche du pseudo-code jugée incohérente
    with pytest.raises(rdna2_exec.Untranslated, match="v_div_scale_f64 : 1/S1 dénormal"):
        scale64(power(0), power(1023))


def division64(numerator, denominator):
    """Séquence de LLVM (SIISelLowering, lowerFDIV64) pour numérateur / dénominateur."""
    wave = rdna2_exec.run(program(
        *mov64(2, numerator), *mov64(4, denominator),
        *vop3("v_div_scale_f64", 6, V + 4, V + 4, V + 2, sdst=SDST),              # d'
        vop1("v_rcp_f64", 8, V + 6),                                                # r = 1/d'
        *vop3("v_fma_f64", 10, V + 6, V + 8, ONE, neg=0b001),                       # 1 - d' r
        *vop3("v_fma_f64", 8, V + 8, V + 10, V + 8),                                # r += r e
        *vop3("v_fma_f64", 10, V + 6, V + 8, ONE, neg=0b001),
        *vop3("v_div_scale_f64", 12, V + 2, V + 4, V + 2, sdst=VCC),               # n', VCC
        *vop3("v_fma_f64", 8, V + 8, V + 10, V + 8),
        *vop3("v_mul_f64", 14, V + 12, V + 8),                                      # q = n' r
        *vop3("v_fma_f64", 16, V + 6, V + 14, V + 12, neg=0b001),                   # n' - d' q
        *vop3("v_div_fmas_f64", 16, V + 16, V + 8, V + 14),
        *vop3("v_div_fixup_f64", 18, V + 16, V + 4, V + 2)))
    return wave.v[18][0] | wave.v[19][0] << 32


@pytest.mark.parametrize("numerator, denominator, expected", [
    (power(0), 0x4008_0000_0000_0000, 0x3FD5_5555_5555_5555),     # 1/3 arrondi au plus près (…5555, sous le milieu)
    (0x4018_0000_0000_0000, 0xC000_0000_0000_0000, 0xC008_0000_0000_0000),     # 6 / -2 = -3
    # 2^-30 / 2^1000 : n' = 2^98, r = 2^-1000 exact, q = 2^-902, reste 0 ; VCC et |q| < 2 : × 2^-128
    # = 2^-1030, dénormal exact : 2^-1030 / 2^-1074 = 2^44 → bits 0x0000_1000_0000_0000
    (power(-30), power(1000), 0x0000_1000_0000_0000),
])
def test_llvm_double_division_sequence(numerator, denominator, expected):
    assert division64(numerator, denominator) == expected


def test_recompiler_names_double_scale():
    with pytest.raises(rdna2_hlsl.Untranslatable, match="v_div_scale_f64 .*1/S1 dénormal"):
        rdna2_hlsl.translate(program(*vop3("v_div_scale_f64", 4, ONE, ONE, ONE, sdst=SDST)))
