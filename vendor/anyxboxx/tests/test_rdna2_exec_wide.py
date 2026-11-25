"""Instructions à opérandes 64 bits : double précision (VOP1/VOP3) et décalages 64 bits.

Valeurs attendues calculées à la main d'après la documentation publique d'AMD (« RDNA 2
Instruction Set Architecture ») et IEEE 754 ; le test GPU vérifie le HLSL contre l'interpréteur.
"""
import math
import struct

import pytest

from emu import rdna2_exec, rdna2_gpu, rdna2_hlsl, rdna2_hlsl_wide
from emu.rdna2_exec_ops import float_bits
from test_rdna2_exec import V, const, program, vop1, vop2, vop3
from test_rdna2_exec_memory import INPUT
from test_rdna2_hlsl import compare, global_, store_lane, with_lane_offset

LITERAL, ONE, TWO = 255, 242, 244
gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc ou anyxboxx_gpu_run.exe absent")
MAX_DOUBLE = 1.7976931348623157e308


def dbits(value):
    return struct.unpack("<Q", struct.pack("<d", value))[0]


def dval(low, high):
    return struct.unpack("<d", struct.pack("<Q", low | high << 32))[0]


def mov64(vdst, bits):
    return (vop1("v_mov_b32", vdst, LITERAL), bits & 0xFFFF_FFFF, vop1("v_mov_b32", vdst + 1, LITERAL), bits >> 32)


def result64(wave, register):
    return dval(wave.v[register][0], wave.v[register + 1][0])


def unary(name, value):
    return rdna2_exec.run(program(*mov64(2, dbits(value)), vop1(name, 4, V + 2)))


@pytest.mark.parametrize("name, value, expected", [
    ("v_trunc_f64", -2.5, -2.0), ("v_ceil_f64", -2.5, -2.0), ("v_floor_f64", -2.5, -3.0),
    ("v_rndne_f64", -2.5, -2.0), ("v_rndne_f64", 3.5, 4.0), ("v_fract_f64", -0.25, 0.75),
    ("v_rcp_f64", 4.0, 0.25), ("v_rcp_f64", -0.0, -math.inf), ("v_rsq_f64", 4.0, 0.5),
    ("v_sqrt_f64", 2.0, 1.4142135623730951), ("v_frexp_mant_f64", 8.0, 0.5), ("v_frexp_mant_f64", -3.0, -0.75),
    ("v_floor_f64", math.inf, math.inf),
])
def test_double_unary_operations(name, value, expected):
    assert result64(unary(name, value), 4) == expected


@pytest.mark.parametrize("name, value", [("v_fract_f64", math.inf), ("v_sqrt_f64", -1.0), ("v_rsq_f64", -4.0)])
def test_double_operations_returning_nan(name, value):
    assert math.isnan(result64(unary(name, value), 4))


def unary_bits(name, bits):
    wave = rdna2_exec.run(program(*mov64(2, bits), vop1(name, 4, V + 2)))
    return wave.v[4][0] | wave.v[5][0] << 32


QNAN64, NEG_QNAN64, SNAN64 = 0x7FF8_0000_0000_1234, 0xFFF8_0000_0000_1234, 0x7FF0_0000_0000_0001
NEG_ZERO64 = 1 << 63


@pytest.mark.parametrize("name, bits, expected", [
    ("v_fract_f64", QNAN64, QNAN64), ("v_fract_f64", NEG_QNAN64, NEG_QNAN64),       # NaN rendu tel quel
    ("v_fract_f64", SNAN64, 0x7FF8_0000_0000_0001),                                 # rendu silencieux
    ("v_fract_f64", dbits(math.inf), 0x7FF8_0000_0000_0000), ("v_fract_f64", dbits(-math.inf), 0x7FF8_0000_0000_0000),
    ("v_fract_f64", dbits(-1e-20), 0x3FEF_FFFF_FFFF_FFFF),                           # jamais 1.0 (LLVM)
    ("v_fract_f64", NEG_ZERO64, 0), ("v_fract_f64", dbits(2.5), dbits(0.5)),
    ("v_trunc_f64", dbits(-0.5), NEG_ZERO64), ("v_ceil_f64", dbits(-0.5), NEG_ZERO64),
    ("v_rndne_f64", dbits(-0.5), NEG_ZERO64), ("v_floor_f64", NEG_ZERO64, NEG_ZERO64),
    ("v_floor_f64", dbits(0.5), 0), ("v_trunc_f64", SNAN64, 0x7FF8_0000_0000_0001),
])
def test_double_rounding_and_fract_edge_cases(name, bits, expected):
    assert unary_bits(name, bits) == expected


def test_inline_one_over_two_pi_is_the_isa_double():
    """ISA §6.2 (p. 41) : 1/(2π) en double vaut exactement 0x3FC45F306DC9C882 (pas l'arrondi de Python)."""
    wave = rdna2_exec.run(program(*vop3("v_mul_f64", 4, 248, ONE)))
    assert wave.v[4][0] | wave.v[5][0] << 32 == 0x3FC4_5F30_6DC9_C882
    assert rdna2_hlsl_wide._words(248) == ("0x6DC9C882u", "0x3FC45F30u")


