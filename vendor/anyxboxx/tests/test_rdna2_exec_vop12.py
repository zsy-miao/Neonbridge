"""Instructions VOP1/VOP2 ajoutées à l'interpréteur : conversions, frexp, legacy, madmk/madak,
forme VOP2 des additions avec retenue (VCC) et instructions sans effet.

Valeurs attendues calculées à la main d'après la documentation publique d'AMD (« RDNA 2
Instruction Set Architecture », chapitre 6), pas par l'interpréteur.
"""
import pytest

from emu import rdna2_exec, rdna2_gpu, rdna2_hlsl
from emu.rdna2_exec_ops import bits_float, float_bits
from test_rdna2_exec import EXEC_LO, VCC_LO, V, const, program, sop1, vop1, vop2, vop3
from test_rdna2_hlsl import compare, store_lane, with_lane_offset

LITERAL = 255
gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc ou anyxboxx_gpu_run.exe absent")


def run(*words):
    return rdna2_exec.run(program(*words))


def mov(vdst, value):
    return vop1("v_mov_b32", vdst, LITERAL), value & 0xFFFF_FFFF


def movf(vdst, value):
    return mov(vdst, float_bits(value))


def lane0(wave, register):
    return wave.v[register][0]


@pytest.mark.parametrize("name, value, expected", [
    ("v_cvt_flr_i32_f32", -1.5, -2), ("v_cvt_flr_i32_f32", 2.7, 2),
    ("v_cvt_rpi_i32_f32", 2.5, 3), ("v_cvt_rpi_i32_f32", -2.5, -2), ("v_cvt_rpi_i32_f32", 1.4, 1),
])
def test_float_to_int_with_rounding_mode(name, value, expected):
    wave = run(*movf(1, value), vop1(name, 2, V + 1))
    assert lane0(wave, 2) == expected & 0xFFFF_FFFF


def test_nan_follows_its_sign_bit():
    wave = run(*mov(1, 0x7FC0_0000), *mov(2, 0xFFC0_0000),
               vop1("v_cvt_flr_i32_f32", 3, V + 1), vop1("v_cvt_rpi_i32_f32", 4, V + 2),
               vop1("v_cvt_i32_f32", 5, V + 1))
    assert (lane0(wave, 3), lane0(wave, 4), lane0(wave, 5)) == (0x7FFF_FFFF, 0x8000_0000, 0)


@pytest.mark.parametrize("nibble, expected", [(8, -0.5), (7, 0.4375), (0xF, -0.0625), (0x10, 0.0)])
def test_signed_nibble_offset_to_float(nibble, expected):
    wave = run(vop1("v_cvt_off_f32_i4", 2, LITERAL), nibble)
    assert bits_float(lane0(wave, 2)) == expected


def test_reciprocal_with_integer_flag():
    wave = run(*movf(1, 4.0), vop1("v_rcp_iflag_f32", 2, V + 1))
    assert bits_float(lane0(wave, 2)) == 0.25


@pytest.mark.parametrize("value, expected", [(0x0000_FFFF, 16), (0xFFFF_0000, 16), (0xFFFF_FFFF, -1),
                                             (0, -1), (0x4000_0000, 1)])
def test_signed_leading_bit_search(value, expected):
    wave = run(*mov(1, value), vop1("v_ffbh_i32", 2, V + 1))
    assert lane0(wave, 2) == expected & 0xFFFF_FFFF


@pytest.mark.parametrize("bits, exponent, mantissa", [
    (float_bits(8.0), 4, float_bits(0.5)),
    (float_bits(-3.0), 2, float_bits(-0.75)),
    (float_bits(0.75), 0, float_bits(0.75)),
    (0x0000_0001, -148, float_bits(0.5)),               # plus petit dénormal : 2^-149
    (0x7F80_0000, 0, 0x7F80_0000),                      # +infini rendu tel quel, exposant 0
    (0x8000_0000, 0, 0x8000_0000),                      # -0
])
def test_frexp_splits_mantissa_and_exponent(bits, exponent, mantissa):
    wave = run(*mov(1, bits), vop1("v_frexp_exp_i32_f32", 2, V + 1), vop1("v_frexp_mant_f32", 3, V + 1))
    assert lane0(wave, 2) == exponent & 0xFFFF_FFFF
    assert lane0(wave, 3) == mantissa


