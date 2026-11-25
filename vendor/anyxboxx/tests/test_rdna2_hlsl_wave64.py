"""Programmes wave64 recompilés en HLSL (issue #70) : un groupe de 64 threads est le wave virtuel.

Chaque programme tourne dans l'interpréteur de référence en wave64 et sur le GPU du PC (RTX : waves de
32 lignes) ou WARP ; les deux mémoires finales doivent être identiques au bit près.
"""
import random
import re
import struct

import pytest

from emu import rdna2_exec, rdna2_gpu, rdna2_hlsl
from test_rdna2_exec import V, const, program, sop1, sop2, sopc, sopp, vop1, vop2, vop3, vopc
from test_rdna2_exec_memory import BUFFER, INPUT, OUTPUT, ds, global_
from test_rdna2_hlsl import MEMORY_SIZE
from test_rdna2_sdwa_dpp_exec import DPP8, SDWA, dpp8_word, sdwa_word

VCC_LO, VCC_HI, EXEC_LO, EXEC_HI, MINUS_ONE = 106, 107, 126, 127, 193
LANES = 64
SLOT = LANES * 4
gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc ou anyxboxx_gpu_run.exe absent")


def store_lane(value_register, slot):
    """global_store_dword de v<value_register> à OUTPUT + slot × 256 + ligne × 4 (v31 = ligne × 4)."""
    return global_("global_store_dword", addr=31, data=value_register, saddr=2, offset=slot * SLOT)


def with_lane_offset(*words):
    return program(vop2("v_lshlrev_b32", 31, const(2), 0), *words)


def lane_values(seed):
    """64 valeurs pseudo-aléatoires fixes, une par ligne."""
    generator = random.Random(seed)
    return [generator.getrandbits(32) for _ in range(LANES)]


def compare(data, memory=b""):
    """Mémoire finale (interpréteur wave64, GPU) pour le même programme et le même état initial."""
    sgprs = {0: INPUT, 2: OUTPUT, 20: BUFFER, 22: 64 * 4}
    image = bytearray(MEMORY_SIZE)
    image[:len(memory)] = memory
    wave = rdna2_exec.Wave(lanes=LANES)
    wave.memory.write(0, bytes(image))
    for number, value in sgprs.items():
        wave.s[number] = value
    rdna2_exec.run(data, wave)
    expected = wave.memory.read(0, MEMORY_SIZE)
    return expected, rdna2_gpu.run(rdna2_hlsl.translate(data, sgprs, wave_size=LANES), bytes(image))


def output(memory, slot):
    return list(struct.unpack_from(f"<{LANES}I", memory, OUTPUT + slot * SLOT))


def input_memory(*seeds):
    memory = bytearray(INPUT + SLOT * len(seeds))
    for index, seed in enumerate(seeds):
        struct.pack_into(f"<{LANES}I", memory, INPUT + index * SLOT, *lane_values(seed))
    return bytes(memory)


def test_wave64_is_one_group_of_64_threads_without_wave_size():
    hlsl = rdna2_hlsl.translate(program(vop1("v_mov_b32", 1, const(7))), wave_size=64)
    assert "[numthreads(64, 1, 1)]" in hlsl and not re.search(r"\bWave[A-Z]", hlsl)    # ni WaveSize ni intrinsèque
    assert "s[127] = 0xFFFFFFFFu;" in hlsl and "if (LANE_ON) v[1] = 7u;" in hlsl
    assert "[numthreads(32, 1, 1)]" in rdna2_hlsl.translate(program(vop1("v_mov_b32", 1, const(7))))
    with pytest.raises(ValueError, match="16"):
        rdna2_hlsl.translate(program(), wave_size=16)


def test_cross_lane_forms_not_written_for_64_lanes_are_refused_by_name():
    dpp = program(vop1("v_mov_b32", 2, DPP8), dpp8_word(1, [7, 6, 5, 4, 3, 2, 1, 0]))
    with pytest.raises(rdna2_hlsl.Untranslatable, match="v_mov_b32.*wave64|wave64.*v_mov_b32"):
        rdna2_hlsl.translate(dpp, wave_size=64)
    sdwa_compare = program(vopc("v_cmp_eq_u32", SDWA, 1), sdwa_word(0))
    with pytest.raises(rdna2_hlsl.Untranslatable, match="v_cmp_eq_u32"):
        rdna2_hlsl.translate(sdwa_compare, wave_size=64)
    rdna2_hlsl.translate(dpp)                                   # toujours traduit en wave32


