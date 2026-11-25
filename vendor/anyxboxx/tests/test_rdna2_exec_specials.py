"""Valeurs spéciales (±infini, NaN, plus grand flottant) dans l'interpréteur RDNA 2.

Avant correction, v_cvt_i32_f32, v_cvt_u32_f32 et v_fract_f32 (±infini), v_exp_f32 et
v_ldexp_f32 (dépassement) levaient OverflowError au lieu de rendre un résultat.
"""
import struct

import pytest

from emu import rdna2_exec, rdna2_exec_ops, rdna2_exec_vop3, rdna2_gpu
from emu.rdna2_exec_ops import float_bits
from test_rdna2_exec import ONE as INLINE_ONE, V, const, program, vop1, vop2, vop3
from test_rdna2_exec_memory import INPUT
from test_rdna2_hlsl import compare, global_, store_lane, with_lane_offset

LITERAL = 255
PLUS_INF, MINUS_INF, NAN, MAX, ONE = 0x7F80_0000, 0xFF80_0000, 0x7FC0_0000, 0x7F7F_FFFF, 0x3F80_0000
SPECIALS = (PLUS_INF, MINUS_INF, NAN, MAX, MAX | 1 << 31)
gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc ou anyxboxx_gpu_run.exe absent")


def lane0(*words):
    return rdna2_exec.run(program(*words)).v[2][0]


@pytest.mark.parametrize("name, value, expected", [
    ("v_cvt_i32_f32", PLUS_INF, 0x7FFF_FFFF), ("v_cvt_i32_f32", MINUS_INF, 0x8000_0000),
    ("v_cvt_u32_f32", PLUS_INF, 0xFFFF_FFFF), ("v_cvt_u32_f32", MINUS_INF, 0),
    ("v_fract_f32", PLUS_INF, NAN), ("v_fract_f32", MINUS_INF, NAN),   # inf + -floor(inf) = inf - inf
    ("v_exp_f32", MAX, PLUS_INF),
])
def test_unary_operations_on_special_values(name, value, expected):
    assert lane0(vop1("v_mov_b32", 1, LITERAL), value, vop1(name, 2, V + 1)) == expected


@pytest.mark.parametrize("value, expected", [(MAX, PLUS_INF), (MAX | 1 << 31, MINUS_INF)])
def test_ldexp_overflows_to_signed_infinity(value, expected):
    assert lane0(vop1("v_mov_b32", 1, LITERAL), value, *vop3("v_ldexp_f32", 2, V + 1, 128 + 10)) == expected


def test_no_operation_raises_on_special_values():
    """Toutes les opérations ALU, sources ±infini, NaN et ±plus grand flottant."""
    for value in SPECIALS:
        for name, operation in rdna2_exec_ops.VECTOR.items():
            operation(value, value, value, ONE, 0)
            operation(value, ONE, ONE, ONE, 0)
        for name, operation in rdna2_exec_vop3.VECTOR3.items():
            operation(value, value, value)


def specials_program():
    """Cas dont le résultat n'est pas NaN (le motif d'un NaN produit diffère selon le GPU : mesuré
    0x7FFFFFFF sur NVIDIA, 0xFFC00000 sur WARP, 0x7FC00000 pour RDNA). Avant correction, le HLSL
    convertissait ±infini et les valeurs hors bornes par un simple cast, indéfini en HLSL."""
    words = []
    cases = [("v_cvt_i32_f32", PLUS_INF), ("v_cvt_i32_f32", MINUS_INF), ("v_cvt_u32_f32", PLUS_INF),
             ("v_cvt_u32_f32", MINUS_INF), ("v_exp_f32", MAX), ("v_cvt_i32_f32", NAN), ("v_cvt_u32_f32", NAN),
             ("v_cvt_i32_f32", float_bits(3e9)), ("v_cvt_i32_f32", float_bits(-3e9)),
             ("v_cvt_i32_f32", float_bits(2.0 ** 31)), ("v_cvt_u32_f32", float_bits(5e9)),
             ("v_cvt_u32_f32", float_bits(-1.5)), ("v_cvt_i32_f32", float_bits(-1.5))]
    for slot, (name, value) in enumerate(cases):
        words += [vop1("v_mov_b32", 1, LITERAL), value, vop1(name, 2, V + 1), *store_lane(2, slot)]
    words += [vop1("v_mov_b32", 1, LITERAL), MAX, *vop3("v_ldexp_f32", 2, V + 1, 128 + 10), *store_lane(2, len(cases))]
    return with_lane_offset(*words)


@gpu
def test_special_values_match_on_gpu():
    expected, actual = compare(specials_program())
    assert actual == expected