def test_legacy_multiply_accumulate_ignores_nan_times_zero():
    wave = run(
        *movf(1, 5.0), *mov(2, 0x7FC0_0000),
        vop2("v_fmac_legacy_f32", 1, const(0), 2),          # 0 × NaN = 0 : v1 reste 5.0
        *movf(3, 1.0), *movf(4, 3.0),
        vop2("v_fmac_legacy_f32", 3, LITERAL, 4), float_bits(2.0))   # 2 × 3 + 1
    assert bits_float(lane0(wave, 1)) == 5.0
    assert bits_float(lane0(wave, 3)) == 7.0


def test_multiply_add_with_literal_constant():
    wave = run(
        *movf(1, 2.0), *movf(2, 1.0), *movf(3, 3.0),
        vop2("v_madmk_f32", 4, V + 1, 2), float_bits(3.0),      # 2 × K + 1
        vop2("v_madak_f32", 5, V + 1, 3), float_bits(1.0))      # 2 × 3 + K
    assert bits_float(lane0(wave, 4)) == 7.0
    assert bits_float(lane0(wave, 5)) == 7.0


def test_multiply_high_of_signed_24_bit_values():
    wave = run(
        *mov(1, 0x0080_0000), *mov(2, 0x007F_FFFF),
        vop2("v_mul_hi_i32_i24", 3, V + 1, 2),              # -2^23 × (2^23 - 1)
        vop2("v_mul_hi_i32_i24", 4, V + 2, 2))              # (2^23 - 1)²
    assert lane0(wave, 3) == 0xFFFF_C000
    assert lane0(wave, 4) == 0x3FFF


def test_vop2_add_with_carry_builds_a_64_bit_sum():
    wave = run(
        *mov(1, 0xFFFF_FFFF), *mov(5, 0x10),
        *vop3("v_add_co_u32", 3, V + 1, V + 0, sdst=VCC_LO),    # retenue sur les lignes 1 à 31
        vop2("v_add_co_ci_u32", 4, const(0), 5))
    assert wave.v[3] == [0xFFFF_FFFF] + list(range(31))
    assert wave.v[4] == [0x10] + [0x11] * 31
    assert wave.vcc == 0


def test_carry_out_of_inactive_lanes_is_zero():
    """Revue de #133 : EXEC = 0b0001, VCC = 0xFFFFFFFE. Les lignes inactives écrivent 0 dans le masque
    de retenue ; seule la ligne 0 compte. Même cas mesuré sur RDNA 2 (gfx1036, Wave32) en VOP2 et en
    VOP3, résultat 0x00000001 ; doc RDNA 2 section 3.9 (sources dans rdna2_exec._carry)."""
    wave = run(
        sop1("s_mov_b32", VCC_LO, LITERAL), 0xFFFF_FFFE, sop1("s_mov_b32", 40, LITERAL), 0xFFFF_FFFE,
        sop1("s_mov_b32", EXEC_LO, const(1)),
        *mov(1, 1),
        vop2("v_add_co_ci_u32", 2, LITERAL, 1), 0xFFFF_FFFF,               # 0xFFFFFFFF + 1 + 0 : retenue
        *vop3("v_add_co_ci_u32", 3, LITERAL, V + 1, 40, sdst=40), 0xFFFF_FFFF)
    assert wave.vcc == 0x0000_0001
    assert wave.s[40] == 0x0000_0001


def partial_exec_carry_program():
    return with_lane_offset(
        sop1("s_mov_b32", VCC_LO, LITERAL), 0xAAAA_AAAA,
        sop1("s_mov_b32", 40, LITERAL), 0x5555_5555,
        sop1("s_mov_b32", EXEC_LO, LITERAL), 0x00FF_FF00,
        vop2("v_add_co_ci_u32", 2, LITERAL, 0), 0xFFFF_FFF0,               # retenue sur les lignes ≥ 16
        *vop3("v_sub_co_ci_u32", 3, V + 0, LITERAL, 40, sdst=40), 8,
        sop1("s_mov_b32", EXEC_LO, LITERAL), 0xFFFF_FFFF,
        vop1("v_mov_b32", 4, VCC_LO), vop1("v_mov_b32", 5, 40),
        *store_lane(2, 0), *store_lane(3, 1), *store_lane(4, 2), *store_lane(5, 3))