@gpu
def test_lane_varying_alu_and_memory_match():
    expected, actual = compare(with_lane_offset(
        *global_("global_load_dword", addr=31, vdst=1, saddr=0),
        *global_("global_load_dword", addr=31, vdst=2, saddr=0, offset=SLOT),
        vop2("v_xor_b32", 3, V + 1, 2),
        *vop3("v_mad_u32_u24", 4, V + 1, V + 2, V + 0),
        *vop3("v_mad_u64_u32", 5, V + 1, V + 2, V + 3, sdst=10),   # v[5:6], retenues dans s[10:11]
        vop1("v_mov_b32", 7, 10), vop1("v_mov_b32", 8, 11),
        *store_lane(3, 0), *store_lane(4, 1), *store_lane(5, 2), *store_lane(6, 3), *store_lane(7, 4),
        *store_lane(8, 5)), memory=input_memory(1, 2))
    assert actual == expected
    values = lane_values(1)
    assert output(actual, 0)[40] == values[40] ^ lane_values(2)[40]


@gpu
def test_ballot_spanning_both_halves_and_scalar_mask_reductions_match():
    expected, actual = compare(with_lane_offset(
        *global_("global_load_dword", addr=31, vdst=1, saddr=0),
        *vop3("v_readlane_b32", 24, V + 1, const(0)),
        vopc("v_cmp_lt_u32", 24, 1),                              # vcc : lignes où v1[0] < v1
        *vop3("v_cmp_gt_u32", 12, V + 0, const(45)),              # s[12:13] : lignes 46 à 63
        sop1("s_bcnt1_i32_b64", 14, VCC_LO),
        sop1("s_ff1_i32_b64", 15, 12),
        sop2("s_xor_b64", 16, VCC_LO, 12),
        sopc("s_cmp_lg_u64", 16, 12),
        sop2("s_cselect_b32", 18, const(3), const(4)),
        vop1("v_mov_b32", 2, VCC_LO), vop1("v_mov_b32", 3, VCC_HI),
        vop1("v_mov_b32", 4, 14), vop1("v_mov_b32", 5, 15), vop1("v_mov_b32", 6, 16), vop1("v_mov_b32", 7, 17),
        vop1("v_mov_b32", 8, 18),
        *store_lane(2, 0), *store_lane(3, 1), *store_lane(4, 2), *store_lane(5, 3), *store_lane(6, 4),
        *store_lane(7, 5), *store_lane(8, 6)), memory=input_memory(3))
    assert actual == expected
    values = lane_values(3)
    mask = sum(1 << lane for lane in range(LANES) if values[0] < values[lane])
    assert (output(actual, 0)[0], output(actual, 1)[0]) == (mask & 0xFFFF_FFFF, mask >> 32)
    assert output(actual, 3)[0] == 46


@gpu
def test_divergent_exec_across_both_halves_matches():
    expected, actual = compare(with_lane_offset(
        *global_("global_load_dword", addr=31, vdst=1, saddr=0),
        vop1("v_mov_b32", 2, const(0)),
        vop2("v_and_b32", 3, const(3), 1),                       # v3 = valeur & 3 : divergence irrégulière
        vopc("v_cmp_eq_u32", const(1), 3),
        sop1("s_and_saveexec_b64", 4, VCC_LO),                   # if (v3 == 1)
        sopp("s_cbranch_execz", 2),
        vop2("v_add_nc_u32", 2, const(10), 1),
        vop2("v_lshrrev_b32", 2, const(3), 2),
        sop2("s_andn2_b64", EXEC_LO, 4, EXEC_LO),                # else
        sopp("s_cbranch_execz", 1),
        vop2("v_sub_nc_u32", 2, V + 1, 0),
        sop1("s_mov_b64", EXEC_LO, 4),                           # fin du if
        vopc("v_cmpx_gt_u32", const(40), 0),                     # lignes 0 à 39 seulement (exec_hi partiel)
        vop2("v_add_nc_u32", 2, const(7), 2),
        vopc("v_cmpx_lt_u32", const(50), 0),                     # aucune ligne : exec = 0
        sopp("s_cbranch_execz", 1),
        vop1("v_mov_b32", 2, const(0)),                          # sautée
        sop1("s_mov_b64", EXEC_LO, 4),
        *store_lane(2, 0)), memory=input_memory(4))
    assert actual == expected


@gpu
def test_vccnz_loop_runs_until_every_lane_is_done():
    expected, actual = compare(with_lane_offset(
        vop2("v_and_b32", 1, const(7), 0),                       # compteur par ligne : 0..7
        vop1("v_mov_b32", 2, const(0)),
        vopc("v_cmp_ne_u32", const(0), 1),
        sop2("s_and_b64", EXEC_LO, EXEC_LO, VCC_LO),
        vop2("v_add_nc_u32", 2, V + 0, 2),                       # pc 20 : boucle, v2 += ligne
        vop2("v_subrev_nc_u32", 1, const(1), 1),
        vopc("v_cmp_ne_u32", const(0), 1),
        sop2("s_and_b64", EXEC_LO, EXEC_LO, VCC_LO),
        sopp("s_cbranch_vccnz", -5),                             # pc 36 → 20
        sop1("s_mov_b64", EXEC_LO, MINUS_ONE),
        *store_lane(2, 0)))
    assert actual == expected
    assert output(actual, 0)[63] == 63 * 7


