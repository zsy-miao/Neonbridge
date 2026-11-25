"""Demi-précision (f16) dans l'interpréteur de référence RDNA 2 : valeurs attendues écrites à la main
d'après la documentation ISA d'AMD (arrondi au plus proche pair, dénormaux conservés)."""
import math

import pytest

from emu import rdna2_exec, rdna2_exec_f16 as f16
from emu.rdna2_exec_ops import float_bits
from test_rdna2_exec import EXEC_LO, LITERAL, ONE, V, const, program, sop1, vop1, vop2, vop3

OLD = 0x1234_5678               # destination avant l'instruction : sa moitié haute doit rester


def run(name, a=0, b=0, c=0, d=0, k=0):
    return f16.OPERATIONS[name][1](a, b, c, d, k)


def test_rounding_to_half_is_nearest_even():
    cases = {1.0: 0x3C00, 1 + 2**-11: 0x3C00, 1 + 3 * 2**-11: 0x3C02, 65504.0: 0x7BFF, 65519.0: 0x7BFF,
             65520.0: 0x7C00, -65520.0: 0xFC00, 1e9: 0x7C00, 2**-24: 0x0001, 2**-25: 0x0000,
             1.5 * 2**-24: 0x0002, 2**-25 + 2**-40: 0x0001, -0.0: 0x8000}
    for value, bits in cases.items():
        assert f16.half_bits(value) == bits, value
    assert f16.half_bits(math.nan) == f16.half_bits(-math.nan) == 0x7E00


def test_arithmetic_rounds_once():
    assert run("v_add_f16", 0x3C00, 0x3C00) == 0x4000
    assert run("v_add_f16", 0x3C00, 0x1000) == 0x3C00          # 1 + 2^-11 : égalité, pair
    assert run("v_add_f16", 0x3C01, 0x1000) == 0x3C02
    assert run("v_add_f16", 0x7BFF, 0x7BFF) == 0x7C00
    assert run("v_add_f16", 0x7C00, 0xFC00) == 0x7E00
    assert run("v_add_f16", 0x0001, 0x8001) == 0x0000
    assert run("v_add_f16", 0x0400, 0x8001) == 0x03FF
    assert run("v_sub_f16", 0x3C00, 0x3C00) == 0x0000
    assert run("v_subrev_f16", 0x3C00, 0x4000) == 0x3C00
    assert run("v_mul_f16", 0x3C01, 0x3C01) == 0x3C02
    assert run("v_mul_f16", 0x0001, 0x3800) == 0x0000
    assert run("v_mul_f16", 0x0003, 0x3800) == 0x0002
    assert run("v_mul_f16", 0x7C00, 0x0000) == 0x7E00


def test_fused_multiply_add_rounds_once():
    assert run("v_fma_f16", 0x101F, 0x3BC4, 0x3C00) == 0x3C01   # f32 puis f16 donnerait 0x3C00
    assert run("v_fmac_f16", 0x101F, 0x3BC4, d=0x3C00) == 0x3C01
    assert run("v_fmamk_f16", 0x101F, 0x3C00, k=0x3BC4) == 0x3C01
    assert run("v_fmaak_f16", 0x101F, 0x3BC4, k=0x3C00) == 0x3C01


def test_min_and_max_follow_rdna2():
    assert run("v_min_f16", 0x8000, 0x0000) == run("v_min_f16", 0x0000, 0x8000) == 0x8000
    assert run("v_max_f16", 0x8000, 0x0000) == 0x0000
    assert run("v_min_f16", 0x7E00, 0x3C00) == run("v_max_f16", 0x3C00, 0x7E00) == 0x3C00
    assert run("v_min_f16", 0x7E00, 0xFE01) == 0x7E00
    assert run("v_min_f16", 0xFC00, 0x7C00) == 0xFC00
    assert run("v_max_f16", 0xBC00, 0x3C00) == 0x3C00


