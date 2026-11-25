"""Recompilateur RDNA 2 → HLSL → DXIL : chaque programme tourne dans l'interpréteur de référence
et sur le GPU du PC (ou WARP) ; les deux mémoires finales doivent être identiques."""
import struct

import pytest

from emu import rdna2_exec, rdna2_gpu, rdna2_hlsl
from emu.rdna2_exec_ops import float_bits
from test_rdna2_exec import ONE, TWO, V, const, program, sop1, sop2, sopc, sopp, vop1, vop2, vop3, vopc
from test_rdna2_exec_memory import BUFFER, INPUT, OUTPUT, ds, global_, mubuf

VCC_LO, EXEC_LO, LITERAL = 106, 126, 255
MEMORY_SIZE = 0x50000
gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc ou anyxboxx_gpu_run.exe absent")


def store_lane(value_register, slot):
    """global_store_dword de v<value_register> à OUTPUT + slot × 128 + ligne × 4 (v31 = ligne × 4)."""
    return global_("global_store_dword", addr=31, data=value_register, saddr=2, offset=slot * 128)


def with_lane_offset(*words):
    return program(vop2("v_lshlrev_b32", 31, const(2), 0), *words)


def compare(data, sgprs=None, memory=b""):
    """Mémoire finale (interpréteur, GPU) pour le même programme et le même état initial."""
    sgprs = {0: INPUT, 2: OUTPUT, 20: BUFFER, 22: 16, **(sgprs or {})}
    image = bytearray(MEMORY_SIZE)
    image[:len(memory)] = memory
    wave = rdna2_exec.Wave()
    wave.memory.write(0, bytes(image))
    for number, value in sgprs.items():
        wave.s[number] = value
    rdna2_exec.run(data, wave)
    expected = wave.memory.read(0, MEMORY_SIZE)
    return expected, rdna2_gpu.run(rdna2_hlsl.translate(data, sgprs), bytes(image))


def test_translation_is_a_plain_compute_shader():
    hlsl = rdna2_hlsl.translate(program(vop1("v_mov_b32", 1, ONE)))
    assert "[numthreads(32, 1, 1)]" in hlsl and "switch (pc)" not in hlsl and "v[1] = 0x3F800000u" in hlsl


def test_untranslatable_instruction_is_named():
    with pytest.raises(rdna2_hlsl.Untranslatable, match="image_load"):
        rdna2_hlsl.translate(struct.pack("<3I", 0xF000_0000, 0, sopp("s_endpgm")))


@gpu
def test_compute_shader_matches_the_interpreter():
    memory = bytearray(INPUT + 128)
    memory[INPUT:] = struct.pack("<32I", *[float_bits(float(i)) for i in range(32)])
    expected, actual = compare(with_lane_offset(
        *global_("global_load_dword", addr=31, vdst=2, saddr=0),
        *vop3("v_fma_f32", 3, V + 2, TWO, ONE, neg=0b100),          # v3 = v2 × 2 - 1
        *store_lane(3, 0)), memory=bytes(memory))
    assert actual == expected


@gpu
def test_scalar_loop_and_divergence_match():
    expected, actual = compare(with_lane_offset(
        sop1("s_mov_b32", 4, const(0)),
        sop1("s_mov_b32", 5, const(5)),
        sop2("s_add_u32", 4, 4, 5),                                 # boucle : s4 = 5 + 4 + 3 + 2 + 1
        sop2("s_sub_u32", 5, 5, const(1)),
        sopc("s_cmp_lg_u32", 5, const(0)),
        sopp("s_cbranch_scc1", -4),
        vop1("v_mov_b32", 1, 4),                                    # v1 = s4
        vopc("v_cmp_gt_u32", const(16), 0),
        sop1("s_and_saveexec_b32", 6, VCC_LO),
        vop2("v_add_nc_u32", 1, const(7), 1),                       # lignes 0 à 15 seulement
        sop1("s_mov_b32", EXEC_LO, 6),
        *store_lane(1, 0)))
    assert actual == expected
    assert struct.unpack_from("<2I", actual, OUTPUT + 15 * 4) == (22, 15)