@gpu
def test_lane_reads_writes_ids_and_bpermute_match():
    expected, actual = compare(with_lane_offset(
        *global_("global_load_dword", addr=31, vdst=1, saddr=0),
        *vop3("v_mbcnt_lo_u32_b32", 2, MINUS_ONE, const(0)),
        *vop3("v_mbcnt_hi_u32_b32", 2, MINUS_ONE, V + 2),        # v2 = numéro de ligne
        *vop3("v_readlane_b32", 24, V + 1, const(7)),
        vopc("v_cmp_lt_u32", 24, 1),                             # masque irrégulier sur les deux moitiés
        *vop3("v_mbcnt_lo_u32_b32", 3, VCC_LO, const(0)),
        *vop3("v_mbcnt_hi_u32_b32", 3, VCC_HI, V + 3),           # v3 = bits du masque sous la ligne
        *vop3("v_readlane_b32", 4, V + 1, const(50)),
        sop1("s_mov_b32", 6, const(61)),
        *vop3("v_writelane_b32", 1, 4, 6),                       # v1[61] = v1[50]
        vopc("v_cmpx_lt_u32", const(33), 0),                     # exec : lignes 34 à 63
        vop1("v_readfirstlane_b32", 5, V + 1),
        sop1("s_mov_b64", EXEC_LO, MINUS_ONE),
        vop1("v_mov_b32", 6, 5),
        vop2("v_mul_u32_u24", 7, const(60), 0),                  # adresse = 60 × ligne
        *ds("ds_bpermute_b32", addr=7, data0=1, vdst=8, offset0=4),
        *ds("ds_write_b32", addr=31, data0=1),                   # LDS sur 64 lignes
        vop2("v_sub_nc_u32", 9, const(63), 0),
        vop2("v_lshlrev_b32", 9, const(2), 9),
        *ds("ds_read_b32", addr=9, vdst=10),                     # v10 = v1[63 - ligne]
        *store_lane(2, 0), *store_lane(3, 1), *store_lane(1, 2), *store_lane(6, 3), *store_lane(8, 4),
        *store_lane(10, 5)), memory=input_memory(5))
    assert actual == expected
    assert output(actual, 0) == list(range(LANES))
    values = lane_values(5)
    assert output(actual, 2)[61] == values[50]


@gpu
def test_carries_and_cndmask_match():
    expected, actual = compare(with_lane_offset(
        *global_("global_load_dword", addr=31, vdst=1, saddr=0),
        *global_("global_load_dword", addr=31, vdst=2, saddr=0, offset=SLOT),
        *vop3("v_add_co_u32", 3, V + 1, V + 2, sdst=8),          # retenues dans s[8:9]
        *vop3("v_add_co_ci_u32", 4, V + 1, const(0), 8, sdst=10),
        vopc("v_cmp_lt_u32", V + 1, 2),
        vop2("v_add_co_ci_u32", 5, V + 1, 2),                     # retenue d'entrée = vcc
        vop2("v_cndmask_b32", 6, V + 1, 2),                       # vcc ? v2 : v1
        *vop3("v_cndmask_b32", 7, V + 2, V + 1, 8),               # s[8:9] ? v1 : v2
        vop1("v_mov_b32", 11, 8), vop1("v_mov_b32", 12, 9),
        vop1("v_mov_b32", 13, VCC_LO), vop1("v_mov_b32", 14, VCC_HI),
        *store_lane(3, 0), *store_lane(4, 1), *store_lane(5, 2), *store_lane(6, 3), *store_lane(7, 4),
        *store_lane(11, 5), *store_lane(12, 6), *store_lane(13, 7), *store_lane(14, 8)),
        memory=input_memory(6, 7))
    assert actual == expected


@gpu
def test_scalar_64_bit_mask_operations_match_in_wave32():
    from test_rdna2_hlsl import compare as compare32, store_lane as store32, with_lane_offset as lanes32
    expected, actual = compare32(lanes32(
        vopc("v_cmp_gt_u32", const(20), 0),
        sop1("s_and_saveexec_b64", 4, VCC_LO),                    # exec_hi = 0 en wave32
        vop1("v_mov_b32", 1, const(5)),
        sop1("s_orn2_saveexec_b64", 6, 4),
        sop1("s_not_b64", 8, 4),
        sop2("s_nor_b64", 10, 4, VCC_LO),
        sop1("s_bcnt0_i32_b64", 12, 8),
        sop1("s_ff0_i32_b64", 13, 4),
        sop1("s_andn1_wrexec_b64", 14, 8),
        sopc("s_cmp_eq_u64", 4, 6),
        sop1("s_cmov_b64", 16, 10),
        sop1("s_mov_b64", EXEC_LO, MINUS_ONE),
        *[instruction for register in range(4, 18) for instruction in (vop1("v_mov_b32", 2, register),
                                                                       *store32(2, register - 4))],
        *store32(1, 14)))
    assert actual == expected