def test_ldexp_takes_a_signed_16_bit_exponent():
    expected = {1: 0x4000, 0xFFFF: 0x3800, 16: 0x7C00, 0xFFE8: 0x0001, 0xFFE7: 0x0000, 0x7FFF: 0x7C00, 0x8000: 0x0000}
    for exponent, bits in expected.items():
        assert run("v_ldexp_f16", 0x3C00, exponent) == bits, hex(exponent)
    assert run("v_ldexp_f16", 0x3E00, 0xFFE7) == 0x0001           # 1,5 × 2^-25
    assert run("v_ldexp_f16", 0x0001, 0x0001_0001) == 0x0002      # bits 31:16 ignorés


def test_conversions_between_f16_and_f32():
    to_half = {float_bits(1.0): 0x3C00, 0x477F_F000: 0x7C00, 0x477F_EFFF: 0x7BFF, 0x3F80_1000: 0x3C00,
               0x3F80_3000: 0x3C02, 0x0000_0001: 0x0000, 0x8000_0001: 0x8000, 0x33C0_0000: 0x0002,
               0x3300_0000: 0x0000, 0x3300_0001: 0x0001, 0x387F_FFFF: 0x0400, 0x7FC0_0001: 0x7E00,
               0xFF80_0000: 0xFC00, 0xC77F_E000: 0xFBFF}
    for word, bits in to_half.items():
        assert run("v_cvt_f16_f32", word) == bits, hex(word)
    to_word = {0x3C00: 0x3F80_0000, 0x0001: 0x3380_0000, 0x8000: 0x8000_0000, 0x7C00: 0x7F80_0000,
               0x7C01: 0x7FC0_0000, 0xFE00: 0x7FC0_0000}
    for bits, word in to_word.items():
        assert run("v_cvt_f32_f16", bits) == word, hex(bits)


def test_clamp_to_the_unit_interval():
    assert [f16.half_clamp(h) for h in (0x7E00, 0x8000, 0xBC00, 0x4000, 0x3800, 0x7C00)] == \
        [0, 0, 0, 0x3C00, 0x3800, 0x3C00]


def execute(*words, v=None, s=None):
    wave = rdna2_exec.Wave()
    for register, word in (v or {}).items():
        wave.v[register] = [word] * 32
    for register, word in (s or {}).items():
        wave.s[register] = word
    rdna2_exec.run(program(*words), wave)
    return wave


def test_vop2_writes_the_low_half_and_keeps_the_high_half():
    wave = execute(vop2("v_add_f16", 3, V + 1, 2), v={1: 0xAAAA_3C00, 2: 0x5555_4000, 3: OLD})
    assert wave.v[3][0] == 0x1234_4200


def test_constants_literals_and_scalars_give_16_bit_operands():
    assert execute(vop2("v_add_f16", 3, ONE, 2), v={2: 0x4000, 3: OLD}).v[3][0] == 0x1234_4200
    assert execute(vop2("v_add_f16", 3, const(1), 2), v={2: 0x3C00, 3: OLD}).v[3][0] == 0x1234_3C00
    assert execute(vop2("v_add_f16", 3, LITERAL, 2), 0xDEAD_3C00, v={2: 0x3C00, 3: OLD}).v[3][0] == 0x1234_4000
    assert execute(vop2("v_add_f16", 3, 4, 2), v={2: 0x3800, 3: OLD}, s={4: 0xFFFF_3800}).v[3][0] == 0x1234_3C00


def test_every_inline_float_constant_reads_as_its_half():
    for code, value in rdna2_exec.INLINE_FLOATS.items():                    # constante + 0 : la constante en f16
        wave = execute(vop2("v_add_f16", 3, code, 2), v={3: OLD})
        assert wave.v[3][0] == 0x1234_0000 | f16.half_bits(value), code