@pytest.mark.parametrize("name, value, expected", [
    ("v_cvt_i32_f64", 2.9, 2), ("v_cvt_i32_f64", -2.9, -2), ("v_cvt_i32_f64", 1e10, 0x7FFF_FFFF),
    ("v_cvt_i32_f64", -math.inf, 0x8000_0000), ("v_cvt_i32_f64", math.nan, 0),
    ("v_cvt_u32_f64", 5e9, 0xFFFF_FFFF), ("v_cvt_u32_f64", -1.0, 0),
    ("v_frexp_exp_i32_f64", 8.0, 4),
    ("v_cvt_f32_f64", 1 / 3, 0x3EAA_AAAB), ("v_cvt_f32_f64", 1e300, 0x7F80_0000),
])
def test_double_to_32_bit_results(name, value, expected):
    assert unary(name, value).v[4][0] == expected & 0xFFFF_FFFF


def test_32_bit_to_double_conversions():
    wave = rdna2_exec.run(program(
        vop1("v_mov_b32", 1, LITERAL), 0xFFFF_FFF9,                       # -7
        vop1("v_cvt_f64_i32", 2, V + 1), vop1("v_cvt_f64_u32", 4, V + 1),
        vop1("v_mov_b32", 6, LITERAL), float_bits(0.1),
        vop1("v_cvt_f64_f32", 8, V + 6)))
    assert result64(wave, 2) == -7.0
    assert result64(wave, 4) == 4294967289.0
    assert result64(wave, 8) == 0.10000000149011612                      # 0.1f exactement


F32_SOURCES = [float_bits(1.0), float_bits(-1.0), 0, 0x8000_0000, 0x7F80_0000, 0xFF80_0000, 0x7FC0_0000,
               0xFFC0_0000, 0x7FC1_2345, 0xFFC1_2345, float_bits(0.1), 0x7F7F_FFFF, 0x0080_0000, 0x8080_0000,
               0x0000_0001, 0x8000_0001, 0x007F_FFFF, 0x8001_2345]                # dénormaux : gardés (ISA)
MODIFIERS = [(0, 0), (0, 1), (1, 0), (1, 1)]                             # (abs, neg) : aucun, -x, |x|, -|x|


@pytest.mark.parametrize("abs_, neg, expected", [(0, 1, -1.0), (1, 0, 1.0), (1, 1, -1.0)])
def test_f32_to_double_applies_source_modifiers(abs_, neg, expected):
    wave = rdna2_exec.run(program(vop1("v_mov_b32", 2, LITERAL), float_bits(-1.0 if abs_ else 1.0),
                                  *vop3("v_cvt_f64_f32", 4, V + 2, abs_=abs_, neg=neg)))
    assert result64(wave, 4) == expected


def f32_to_double_program():
    """Chaque ligne convertit sa source f32 (INPUT + 4 × ligne) sous les quatre combinaisons abs/neg."""
    words = [*global_("global_load_dword", addr=31, vdst=1, saddr=0)]
    for slot, (abs_, neg) in enumerate(MODIFIERS):
        words += vop3("v_cvt_f64_f32", 2 + 2 * slot, V + 1, abs_=abs_, neg=neg)
    for slot in range(2 * len(MODIFIERS)):
        words += store_lane(2 + slot, slot)
    return with_lane_offset(*words)


def f32_sources_memory():
    sources = (F32_SOURCES * 3)[:32]
    memory = bytearray(INPUT + 128)
    memory[INPUT:] = struct.pack("<32I", *sources)
    return bytes(memory)


def test_f32_to_double_is_translated_without_doubles():
    [statement] = rdna2_hlsl_wide.statements("v_cvt_f64_f32", [V + 1], 2, "true", abs_bits=1, neg_bits=1)
    assert "double" not in statement and "0x7FFFFFFFu" in statement and "0x80000000u" in statement


@gpu
def test_f32_to_double_with_abs_neg_matches_interpreter_on_gpu():
    """-0.0, NaN des deux signes (charge utile gardée), ±infini, dénormaux : le signe suit abs/neg comme
    dans l'interpréteur. Traduction en entiers : vaut aussi sans doubles (Intel UHD)."""
    expected, actual = compare(f32_to_double_program(), memory=f32_sources_memory())
    assert actual == expected


def test_fma_rounds_once():
    epsilon = 2.0 ** -52
    wave = rdna2_exec.run(program(
        *mov64(2, dbits(1 + epsilon)), *mov64(4, dbits(1 - epsilon)), *mov64(6, dbits(-1.0)),
        *vop3("v_fma_f64", 8, V + 2, V + 4, V + 6),
        *vop3("v_mul_f64", 10, V + 2, V + 4), *vop3("v_add_f64", 12, V + 10, V + 6)))
    assert result64(wave, 8) == -(2.0 ** -104)                          # (1+e)(1-e) - 1 = -e²
    assert result64(wave, 12) == 0.0                                    # mul puis add : deux arrondis


