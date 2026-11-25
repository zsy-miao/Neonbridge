"""Instructions scalaires de bits, de champs et d'EXEC (emu/rdna2_exec_bits.py).

Les valeurs attendues sont calculées à la main d'après le pseudo-code de la documentation
publique d'AMD (« RDNA 2 Instruction Set Architecture », chapitre 5), pas par l'interpréteur.
"""
import pytest

from emu import rdna2_exec, rdna2_gpu, rdna2_hlsl
from test_rdna2_exec import EXEC_LO, const, program, sop1, sop2, sopc, vop1
from test_rdna2_hlsl import compare, store_lane, with_lane_offset

LITERAL = 255
gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc ou anyxboxx_gpu_run.exe absent")


def run(*words):
    return rdna2_exec.run(program(*words))


def literal(register, value):
    return sop1("s_mov_b32", register, LITERAL), value & 0xFFFF_FFFF


def literal64(register, value):
    return (*literal(register, value), *literal(register + 1, value >> 32))


def scc_into(register):
    return sop2("s_cselect_b32", register, const(1), const(0))


def pair(wave, register):
    return wave.s[register] | wave.s[register + 1] << 32


def test_bfe_u32_takes_offset_and_seven_bit_width():
    wave = run(
        *literal(40, 0xDEAD_BEEF),
        sop2("s_bfe_u32", 41, 40, LITERAL), 8 << 16 | 4, scc_into(42),
        sop2("s_bfe_u32", 43, 40, const(4)), scc_into(44),              # largeur 0
        sop2("s_bfe_u32", 45, 40, LITERAL), 40 << 16 | 28)              # largeur 40 ≥ 32
    assert (wave.s[41], wave.s[42]) == (0xEE, 1)
    assert (wave.s[43], wave.s[44]) == (0, 0)
    assert wave.s[45] == 0xD


def test_bfe_i32_sign_extends_the_field():
    wave = run(
        *literal(40, 0xDEAD_BEEF),
        sop2("s_bfe_i32", 41, 40, LITERAL), 8 << 16 | 4,                # 0xEE → -18
        sop2("s_bfe_i32", 42, 40, LITERAL), 4 << 16 | 28,               # 0xD → -3
        sop2("s_bfe_i32", 43, 40, LITERAL), 40 << 16 | 28)              # décalage arithmétique
    assert wave.s[41:44] == [0xFFFF_FFEE, 0xFFFF_FFFD, 0xFFFF_FFFD]


def test_64_bit_fields_masks_and_shifts():
    wave = run(
        *literal64(40, 0x1234_5678_9ABC_DEF0),
        sop2("s_bfe_u64", 42, 40, LITERAL), 16 << 16 | 36,
        *literal64(44, 0xF000_0000_0000_0000),
        sop2("s_bfe_i64", 46, 44, LITERAL), 8 << 16 | 56,
        sop2("s_bfm_b64", 48, const(40), const(8)),
        *literal64(50, 1 << 63),
        sop2("s_ashr_i64", 52, 50, const(63)), scc_into(54))
    assert pair(wave, 42) == 0x4567
    assert pair(wave, 46) == 0xFFFF_FFFF_FFFF_FFF0
    assert pair(wave, 48) == 0x0000_FFFF_FFFF_FF00     # bits 8 à 47
    assert (pair(wave, 52), wave.s[54]) == ((1 << 64) - 1, 1)


def test_absdiff_wraps_like_32_bit_hardware():
    wave = run(
        sop2("s_absdiff_i32", 40, const(3), const(10)),
        *literal(41, 0x7FFF_FFFF),
        sop2("s_absdiff_i32", 42, 41, 193), scc_into(43))               # 193 : constante -1
    assert wave.s[40] == 7
    assert (wave.s[42], wave.s[43]) == (0x8000_0000, 1)


def test_shift_and_add_carry_counts_bits_shifted_out():
    wave = run(
        *literal(40, 0x4000_0001),
        sop2("s_lshl2_add_u32", 41, 40, const(1)), scc_into(42),
        sop2("s_lshl1_add_u32", 43, const(1), const(2)), scc_into(44),
        sop2("s_lshl3_add_u32", 45, const(1), const(1)),
        sop2("s_lshl4_add_u32", 46, const(1), const(1)))
    assert (wave.s[41], wave.s[42]) == (5, 1)
    assert (wave.s[43], wave.s[44]) == (4, 0)
    assert (wave.s[45], wave.s[46]) == (9, 17)


def test_multiply_high_halves():
    wave = run(
        *literal(40, 0xFFFF_FFFF), *literal(41, 0x8000_0000),
        sop2("s_mul_hi_u32", 42, 40, 40),
        sop2("s_mul_hi_i32", 43, 40, 40),                               # (-1)(-1) = 1
        sop2("s_mul_hi_i32", 44, 41, const(2)))                         # -2^31 × 2 = -2^32
    assert wave.s[42:45] == [0xFFFF_FFFE, 0, 0xFFFF_FFFF]


