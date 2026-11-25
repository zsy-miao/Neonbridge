"""Interpréteur de référence RDNA 2 en wave64 (issue #70) : masques de 64 lignes sur deux SGPR.

Attendus calculés à la main d'après la documentation RDNA 2 d'AMD (chapitre 3 : exec_hi = s127,
vcc_hi = s107 ; v_mbcnt_lo/hi, v_readlane SSRC1[5:0]) ; ds_bpermute_b32 permute dans chaque moitié.
"""
import pytest

from emu import rdna2_exec
from test_rdna2_exec import V, const, program, sop1, sop2, sopc, sopp, vop1, vop2, vop3, vopc
from test_rdna2_exec_memory import ds

VCC_LO, VCC_HI, EXEC_LO, EXEC_HI, MINUS_ONE = 106, 107, 126, 127, 193
LANES = 64
ALL = (1 << LANES) - 1


def run64(*words):
    return rdna2_exec.run(program(*words), rdna2_exec.Wave(lanes=LANES))


def test_wave_size_defaults_to_32_and_rejects_other_sizes():
    wave32, wave64 = rdna2_exec.Wave(), rdna2_exec.Wave(lanes=64)
    assert (wave32.s[EXEC_LO], wave32.s[EXEC_HI], len(wave32.v[0])) == (0xFFFF_FFFF, 0, 32)
    assert (wave64.s[EXEC_LO], wave64.s[EXEC_HI], wave64.exec) == (0xFFFF_FFFF, 0xFFFF_FFFF, ALL)
    assert wave64.v[0] == list(range(64))
    with pytest.raises(ValueError, match="16"):
        rdna2_exec.Wave(lanes=16)


def test_compare_writes_a_64_bit_mask_into_vcc_and_an_sgpr_pair():
    wave = run64(
        vopc("v_cmp_lt_u32", const(40), 0),                     # vcc : lignes 41 à 63
        *vop3("v_cmp_gt_u32", 4, const(3), V + 0))              # s[4:5] : lignes 0 à 2
    assert (wave.s[VCC_LO], wave.s[VCC_HI]) == (0, 0xFFFF_FE00)
    assert (wave.s[4], wave.s[5]) == (0b111, 0)


def test_saveexec_b64_divergence_spans_both_halves():
    wave = run64(
        vop2("v_and_b32", 1, const(1), 0),                      # v1 = ligne & 1
        vopc("v_cmp_eq_u32", const(1), 1),                      # vcc : lignes impaires
        sop1("s_and_saveexec_b64", 2, VCC_LO),
        vop1("v_mov_b32", 2, const(7)),
        sop2("s_or_b64", EXEC_LO, EXEC_LO, 2))                  # exec restauré
    assert wave.v[2] == [7 * (lane & 1) for lane in range(64)]
    assert (wave.s[2], wave.s[3]) == (0xFFFF_FFFF, 0xFFFF_FFFF) and wave.exec == ALL


def test_cmpx_and_execz_see_the_high_half():
    wave = run64(
        vopc("v_cmpx_le_u32", const(50), 0),                    # exec : lignes 50 à 63, exec_lo = 0
        sopp("s_cbranch_execz", 1),                             # non pris : exec_hi n'est pas nul
        vop1("v_mov_b32", 3, const(9)),
        sop1("s_mov_b32", 8, const(1)))
    assert (wave.s[EXEC_LO], wave.s[EXEC_HI]) == (0, 0xFFFC_0000)
    assert wave.v[3] == [0] * 50 + [9] * 14
    assert wave.s[8] == 1


def test_vccz_branch_is_taken_only_when_both_halves_are_zero():
    wave = run64(
        vopc("v_cmp_eq_u32", const(63), 0),                     # vcc : ligne 63 seulement
        sopp("s_cbranch_vccz", 1),
        sop1("s_mov_b32", 8, const(1)),                         # exécutée : vcc_hi = 0x80000000
        vopc("v_cmp_eq_u32", const(64), 0),                     # vcc = 0
        sopp("s_cbranch_vccz", 1),
        sop1("s_mov_b32", 9, const(1)))                         # sautée
    assert (wave.s[8], wave.s[9]) == (1, 0)


def test_mbcnt_lo_hi_give_the_lane_number_and_counts_below():
    wave = run64(
        *vop3("v_mbcnt_lo_u32_b32", 1, MINUS_ONE, const(0)),
        *vop3("v_mbcnt_hi_u32_b32", 1, MINUS_ONE, V + 1),       # v1 = numéro de ligne 0..63
        sop1("s_mov_b32", 4, const(0b101)),                     # masque : lignes 0, 2, 32, 34, 63
        sop1("s_brev_b32", 5, const(1)),
        sop2("s_or_b32", 5, 5, const(0b101)),
        *vop3("v_mbcnt_lo_u32_b32", 2, 4, const(0)),
        *vop3("v_mbcnt_hi_u32_b32", 2, 5, V + 2))               # v2 = bits du masque sous la ligne
    assert wave.v[1] == list(range(64))
    bits = {0, 2, 32, 34, 63}
    assert wave.v[2] == [sum(1 for bit in bits if bit < lane) for lane in range(64)]
    assert wave.v[2][33] == 3 and wave.v[2][63] == 4