NEG_ZERO = 0x8000_0000
ROUNDINGS = ["v_fract_f32", "v_trunc_f32", "v_ceil_f32", "v_rndne_f32", "v_floor_f32"]
# NaN, ±infini (fract → NaN) et dénormaux exclus : NaN produit et vidage des dénormaux diffèrent selon le GPU (mesuré)
ROUNDING_SOURCES = [float_bits(-1e-20), float_bits(-0.5), NEG_ZERO, 0, float_bits(-0.25), float_bits(-1e-7),
                    float_bits(2.5), float_bits(-2.5), float_bits(1.5), float_bits(-1.5), float_bits(0.5),
                    MAX, MAX | 1 << 31, float_bits(-0.75), float_bits(7.0e6), float_bits(-8388607.5)]


@pytest.mark.parametrize("name, value, expected", [
    ("v_trunc_f32", float_bits(-0.5), NEG_ZERO), ("v_ceil_f32", float_bits(-0.5), NEG_ZERO),
    ("v_rndne_f32", float_bits(-0.5), NEG_ZERO), ("v_floor_f32", NEG_ZERO, NEG_ZERO),
    ("v_floor_f32", float_bits(0.5), 0), ("v_fract_f32", NEG_ZERO, 0),
    ("v_fract_f32", float_bits(-1e-20), 0x3F7F_FFFF),                # jamais 1.0 (LLVM, WARP, Intel UHD)
    ("v_fract_f32", float_bits(-0.25), float_bits(0.75)),
])
def test_rounding_keeps_the_sign_of_zero_and_fract_stays_below_one(name, value, expected):
    assert lane0(vop1("v_mov_b32", 1, LITERAL), value, vop1(name, 2, V + 1)) == expected


def rounding_program():
    """Chaque ligne arrondit sa source (INPUT + 4 × ligne) par les cinq instructions."""
    words = [*global_("global_load_dword", addr=31, vdst=1, saddr=0)]
    for slot, name in enumerate(ROUNDINGS):
        words += [vop1(name, 2 + slot, V + 1)]
    for slot in range(len(ROUNDINGS)):
        words += store_lane(2 + slot, slot)
    return with_lane_offset(*words)


SIGNED = [0x8000_0000, 0, ONE, ONE | 1 << 31, float_bits(3.0), float_bits(-3.0)]       # ±0, ±1, ±3


def zero_folds_program():
    """x + 0, 0 - x, x - x, 0 × x et fma(x, 1, 0) : sans `precise`, dxc et les pilotes simplifiaient ces
    expressions (-0 au lieu de +0, ou 0 au lieu du NaN de ±infini - ±infini). v1 : ±0, ±1, ±3, ±infini ;
    v2, pour 0 × x : ±0, ±1, ±3, ±0,5 (0 × ±infini donne un NaN dont le motif dépend du GPU)."""
    words = [*global_("global_load_dword", addr=31, vdst=1, saddr=0),
             *global_("global_load_dword", addr=31, vdst=2, saddr=0, offset=128)]
    operations = [(vop2("v_add_f32", 10, const(0), 1),), vop3("v_add_f32", 10, V + 1, const(0)),
                  (vop2("v_sub_f32", 10, const(0), 1),), vop3("v_subrev_f32", 10, V + 1, const(0)),
                  (vop2("v_sub_f32", 10, V + 1, 1),), (vop2("v_mul_f32", 10, const(0), 2),),
                  vop3("v_fma_f32", 10, V + 1, INLINE_ONE, const(0)),
                  (vop1("v_mov_b32", 10, const(0)), vop2("v_fmac_f32", 10, INLINE_ONE, 1))]
    for slot, operation in enumerate(operations):
        words += [*operation, *store_lane(10, slot)]
    return with_lane_offset(*words)


@gpu
def test_rounding_and_fract_match_on_gpu():
    memory = bytearray(INPUT + 128)
    memory[INPUT:] = struct.pack("<32I", *(ROUNDING_SOURCES * 2))
    expected, actual = compare(rounding_program(), memory=bytes(memory))
    assert actual == expected


@gpu
def test_constant_zero_folds_match_on_gpu():
    memory = bytearray(INPUT + 256)
    struct.pack_into("<32I", memory, INPUT, *(SIGNED + [PLUS_INF, MINUS_INF]) * 4)
    struct.pack_into("<32I", memory, INPUT + 128, *(SIGNED + [float_bits(0.5), float_bits(-0.5)]) * 4)
    expected, actual = compare(zero_folds_program(), memory=bytes(memory))
    assert actual == expected