def test_64_bit_logic_and_bit_compare():
    wave = run(
        *literal64(40, 0x0000_0100_0000_000F),
        sop2("s_nand_b64", 42, 40, 40),
        sop2("s_xnor_b64", 44, 40, 40),
        sop2("s_nor_b64", 46, 40, 40),
        sop2("s_orn2_b64", 48, 40, 40),
        sopc("s_bitcmp1_b64", 40, const(40)), scc_into(50),
        sopc("s_bitcmp1_b64", 40, const(8)), scc_into(51),
        sopc("s_bitcmp0_b64", 40, const(8)), scc_into(52))
    assert pair(wave, 42) == 0xFFFF_FEFF_FFFF_FFF0
    assert pair(wave, 44) == (1 << 64) - 1
    assert pair(wave, 46) == 0xFFFF_FEFF_FFFF_FFF0
    assert pair(wave, 48) == (1 << 64) - 1
    assert wave.s[50:53] == [1, 0, 1]


def test_quad_masks_and_bit_replication():
    wave = run(
        *literal(40, 0x0000_1020),
        sop1("s_wqm_b32", 41, 40), scc_into(42),
        sop1("s_quadmask_b32", 43, 40),
        *literal64(44, 0x0001_0000_0000_0008),
        sop1("s_wqm_b64", 46, 44),
        sop1("s_quadmask_b64", 48, 44),
        *literal(50, 0x8000_0001),
        sop1("s_bitreplicate_b64_b32", 52, 50))
    assert (wave.s[41], wave.s[42]) == (0x0000_F0F0, 1)
    assert wave.s[43] == 0b1010
    assert pair(wave, 46) == 0x000F_0000_0000_000F
    assert pair(wave, 48) == 1 << 12 | 1
    assert pair(wave, 52) == 0xC000_0000_0000_0003


def test_bit_counts_and_searches():
    wave = run(
        sop1("s_bcnt0_i32_b32", 40, const(15)),
        *literal64(42, 0x0000_00F0_0000_000F),
        sop1("s_bcnt1_i32_b64", 44, 42),
        sop1("s_bcnt0_i32_b64", 45, 42),
        sop1("s_ff0_i32_b32", 46, LITERAL), 0xFF,
        sop1("s_ff0_i32_b32", 47, 193),                                 # -1 : aucun zéro
        *literal64(48, 0x0000_0010_0000_0000),
        sop1("s_ff1_i32_b64", 50, 48),
        *literal64(52, 0xFFFF_FFFE_FFFF_FFFF),
        sop1("s_ff0_i32_b64", 54, 52),
        sop1("s_flbit_i32_b64", 55, const(1)))
    assert wave.s[40] == 28
    assert (wave.s[44], wave.s[45]) == (8, 56)
    assert (wave.s[46], wave.s[47]) == (8, 0xFFFF_FFFF)
    assert (wave.s[50], wave.s[54], wave.s[55]) == (36, 32, 63)


def test_signed_leading_bit_search():
    wave = run(
        sop1("s_flbit_i32", 40, LITERAL), 0x0000_FFFF,
        sop1("s_flbit_i32", 41, LITERAL), 0xFFFF_0000,
        sop1("s_flbit_i32", 42, 193),                                   # -1
        sop1("s_flbit_i32", 43, const(0)),
        sop1("s_flbit_i32", 44, LITERAL), 0x4000_0000,
        *literal64(46, 0xFFFF_FFFF_7FFF_FFFF),
        sop1("s_flbit_i32_i64", 48, 46))
    assert wave.s[40:45] == [16, 16, 0xFFFF_FFFF, 0xFFFF_FFFF, 1]
    assert wave.s[48] == 32


def test_bitset_reads_and_rewrites_the_destination():
    wave = run(
        sop1("s_mov_b32", 40, LITERAL), 0xFF,
        sop1("s_bitset0_b32", 40, const(3)),
        sop1("s_bitset1_b32", 40, const(31)),
        *literal64(42, 0),
        sop1("s_bitset1_b64", 42, const(40)),
        *literal64(44, (1 << 64) - 1),
        sop1("s_bitset0_b64", 44, const(63)))
    assert wave.s[40] == 0x8000_00F7
    assert pair(wave, 42) == 1 << 40
    assert pair(wave, 44) == (1 << 63) - 1


EXEC_BEFORE, SOURCE = 0xFFFF_0000, 0x0F0F_0F0F
EXEC_CASES = [
    ("s_orn2_saveexec_b32", 0x0F0F_FFFF, EXEC_BEFORE), ("s_nand_saveexec_b32", 0xF0F0_FFFF, EXEC_BEFORE),
    ("s_nor_saveexec_b32", 0x0000_F0F0, EXEC_BEFORE), ("s_xnor_saveexec_b32", 0x0F0F_F0F0, EXEC_BEFORE),
    ("s_andn1_saveexec_b32", 0xF0F0_0000, EXEC_BEFORE), ("s_orn1_saveexec_b32", 0xFFFF_F0F0, EXEC_BEFORE),
    ("s_andn1_wrexec_b32", 0xF0F0_0000, 0xF0F0_0000), ("s_andn2_wrexec_b32", 0x0000_0F0F, 0x0000_0F0F),
]