def test_readlane_writelane_and_readfirstlane_use_64_lanes():
    wave = run64(
        vop2("v_lshlrev_b32", 1, const(1), 0),                  # v1 = 2 × ligne
        *vop3("v_readlane_b32", 4, V + 1, const(50)),           # s4 = 100
        sop1("s_mov_b32", 6, const(61)),
        *vop3("v_writelane_b32", 1, const(5), 6),               # v1[61] = 5
        vopc("v_cmpx_lt_u32", const(36), 0),                    # exec : lignes 37 à 63
        vop1("v_readfirstlane_b32", 5, V + 1))                  # s5 = v1[37] = 74
    assert (wave.s[4], wave.s[5]) == (100, 74)
    assert wave.v[1][61] == 5 and wave.v[1][60] == 120


def test_readlane_lane_number_wraps_at_64():
    wave = run64(
        sop1("s_mov_b32", 6, const(64)),
        sop2("s_add_u32", 6, 6, const(40)),                     # s6 = 104 : ligne 104 & 63 = 40
        *vop3("v_readlane_b32", 4, V + 0, 6))
    assert wave.s[4] == 40


def test_carry_masks_and_cndmask_span_both_halves():
    wave = run64(
        vop1("v_mov_b32", 1, MINUS_ONE),                        # 0xFFFFFFFF
        vopc("v_cmp_gt_u32", const(32), 0),                     # vcc : lignes 0 à 31
        vop2("v_cndmask_b32", 2, const(1), 1),
        vop2("v_add_co_ci_u32", 3, const(0), 2),
        *vop3("v_add_co_u32", 4, V + 2, const(1), sdst=10))     # retenue là où v2 = 0xFFFFFFFF
    assert wave.v[2] == [0xFFFF_FFFF] * 32 + [1] * 32           # vcc → v1 (lignes 0-31), sinon 1
    assert wave.v[3] == [0] * 32 + [1] * 32                     # 0 + v2 + retenue d'entrée
    assert (wave.s[VCC_LO], wave.s[VCC_HI]) == (0xFFFF_FFFF, 0)  # retenue de sortie : lignes 0 à 31
    assert (wave.s[10], wave.s[11]) == (0xFFFF_FFFF, 0)


def test_vop3_cndmask_reads_its_selector_from_an_sgpr_pair():
    wave = run64(
        sop1("s_mov_b32", 4, const(0)),
        sop1("s_mov_b32", 5, const(1)),                         # s[4:5] : ligne 32 seulement
        *vop3("v_cndmask_b32", 1, const(3), const(8), 4))
    assert wave.v[1] == [3] * 32 + [8] + [3] * 31


def test_ds_bpermute_permutes_within_each_half():
    wave = run64(
        vop2("v_lshlrev_b32", 1, const(2), 0),                  # adresse = 4 × ligne
        vop2("v_add_nc_u32", 1, const(4), 1),                   # lit la ligne suivante
        *ds("ds_bpermute_b32", addr=1, data0=0, vdst=2))
    assert wave.v[2] == [(lane + 1) % 32 + (lane & 32) for lane in range(64)]


def test_scalar_mask_reductions_on_a_64_bit_ballot():
    wave = run64(
        vopc("v_cmp_lt_u32", const(30), 0),                     # vcc : lignes 31 à 63
        sop1("s_bcnt1_i32_b64", 4, VCC_LO),
        sop1("s_ff1_i32_b64", 5, VCC_LO),
        sopc("s_cmp_lg_u64", VCC_LO, 128),
        sop2("s_cselect_b32", 6, const(1), const(2)))
    assert (wave.s[4], wave.s[5], wave.s[6]) == (33, 31, 1)


def test_wave32_programs_keep_the_high_registers_untouched():
    wave = rdna2_exec.run(program(vopc("v_cmp_gt_u32", const(4), 0), vopc("v_cmpx_lt_u32", const(1), 0)))
    assert (wave.s[VCC_LO], wave.s[VCC_HI], wave.s[EXEC_LO], wave.s[EXEC_HI]) == (0xF, 0, 0xFFFF_FFFC, 0)


def test_null_reads_as_zero_on_64_bits():
    wave = rdna2_exec.Wave(lanes=LANES)
    assert wave.read_scalar(125, None, 2) == 0 and wave.read_mask(125) == 0     # et non exec_lo << 32
