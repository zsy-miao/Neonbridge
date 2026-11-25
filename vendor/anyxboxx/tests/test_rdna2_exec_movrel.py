"""Registres indexés par M0 (v_movrel*, s_movrel*) et échanges (v_swap_b32, v_swaprel_b32).

Valeurs attendues calculées à la main d'après le pseudo-code d'AMD (numéro de registre écrit dans
l'instruction + M0, ou M0[9:0] / M0[25:16] pour les formes _2 et v_swaprel) ; le test GPU vérifie
le HLSL contre l'interpréteur.
"""
import pytest

from emu import rdna2_exec, rdna2_gpu, rdna2_hlsl
from test_rdna2_exec import V, const, program, sop1, vop1, vop2, vop3
from test_rdna2_hlsl import compare, store_lane, with_lane_offset

LITERAL, M0, EXEC_LO = 255, 124, 126
gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc ou anyxboxx_gpu_run.exe absent")


def m0(value):
    return sop1("s_mov_b32", M0, LITERAL), value


def registers():
    """v10..v13 = 100 + 2 × n + ligne ; s10..s13 = 0x10..0x13."""
    words = []
    for n in range(4):
        words += [vop2("v_add_nc_u32", 10 + n, LITERAL, 0), 100 + 2 * n, sop1("s_mov_b32", 10 + n, const(0x10 + n))]
    return words


def run(*words):
    return rdna2_exec.run(program(*registers(), *words))


def column(base):
    return [base + lane for lane in range(32)]


def test_vector_moves_relative_to_m0():
    wave = run(
        *m0(2), vop1("v_movrels_b32", 1, V + 10),                     # v1 = v12
        *m0(3), vop1("v_movreld_b32", 20, V + 0),                     # v23 = ligne
        *m0(1), vop1("v_movrelsd_b32", 30, V + 10),                   # v31 = v11
        *m0(5 << 16 | 2), vop1("v_movrelsd_2_b32", 40, V + 10),       # v45 = v12
        *m0(3), *vop3("v_movrels_b32", 2, V + 10))                    # forme VOP3 : v2 = v13
    assert wave.v[1] == column(104) and wave.v[23] == column(0) and wave.v[31] == column(102)
    assert wave.v[45] == column(104) and wave.v[2] == column(106)
    assert wave.v[20] == [0] * 32 and wave.v[40] == [0] * 32


def test_swaps_follow_exec():
    wave = run(
        sop1("s_mov_b32", EXEC_LO, LITERAL), 0x0000_FFFF,
        vop1("v_swap_b32", 10, V + 11),                               # v10 ↔ v11, lignes 0 à 15
        *m0(1 << 16 | 2), vop1("v_swaprel_b32", 20, V + 10),          # v21 ↔ v12
        sop1("s_mov_b32", EXEC_LO, LITERAL), 0xFFFF_FFFF)
    assert wave.v[10] == column(102)[:16] + column(100)[16:]
    assert wave.v[11] == column(100)[:16] + column(102)[16:]
    assert wave.v[21] == column(104)[:16] + [0] * 16
    assert wave.v[12] == [0] * 16 + column(104)[16:]


def test_scalar_moves_relative_to_m0():
    wave = run(
        *m0(2), sop1("s_movrels_b32", 1, 10), sop1("s_movrels_b64", 2, 10),    # s1 = s12 ; s[2:3] = s[12:13]
        *m0(3), sop1("s_movreld_b32", 20, LITERAL), 0xABCD,                     # s23 = 0xABCD
        *m0(2), sop1("s_movreld_b64", 30, 10),                                  # s[32:33] = s[10:11]
        *m0(4 << 16 | 1), sop1("s_movrelsd_2_b32", 40, 10))                    # s44 = s11
    assert wave.s[1:4] == [0x12, 0x12, 0x13] and wave.s[23] == 0xABCD
    assert wave.s[32:34] == [0x10, 0x11] and wave.s[44] == 0x11 and wave.s[20] == 0


@pytest.mark.parametrize("words", [
    (*m0(250), vop1("v_movrels_b32", 1, V + 10)),                     # v260 : au-delà de v255
    (*m0(1), sop1("s_movrels_b64", 2, 10)),                           # paire s[11:12] impaire
    (*m0(100), sop1("s_movreld_b32", 10, const(1))),                  # s110 : hors de s0-s105
    (vop1("v_movrels_b32", 1, const(1)),),                            # source qui n'est pas un VGPR
    (*vop3("v_movrels_b32", 1, V + 10, neg=1),),                      # modificateur VOP3
])
def test_out_of_model_indexing_is_named(words):
    with pytest.raises(rdna2_exec.Untranslated, match="mov"):
        run(*words)


def movrel_program():
    ops = [
        *registers(), vop2("v_mul_u32_u24", 14, LITERAL, 0), 0x0001_0101,
        *m0(2), vop1("v_movrels_b32", 1, V + 10), *vop3("v_movrels_b32", 2, V + 11),
        *m0(3), vop1("v_movreld_b32", 20, V + 14), *vop3("v_movreld_b32", 21, LITERAL), 0x1234,
        *m0(1), vop1("v_movrelsd_b32", 32, V + 10),                  # v33 = v11 (v31 porte les adresses)
        *m0(5 << 16 | 2), vop1("v_movrelsd_2_b32", 40, V + 10),
        sop1("s_mov_b32", EXEC_LO, LITERAL), 0x3C3C_A5A5,
        vop1("v_swap_b32", 10, V + 11), *m0(1 << 16 | 2), *vop3("v_swaprel_b32", 50, V + 11),
        sop1("s_mov_b32", EXEC_LO, LITERAL), 0xFFFF_FFFF,
        *m0(2), sop1("s_movrels_b32", 60, 10), sop1("s_movrels_b64", 62, 10),
        *m0(3), sop1("s_movreld_b32", 64, LITERAL), 0xABCD, *m0(2), sop1("s_movreld_b64", 66, 10),
        *m0(4 << 16 | 1), sop1("s_movrelsd_2_b32", 70, 12),
    ]
    vector = [1, 2, 23, 24, 33, 45, 10, 11, 13, 51]
    scalar = [60, 62, 63, 67, 68, 69, 74]
    stores = []
    for slot, register in enumerate(vector):
        stores += store_lane(register, slot)
    for slot, register in enumerate(scalar, start=len(vector)):
        stores += [vop1("v_mov_b32", 3, register), *store_lane(3, slot)]
    return with_lane_offset(*ops, *stores)


def test_hlsl_indexes_register_arrays():
    hlsl = rdna2_hlsl.translate(movrel_program())
    assert "v[i]" in hlsl and "s[j] = x;" in hlsl and "(s[124] & 0x3FFu)" in hlsl


@gpu
def test_relative_moves_match_interpreter_on_gpu():
    expected, actual = compare(movrel_program())
    assert actual == expected