def test_accumulate_constant_and_conversion_forms():
    seeds = {1: 0x7777_4000, 2: 0x6666_4200, 3: 0x9999_3C00}
    assert execute(vop2("v_fmac_f16", 3, V + 1, 2), v=seeds).v[3][0] == 0x9999_4700        # 2 × 3 + 1
    assert execute(vop2("v_fmamk_f16", 3, V + 1, 2), 0x4200, v={1: 0x4000, 2: 0x3C00, 3: OLD}).v[3][0] == 0x1234_4700
    assert execute(vop2("v_fmaak_f16", 3, V + 1, 2), 0x3C00, v={1: 0x4000, 2: 0x4200, 3: OLD}).v[3][0] == 0x1234_4700
    assert execute(vop2("v_ldexp_f16", 3, V + 1, 2), v={1: 0x3C00, 2: 0xFFFF_FFFF, 3: OLD}).v[3][0] == 0x1234_3800
    assert execute(vop1("v_cvt_f32_f16", 3, V + 1), v={1: 0xAAAA_0001, 3: OLD}).v[3][0] == 0x3380_0000
    assert execute(vop1("v_cvt_f16_f32", 3, V + 1), v={1: 0x3F80_0000, 3: OLD}).v[3][0] == 0x1234_3C00
    assert execute(vop1("v_cvt_f16_f32", 3, ONE), v={3: OLD}).v[3][0] == 0x1234_3C00            # 1.0 en f32


def test_lanes_outside_exec_keep_both_halves():
    wave = execute(sop1("s_mov_b32", EXEC_LO, LITERAL), 0x0000_FFFF, vop2("v_add_f16", 3, V + 1, 2),
                   v={1: 0x3C00, 2: 0x3C00, 3: OLD})
    assert wave.v[3][:16] == [0x1234_4000] * 16 and wave.v[3][16:] == [OLD] * 16


def test_destination_may_be_a_source():
    assert execute(vop2("v_add_f16", 1, V + 1, 2), v={1: 0xABCD_3C00, 2: 0x3C00}).v[1][0] == 0xABCD_4000
    assert execute(vop2("v_fmac_f16", 2, V + 2, 2), v={2: 0xABCD_4000}).v[2][0] == 0xABCD_4600     # 2 × 2 + 2
    assert execute(vop1("v_cvt_f16_f32", 5, V + 5), v={5: 0x3F80_0000}).v[5][0] == 0x3F80_3C00     # 1.0 lu en f32


@pytest.mark.parametrize("code", [0xE9, 0xEA, 0xF9, 0xFA])
def test_sdwa_and_dpp_forms_are_refused(code):
    with pytest.raises(rdna2_exec.Untranslated, match="v_add_f16.*SDWA/DPP"):
        execute(vop2("v_add_f16", 3, code, 2), 0)


def test_vop1_and_vop2_f16_encodings_are_executable():
    names = ["v_add_f16", "v_sub_f16", "v_subrev_f16", "v_mul_f16", "v_fmac_f16", "v_fmamk_f16", "v_fmaak_f16",
             "v_max_f16", "v_min_f16", "v_ldexp_f16"]
    assert all(rdna2_exec.executable("VOP2", name) for name in names)
    assert rdna2_exec.executable("VOP1", "v_cvt_f16_f32") and rdna2_exec.executable("VOP1", "v_cvt_f32_f16")


def test_op_sel_reads_and_writes_high_halves():
    wave = execute(*vop3("v_add_f16", 3, V + 1, V + 2, op_sel=0b1001), v={1: 0x4000_3C00, 2: 0x3C00, 3: OLD})
    assert wave.v[3][0] == 0x4200_5678                                   # 2.0 (haut) + 1.0, écrit en haut
    assert execute(*vop3("v_ldexp_f16", 3, V + 1, V + 2, op_sel=0b0010),
                   v={1: 0x3C00, 2: 0x0002_0000, 3: OLD}).v[3][0] == 0x1234_4400
    assert execute(*vop3("v_cvt_f32_f16", 3, V + 1, op_sel=0b0001), v={1: 0x3C00_0000, 3: OLD}).v[3][0] == 0x3F80_0000
    assert execute(*vop3("v_cvt_f16_f32", 3, V + 1, neg=0b001, op_sel=0b1000),
                   v={1: 0x3F80_0000, 3: OLD}).v[3][0] == 0xBC00_5678