@gpu
def test_carry_masks_with_partial_exec_match_on_gpu():
    expected, actual = compare(partial_exec_carry_program())
    assert actual == expected


def test_vop2_subtract_with_borrow_writes_vcc():
    wave = run(
        sop1("s_mov_b32", VCC_LO, 193),                     # emprunt entrant sur toutes les lignes
        vop2("v_sub_co_ci_u32", 6, const(0), 7),            # 0 - 0 - 1
        vop2("v_subrev_co_ci_u32", 8, const(0), 6))         # v6 - 0 - 1 (emprunt entrant), pas d'emprunt sortant
    assert wave.v[6] == [0xFFFF_FFFF] * 32
    assert wave.v[8] == [0xFFFF_FFFE] * 32
    assert wave.vcc == 0


def test_no_operation_instructions_leave_registers_alone():
    wave = run(vop1("v_nop", 0, 0), vop1("v_pipeflush", 0, 0), vop1("v_clrexcp", 0, 0))
    assert wave.v[0] == list(range(32))


def vector_parity_program():
    ops = [
        *movf(1, -2.5), *mov(2, 0x0000_0001), *movf(3, 5.0),
        vop1("v_cvt_flr_i32_f32", 4, V + 1),
        vop1("v_cvt_rpi_i32_f32", 5, V + 1),
        vop1("v_cvt_off_f32_i4", 6, V + 0),
        vop1("v_rcp_iflag_f32", 7, V + 3),
        vop1("v_ffbh_i32", 8, V + 0),
        vop1("v_frexp_exp_i32_f32", 9, V + 3),
        vop1("v_frexp_mant_f32", 10, V + 3),
        vop1("v_frexp_exp_i32_f32", 11, V + 2),
        vop1("v_frexp_mant_f32", 12, V + 2),
        vop2("v_mul_hi_i32_i24", 13, LITERAL, 0), 0x0080_0000,
        vop1("v_mov_b32", 14, V + 3), vop2("v_fmac_legacy_f32", 14, V + 1, 3),
        vop2("v_madmk_f32", 15, V + 1, 3), float_bits(3.0),
        vop2("v_madak_f32", 16, V + 1, 3), float_bits(1.0),
        *mov(17, 0xFFFF_FFFF), *vop3("v_add_co_u32", 18, V + 17, V + 0, sdst=VCC_LO),
        vop2("v_add_co_ci_u32", 19, const(7), 0),
        vop2("v_sub_co_ci_u32", 20, const(3), 0),
        vop1("v_nop", 0, 0),
    ]
    stores = []
    for slot, register in enumerate(range(4, 21)):
        stores += store_lane(register, slot)
    return with_lane_offset(*ops, *stores)


def test_hlsl_translates_the_new_vector_operations():
    hlsl = rdna2_hlsl.translate(vector_parity_program())
    for token in ("FrexpExpF32(", "FrexpMantF32(", "LeadingSignsI32(", "FmaLegacyF32(", "floor("):
        assert token in hlsl


SATURATING = [0x7F80_0000, 0xFF80_0000, 0x7FC0_0000, float_bits(3e9), float_bits(-3e9), float_bits(2.0 ** 31)]


@pytest.mark.parametrize("name", ["v_cvt_flr_i32_f32", "v_cvt_rpi_i32_f32"])
def test_rounding_conversions_saturate(name):
    wave = run(*(word for i, value in enumerate(SATURATING) for word in (*mov(1 + i, value),)),
               *(vop1(name, 10 + i, V + 1 + i) for i in range(len(SATURATING))))
    assert [lane0(wave, 10 + i) for i in range(len(SATURATING))] == [
        0x7FFF_FFFF, 0x8000_0000, 0x7FFF_FFFF, 0x7FFF_FFFF, 0x8000_0000, 0x7FFF_FFFF]


