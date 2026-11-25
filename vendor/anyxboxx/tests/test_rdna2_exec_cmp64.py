"""Comparaisons 64 bits (f64, i64, u64), v_cmp_class et tests flottants non ordonnés.

Valeurs attendues calculées à la main d'après la documentation publique d'AMD (« RDNA 2
Instruction Set Architecture », VOPC) ; le test GPU vérifie le HLSL contre l'interpréteur.
"""
import struct

import pytest

from emu import rdna2_exec, rdna2_gpu, rdna2_hlsl
from emu.rdna2_exec_cmp64 import double_bits, float_class
from emu.rdna2_exec_ops import float_bits
from test_rdna2_exec import V, const, program, vop1, vop2, vop3, vopc
from test_rdna2_exec_memory import INPUT
from test_rdna2_hlsl import compare, global_, store_lane, with_lane_offset

LITERAL, ONE, TWO, MINUS_ONE = 255, 242, 244, 193
ALL = 0xFFFF_FFFF
gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc ou anyxboxx_gpu_run.exe absent")

QNAN64, SNAN64, NINF64 = 0x7FF8_0000_0000_0000, 0x7FF0_0000_0000_0001, 0xFFF0_0000_0000_0000
QNAN32, SNAN32 = 0x7FC0_0000, 0x7F80_0001


def run(*words):
    return rdna2_exec.run(program(*words))


def mov64(vdst, value):
    return (vop1("v_mov_b32", vdst, LITERAL), value & ALL, vop1("v_mov_b32", vdst + 1, LITERAL), value >> 32)


@pytest.mark.parametrize("bits, width, index", [
    (SNAN64, 64, 0), (QNAN64, 64, 1), (NINF64, 64, 2), (double_bits(-1.0), 64, 3), (1 << 63 | 1, 64, 4),
    (1 << 63, 64, 5), (0, 64, 6), (1, 64, 7), (double_bits(1.0), 64, 8), (0x7FF0 << 48, 64, 9),
    (SNAN32, 32, 0), (QNAN32, 32, 1), (0xFF80_0000, 32, 2), (0x8000_0001, 32, 4), (0x0000_0001, 32, 7),
])
def test_ieee_class_index(bits, width, index):
    assert float_class(bits, width) == index


def test_unsigned_and_signed_64_bit_order():
    wave = run(
        *mov64(1, 1 << 32), *mov64(3, 0xFFFF_FFFF),
        vopc("v_cmp_lt_u64", V + 1, 3),                         # 2^32 < 2^32 - 1 : faux
        *vop3("v_cmp_gt_u64", 40, V + 1, V + 3),
        *mov64(5, (1 << 64) - 1),
        *vop3("v_cmp_lt_i64", 41, V + 5, const(1)),             # -1 < 1
        *vop3("v_cmp_lt_u64", 42, V + 5, const(1)),             # 2^64 - 1 < 1 : faux
        *vop3("v_cmp_eq_i64", 43, MINUS_ONE, V + 5))            # constante -1 étendue sur 64 bits
    assert wave.vcc == 0
    assert wave.s[40:44] == [ALL, ALL, 0, ALL]


def test_double_comparisons_with_inline_constants():
    wave = run(
        *mov64(1, double_bits(1.0)), *mov64(3, QNAN64),
        vopc("v_cmp_eq_f64", ONE, 1),                           # 1.0 == v[1:2]
        *vop3("v_cmp_gt_f64", 40, TWO, V + 1),                  # 2.0 > 1.0
        *vop3("v_cmp_u_f64", 41, V + 1, V + 3),                 # NaN : non ordonné
        *vop3("v_cmp_neq_f64", 42, V + 3, V + 3),               # NaN != NaN
        *vop3("v_cmp_eq_f64", 43, V + 3, V + 3))
    assert wave.vcc == ALL
    assert wave.s[40:44] == [ALL, ALL, ALL, 0]


def test_class_masks_select_lanes():
    wave = run(
        *mov64(1, NINF64),
        *vop3("v_cmp_class_f64", 40, V + 1, LITERAL), 1 << 2,         # -infini
        *vop3("v_cmp_class_f64", 41, V + 1, LITERAL), 1 << 9,
        vop1("v_mov_b32", 3, LITERAL), QNAN32, vop1("v_mov_b32", 4, const(1 << 1)),
        vopc("v_cmp_class_f32", V + 3, 4),                            # qNaN : bit 1
        *vop3("v_cmp_class_f32", 42, V + 3, LITERAL), 0b1_1111_1100)  # tout sauf les NaN
    assert wave.s[40:43] == [ALL, 0, 0]
    assert wave.vcc == ALL


def test_vop3_negation_changes_the_class():
    wave = run(*mov64(1, double_bits(1.0)), *vop3("v_cmp_class_f64", 40, V + 1, LITERAL, neg=1), 1 << 3)
    assert wave.s[40] == ALL                                           # -1.0 : -normal


def test_cmpx_writes_exec():
    wave = run(*mov64(1, 5), vopc("v_cmpx_gt_u64", const(4), 1))       # 4 > 5 : faux sur toutes les lignes
    assert wave.exec == 0


def test_literal_in_a_64_bit_comparison_is_named():
    data = program(vopc("v_cmp_eq_u64", LITERAL, 1), 7)
    with pytest.raises(rdna2_exec.Untranslated, match="littéral"):
        rdna2_exec.run(data)


NAN, INF = 0x7FC0_0000, 0x7F80_0000
VALUES = [NAN, SNAN32, INF, 0xFF80_0000, float_bits(1.0), float_bits(2.0), 0x8000_0000, 0, 0x0000_0001,
          float_bits(-1.0)]
F32_TESTS = ["f", "lt", "eq", "le", "gt", "lg", "ge", "o", "u", "nge", "nlg", "ngt", "nle", "neq", "nlt", "tru"]


def f32_compare_program():
    """Chaque ligne compare son couple (a, b) ; résultats en 0/1, un emplacement par test."""
    words = [*global_("global_load_dword", addr=31, vdst=1, saddr=0),
             *global_("global_load_dword", addr=31, vdst=2, saddr=0, offset=128),
             vop1("v_mov_b32", 3, const(1))]
    tests = [f"v_cmp_{name}_f32" for name in F32_TESTS] + ["v_cmp_class_f32"]
    for slot, name in enumerate(tests):
        words += [vopc(name, V + 1, 2), vop2("v_cndmask_b32", 10, const(0), 3), *store_lane(10, slot)]
    return with_lane_offset(*words)


def f32_compare_memory():
    pairs = [(VALUES[i % 10], VALUES[(i * 3 + 1) % 10]) for i in range(32)]
    memory = bytearray(INPUT + 256)
    memory[INPUT:INPUT + 128] = struct.pack("<32I", *[a for a, _ in pairs])
    memory[INPUT + 128:INPUT + 256] = struct.pack("<32I", *[b | 1 << (i % 10) for i, (_, b) in enumerate(pairs)])
    return bytes(memory)


def test_hlsl_translates_every_f32_comparison():
    hlsl = rdna2_hlsl.translate(f32_compare_program())
    assert "IsNanF32(" in hlsl and "ClassF32(" in hlsl


def test_64_bit_comparisons_are_named_by_the_recompiler():
    with pytest.raises(rdna2_hlsl.Untranslatable, match="v_cmp_lt_u64"):
        rdna2_hlsl.translate(program(vopc("v_cmp_lt_u64", V + 1, 3)))


@gpu
def test_f32_comparisons_with_nan_infinity_and_zero_match_on_gpu():
    expected, actual = compare(f32_compare_program(), memory=f32_compare_memory())
    assert actual == expected