def test_abs_neg_and_clamp_act_on_the_half():
    seeds = {1: 0x0000_3C00, 2: 0x0000_C000, 3: OLD}
    assert execute(*vop3("v_add_f16", 3, V + 1, V + 2, neg=0b01, abs_=0b10), v=seeds).v[3][0] == 0x1234_3C00
    assert execute(*vop3("v_add_f16", 3, V + 1, V + 1, clamp=1), v=seeds).v[3][0] == 0x1234_3C00        # 2 → 1
    assert execute(*vop3("v_add_f16", 3, V + 1, V + 2, clamp=1), v=seeds).v[3][0] == 0x1234_0000        # -1 → 0
    assert execute(*vop3("v_add_f16", 3, V + 1, V + 2, clamp=1), v={1: 0x7C00, 2: 0xFC00, 3: OLD}).v[3][0] == 0x1234_0000


def test_vop3_fma_literal_and_vop2_forms():
    assert execute(*vop3("v_fma_f16", 3, V + 1, V + 2, V + 4),
                   v={1: 0x101F, 2: 0x3BC4, 4: 0x3C00, 3: OLD}).v[3][0] == 0x1234_3C01
    assert execute(*vop3("v_add_f16", 3, V + 1, LITERAL), 0x1234_3800, v={1: 0x3800, 3: OLD}).v[3][0] == 0x1234_3C00
    assert execute(*vop3("v_fmamk_f16", 3, LITERAL, V + 2), 0x0000_4000, v={2: 0x3C00, 3: OLD}).v[3][0] == 0x1234_4500


REFUSED_VOP3 = [                                     # partagé avec tests/test_rdna2_hlsl_f16.py
    (vop3("v_add_f16", 3, V + 1, V + 2, omod=1), "v_add_f16.*omod"),
    (vop3("v_add_f16", 3, ONE, V + 2, op_sel=0b0001), "v_add_f16.*op_sel"),
    (vop3("v_fmac_f16", 3, V + 1, V + 2, op_sel=0b1000), "v_fmac_f16.*op_sel"),
    (vop3("v_cvt_f16_f32", 3, V + 1, op_sel=0b0001), "v_cvt_f16_f32.*op_sel"),
    (vop3("v_cvt_f32_f16", 3, V + 1, op_sel=0b1000), "v_cvt_f32_f16.*op_sel"),
    (vop3("v_cvt_f32_f16", 3, V + 1, clamp=1), "v_cvt_f32_f16.*clamp"),
    (vop3("v_ldexp_f16", 3, V + 1, V + 2, neg=0b010), "v_ldexp_f16.*neg"),
    (vop3("v_ldexp_f16", 3, V + 1, ONE), "v_ldexp_f16.*constante flottante"),
    (vop3("v_fmaak_f16", 3, V + 1, V + 2), "v_fmaak_f16.*littéral"),
]


@pytest.mark.parametrize("words, pattern", REFUSED_VOP3)
def test_unsourced_vop3_forms_are_refused(words, pattern):
    with pytest.raises(rdna2_exec.Untranslated, match=pattern):
        execute(*words)


def test_vop3_f16_encodings_are_executable():
    names = ["v_add_f16", "v_sub_f16", "v_subrev_f16", "v_mul_f16", "v_fmac_f16", "v_fmamk_f16", "v_fmaak_f16",
             "v_max_f16", "v_min_f16", "v_ldexp_f16", "v_cvt_f16_f32", "v_cvt_f32_f16", "v_fma_f16"]
    assert all(rdna2_exec.executable("VOP3", name) for name in names)