def test_binary_double_operations_and_modifiers():
    wave = rdna2_exec.run(program(
        *mov64(2, dbits(0.1)), *mov64(4, dbits(0.2)), *mov64(6, dbits(math.nan)),
        *vop3("v_add_f64", 8, V + 2, V + 4),
        *vop3("v_mul_f64", 10, ONE, TWO),                                # constantes en ligne en double
        *vop3("v_add_f64", 12, ONE, TWO, neg=0b10),                      # 1.0 + -(2.0)
        *vop3("v_min_f64", 14, V + 6, TWO), *vop3("v_max_f64", 16, V + 2, V + 4),
        *vop3("v_ldexp_f64", 18, ONE, const(3)),
        *mov64(20, dbits(MAX_DOUBLE)), *vop3("v_ldexp_f64", 22, V + 20, const(2))))
    assert result64(wave, 8) == 0.30000000000000004
    assert result64(wave, 10) == 2.0 and result64(wave, 12) == -1.0
    assert result64(wave, 14) == 2.0 and result64(wave, 16) == 0.2     # NaN seul ignoré
    assert result64(wave, 18) == 8.0 and result64(wave, 22) == math.inf


def test_64_bit_shifts():
    wave = rdna2_exec.run(program(
        *mov64(2, 0x0000_000F_F000_0000), *mov64(4, 1 << 63),
        *vop3("v_lshlrev_b64", 6, const(4), V + 2),
        *vop3("v_lshlrev_b64", 8, const(40), const(1)),
        *vop3("v_lshrrev_b64", 10, const(36), V + 4),
        *vop3("v_ashrrev_i64", 12, const(4), V + 4)))
    assert (wave.v[6][0], wave.v[7][0]) == (0, 0xFF)
    assert (wave.v[8][0], wave.v[9][0]) == (0, 0x100)
    assert (wave.v[10][0], wave.v[11][0]) == (0x0800_0000, 0)
    assert (wave.v[12][0], wave.v[13][0]) == (0, 0xF800_0000)


def test_literal_on_a_64_bit_source_is_named():
    with pytest.raises(rdna2_exec.Untranslated, match="littéral"):
        rdna2_exec.run(program(vop1("v_sqrt_f64", 4, LITERAL), 0x4000_0000))


@pytest.mark.parametrize("name", ["v_sqrt_f64", "v_fract_f64"])
def test_recompiler_names_doubles_without_hlsl_equivalent(name):
    with pytest.raises(rdna2_hlsl.Untranslatable, match=name):
        rdna2_hlsl.translate(program(*mov64(2, dbits(2.0)), vop1(name, 4, V + 2)))


def wide_program():
    """Valeurs propres à chaque ligne : doubles exacts tirés du numéro de ligne."""
    ops = [
        vop2("v_subrev_nc_u32", 1, const(15), 0),                        # v1 = ligne - 15
        vop1("v_cvt_f64_i32", 2, V + 1),                                 # v[2:3] = ligne - 15
        vop1("v_cvt_f64_u32", 4, V + 0),                                 # v[4:5] = ligne
        vop1("v_mov_b32", 6, LITERAL), float_bits(0.1), vop1("v_cvt_f64_f32", 6, V + 6),
        *vop3("v_add_f64", 8, V + 2, V + 6), *vop3("v_mul_f64", 10, V + 2, V + 6),
        *vop3("v_fma_f64", 12, V + 2, V + 6, V + 4, neg=0b001),
        *vop3("v_min_f64", 14, V + 2, V + 4), *vop3("v_max_f64", 16, V + 2, ONE),
        *vop3("v_mul_f64", 18, V + 2, TWO),
        vop1("v_cvt_f32_f64", 20, V + 10), vop1("v_cvt_i32_f64", 21, V + 8), vop1("v_cvt_u32_f64", 22, V + 8),
        *vop3("v_lshlrev_b64", 24, V + 0, V + 2),
        *vop3("v_lshrrev_b64", 26, V + 0, V + 2),
        *vop3("v_ashrrev_i64", 28, V + 0, V + 2),
    ]
    stores = []
    for slot, register in enumerate([*range(8, 20), 20, 21, 22, *range(24, 30)]):
        stores += store_lane(register, slot)
    return with_lane_offset(*ops, *stores)


def test_hlsl_translates_double_operations():
    hlsl = rdna2_hlsl.translate(wide_program())
    for token in ("asdouble(", "fma(", "SatF64ToI32(", "SatF64ToU32("):
        assert token in hlsl


@gpu
def test_double_operations_match_interpreter_on_gpu():
    if not rdna2_gpu.supports_doubles():                 # mesuré : Intel UHD (a78b) refuse le DXIL à doubles
        pytest.skip("l'adaptateur n'exécute pas de doubles dans les shaders (double_precision=non)")
    expected, actual = compare(wide_program())
    assert actual == expected


@gpu
def test_adapter_reports_double_support():
    assert rdna2_gpu.adapter()["double_precision"] in ("oui", "non")