@pytest.mark.parametrize("name, exec_after, saved", EXEC_CASES)
def test_exec_mask_variants(name, exec_after, saved):
    wave = run(*literal(EXEC_LO, EXEC_BEFORE), *literal(40, SOURCE), sop1(name, 41, 40), scc_into(42))
    assert (wave.exec, wave.s[41], wave.s[42]) == (exec_after, saved, 1)


def test_64_bit_saveexec_uses_exec_hi():
    wave = run(
        *literal64(EXEC_LO, 1 << 32 | EXEC_BEFORE), *literal64(40, SOURCE),
        sop1("s_andn1_saveexec_b64", 42, 40))
    assert pair(wave, EXEC_LO) == 1 << 32 | 0xF0F0_0000
    assert pair(wave, 42) == 1 << 32 | EXEC_BEFORE


def scalar_parity_program():
    """Instructions 32 bits de ce module, résultats copiés dans des VGPR puis en mémoire."""
    ops = [
        sop2("s_bfe_u32", 40, 60, LITERAL), 8 << 16 | 4,
        sop2("s_bfe_i32", 41, 60, LITERAL), 40 << 16 | 28,
        sop2("s_absdiff_i32", 42, 61, 193),
        sop2("s_lshl2_add_u32", 43, 61, const(5)), scc_into(44),
        sop2("s_lshl4_add_u32", 45, 60, const(1)), scc_into(46),
        sop2("s_mul_hi_u32", 47, 60, 61),
        sop2("s_mul_hi_i32", 48, 60, 61),
        sop1("s_wqm_b32", 49, 62),
        sop1("s_quadmask_b32", 50, 62),
        sop1("s_bcnt0_i32_b32", 51, 60),
        sop1("s_ff0_i32_b32", 52, 62),
        sop1("s_flbit_i32", 53, 60),
        sop1("s_mov_b32", 54, 62), sop1("s_bitset0_b32", 54, const(5)), sop1("s_bitset1_b32", 54, const(31)),
        sop1("s_xnor_saveexec_b32", 55, 63), sop1("s_mov_b32", EXEC_LO, 55),
        sop1("s_andn1_wrexec_b32", 56, 63), sop1("s_mov_b32", EXEC_LO, LITERAL), 0xFFFF_FFFF,
    ]
    stores = []
    for slot, register in enumerate(range(40, 57)):
        stores += [vop1("v_mov_b32", 1, register), *store_lane(1, slot)]
    return with_lane_offset(
        *literal(60, 0xDEAD_BEEF), *literal(61, 0x7FFF_FFFF), *literal(62, 0x0000_1020), *literal(63, 0x0F0F_00FF),
        *ops, *stores)


def test_hlsl_translates_the_32_bit_scalar_operations():
    hlsl = rdna2_hlsl.translate(scalar_parity_program())
    for token in ("SBfeU32(", "SBfeI32(", "WqmB32(", "QuadMaskB32(", "LeadingSignsI32(", "MulHiI32("):
        assert token in hlsl


@gpu
def test_32_bit_scalar_operations_match_interpreter_on_gpu():
    expected, actual = compare(scalar_parity_program())
    assert actual == expected



def test_signed_64_bit_sources_sign_extend_a_32_bit_literal():
    """ISA section 6.2.1 (p. 43): a literal is sign-extended for a signed 64-bit source, zero-extended else."""
    wave = run(
        sop2("s_ashr_i64", 40, LITERAL, const(1)), 0x8000_0000, scc_into(50),
        sop2("s_ashr_i64", 42, LITERAL, const(4)), 0x7FFF_FFFF,
        *literal(60, 40 << 16 | 4),                                     # décalage 4, largeur 40
        sop2("s_bfe_i64", 44, LITERAL, 60), 0x8000_0000,
        sop2("s_bfe_u64", 46, LITERAL, 60), 0x8000_0000,                # non signé : zéro-étendu
        sop1("s_flbit_i32_i64", 51, LITERAL), 0xFFFF_FFF0,
        sop1("s_flbit_i32_b64", 52, LITERAL), 0xFFFF_FFF0,              # bits : zéro-étendu
        sop1("s_mov_b64", 48, LITERAL), 0x8000_0000)                    # b64 : zéro-étendu
    assert pair(wave, 40) == 0xFFFF_FFFF_C000_0000 and wave.s[50] == 1
    assert pair(wave, 42) == 0x07FF_FFFF
    assert pair(wave, 44) == 0xFFFF_FFFF_F800_0000
    assert pair(wave, 46) == 0x0800_0000
    assert wave.s[51] == 60                                             # bit 3 : premier bit différent du signe
    assert wave.s[52] == 32
    assert pair(wave, 48) == 0x8000_0000