# Mesures sur silicium RDNA 2 (iGPU gfx1036, pilote AMD 32.0.21042.62) publiées par Lyrnc dans
# VIctory #605 (commentaire 7123302580), identiques dans les quatre FLOAT_MODE : entrée → (rpi, flr, i32).
HARDWARE = {
    0x3EFF_FFFF: (0x0000_0000, 0x0000_0000, 0x0000_0000), 0x3F00_0000: (0x0000_0001, 0x0000_0000, 0x0000_0000),
    0xBF00_0000: (0x0000_0000, 0xFFFF_FFFF, 0x0000_0000), 0xBF00_0001: (0xFFFF_FFFF, 0xFFFF_FFFF, 0x0000_0000),
    0x4B00_0001: (0x0080_0001, 0x0080_0001, 0x0080_0001), 0x4B00_0003: (0x0080_0003, 0x0080_0003, 0x0080_0003),
    0x4B7F_FFFF: (0x00FF_FFFF, 0x00FF_FFFF, 0x00FF_FFFF), 0xCB00_0001: (0xFF7F_FFFF, 0xFF7F_FFFF, 0xFF7F_FFFF),
    0x4F00_0000: (0x7FFF_FFFF, 0x7FFF_FFFF, 0x7FFF_FFFF), 0xCF00_0000: (0x8000_0000, 0x8000_0000, 0x8000_0000),
    0x7F80_0000: (0x7FFF_FFFF, 0x7FFF_FFFF, 0x7FFF_FFFF), 0xFF80_0000: (0x8000_0000, 0x8000_0000, 0x8000_0000),
    0x7FC0_0000: (0x7FFF_FFFF, 0x7FFF_FFFF, 0x0000_0000), 0xFFC0_0000: (0x8000_0000, 0x8000_0000, 0x0000_0000),
    0x7F80_0001: (0x7FFF_FFFF, 0x7FFF_FFFF, 0x0000_0000), 0xFFFF_FFFF: (0x8000_0000, 0x8000_0000, 0x0000_0000),
}
CONVERSIONS = ("v_cvt_rpi_i32_f32", "v_cvt_flr_i32_f32", "v_cvt_i32_f32")


@pytest.mark.parametrize("encoding", ["VOP1", "VOP3"])
def test_conversions_match_rdna2_hardware_measurements(encoding):
    for bits, results in HARDWARE.items():
        for name, expected in zip(CONVERSIONS, results):
            convert = vop1(name, 2, V + 1) if encoding == "VOP1" else vop3(name, 2, V + 1)
            got = lane0(run(*mov(1, bits), *([convert] if encoding == "VOP1" else convert)), 2)
            assert got == expected, f"{name} {bits:#010x} ({encoding}) : {got:#010x} au lieu de {expected:#010x}"


def hardware_program():
    words, slot = [], 0
    for bits in HARDWARE:
        for name in CONVERSIONS:
            words += [*mov(1, bits), vop1(name, 2, V + 1), *store_lane(2, slot % 64)]
            words += [*vop3(name, 3, V + 1), *store_lane(3, 64 + slot % 64)]
            slot += 1
    return with_lane_offset(*words)


@gpu
def test_conversions_match_hardware_measurements_on_gpu():
    expected, actual = compare(hardware_program())
    assert actual == expected


def saturating_program():
    words = []
    for slot, (name, value) in enumerate((n, v) for n in ("v_cvt_flr_i32_f32", "v_cvt_rpi_i32_f32")
                                         for v in SATURATING):
        words += [*mov(1, value), vop1(name, 2, V + 1), *store_lane(2, slot)]
    return with_lane_offset(*words)


@gpu
def test_rounding_conversions_saturate_on_gpu():
    expected, actual = compare(saturating_program())
    assert actual == expected


@gpu
def test_new_vector_operations_match_interpreter_on_gpu():
    expected, actual = compare(vector_parity_program())
    assert actual == expected