@gpu
def test_wave32_ballot_used_as_scalar_data_matches_interpreter():
    expected, actual = compare(with_lane_offset(
        vop2("v_and_b32", 1, const(7), 0),
        vopc("v_cmp_eq_u32", const(7), 1),                           # VCC : lignes 7, 15, 23 et 31
        sop2("s_and_b32", 4, VCC_LO, LITERAL), 0x8000_8000,         # garder les lignes 15 et 31
        sop2("s_or_b32", 5, 4, const(1)),                            # ajouter la ligne 0
        sop1("s_bcnt1_i32_b32", 6, 5),
        sop2("s_cselect_b32", 7, const(1), const(0)),                 # SCC du décompte non nul
        vopc("v_cmp_eq_u32", const(32), 0),                          # aucune ligne : VCC est vide
        sop1("s_bcnt1_i32_b32", 8, VCC_LO),
        sop2("s_cselect_b32", 9, const(1), const(0)),                 # SCC doit retomber à zéro
        vop1("v_mov_b32", 2, 5), *store_lane(2, 0),
        vop1("v_mov_b32", 2, 6), *store_lane(2, 1),
        vop1("v_mov_b32", 2, 7), *store_lane(2, 2),
        vop1("v_mov_b32", 2, 8), *store_lane(2, 3),
        vop1("v_mov_b32", 2, 9), *store_lane(2, 4)))
    assert actual == expected
    for slot, value in enumerate((0x8000_8001, 3, 1, 0, 0)):
        assert struct.unpack_from("<32I", actual, OUTPUT + slot * 128) == (value,) * 32


@gpu
def test_lds_and_buffer_match():
    memory = bytearray(BUFFER + 20)
    memory[BUFFER:] = struct.pack("<5I", 10, 20, 30, 40, 50)
    expected, actual = compare(with_lane_offset(
        *ds("ds_write_b32", addr=31, data0=0),
        vop2("v_add_nc_u32", 3, const(1), 0),
        vop2("v_and_b32", 3, const(31), 3),
        vop2("v_lshlrev_b32", 3, const(2), 3),
        *ds("ds_read_b32", addr=3, vdst=2),                          # rotation entre lignes
        *ds("ds_bpermute_b32", addr=3, data0=0, vdst=7),
        vop1("v_mov_b32", 4, LITERAL), 512,
        vop1("v_mov_b32", 6, const(1)),
        *ds("ds_add_u32", addr=4, data0=6),                          # compteur partagé
        *ds("ds_read_b32", addr=4, vdst=8),
        *mubuf("buffer_load_dword", 31, 9, 20, offen=1),             # bornes du V#
        *store_lane(2, 0), *store_lane(7, 1), *store_lane(8, 2), *store_lane(9, 3)), memory=bytes(memory))
    assert actual == expected
    assert struct.unpack_from("<I", actual, OUTPUT + 2 * 128)[0] == 32


@gpu
def test_vop3_integer_operations_match():
    minus_one = 193
    expected, actual = compare(with_lane_offset(
        *vop3("v_mad_u32_u24", 1, V + 0, const(3), V + 0),
        *vop3("v_bfe_u32", 2, V + 0, const(1), const(2)),
        *vop3("v_med3_i32", 3, V + 0, const(4), const(9)),
        *vop3("v_add_co_u32", 4, minus_one, V + 0, sdst=10),
        *vop3("v_mbcnt_lo_u32_b32", 5, minus_one, const(0)),
        *vop3("v_readlane_b32", 11, V + 0, const(5)),
        vop1("v_mov_b32", 6, 11),
        vop1("v_mov_b32", 7, 10),                                   # masque de retenue
        *store_lane(1, 0), *store_lane(2, 1), *store_lane(3, 2), *store_lane(4, 3),
        *store_lane(5, 4), *store_lane(6, 5), *store_lane(7, 6)))
    assert actual == expected
