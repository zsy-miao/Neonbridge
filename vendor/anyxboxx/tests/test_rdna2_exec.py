"""Interpréteur de référence RDNA 2 (wave32) : vrais petits programmes GPU exécutés ligne par ligne."""
import struct

import pytest

from emu import rdna2_exec
from emu.rdna2_exec_ops import BRANCHES, SCALAR, VECTOR, VECTOR_COMPARE, bits_float
from emu.rdna2_ops import MNEMONICS

OPCODES = {(family, name): opcode for (family, opcode), name in MNEMONICS.items()}
V = 256                         # v0 en opérande source 9 bits
VCC_LO, EXEC_LO, LITERAL = 106, 126, 255
ONE, TWO = 242, 244             # constantes flottantes en ligne 1.0 et 2.0


def const(n):
    return 128 + n              # constante entière en ligne 0..64


def sop2(name, sdst, ssrc0, ssrc1):
    return 0b10 << 30 | OPCODES["SOP2", name] << 23 | sdst << 16 | ssrc1 << 8 | ssrc0


def sop1(name, sdst, ssrc0):
    return 0x17D << 23 | sdst << 16 | OPCODES["SOP1", name] << 8 | ssrc0


def sopc(name, ssrc0, ssrc1):
    return 0x17E << 23 | OPCODES["SOPC", name] << 16 | ssrc1 << 8 | ssrc0


def sopp(name, simm16=0):
    return 0x17F << 23 | OPCODES["SOPP", name] << 16 | simm16 & 0xFFFF


def vop1(name, vdst, src0):
    return 0x3F << 25 | vdst << 17 | OPCODES["VOP1", name] << 9 | src0


def vop2(name, vdst, src0, vsrc1):
    return OPCODES["VOP2", name] << 25 | vdst << 17 | vsrc1 << 9 | src0


def vopc(name, src0, vsrc1):
    return 0x3E << 25 | OPCODES["VOPC", name] << 17 | vsrc1 << 9 | src0


def program(*words):
    """Programme terminé par s_endpgm."""
    return struct.pack(f"<{len(words) + 1}I", *words, sopp("s_endpgm"))


def test_vector_math_runs_on_every_lane():
    wave = rdna2_exec.run(program(
        vop1("v_cvt_f32_u32", 1, V + 0),        # v1 = float(numéro de ligne)
        vop2("v_mul_f32", 1, TWO, 1),           # v1 = 2.0 * v1
        vop2("v_add_f32", 1, ONE, 1)))          # v1 = 1.0 + v1
    assert [bits_float(x) for x in wave.v[1]] == [2.0 * lane + 1 for lane in range(32)]


def test_scalar_loop_with_branch():
    # s0 = 0 ; s1 = 5 ; faire { s0 += s1 ; s1 -= 1 } tant que s1 != 0  →  s0 = 15
    wave = rdna2_exec.run(program(
        sop1("s_mov_b32", 0, const(0)),
        sop1("s_mov_b32", 1, const(5)),
        sop2("s_add_u32", 0, 0, 1),             # pc 8 : début de la boucle
        sop2("s_sub_u32", 1, 1, const(1)),
        sopc("s_cmp_lg_u32", 1, const(0)),
        sopp("s_cbranch_scc1", -4)))            # pc 20 : retour à 8 = 20 + 4 - 4 × 4
    assert wave.s[0] == 15 and wave.s[1] == 0


def test_divergent_lanes_follow_exec():
    wave = rdna2_exec.run(program(
        vopc("v_cmp_gt_u32", const(16), 0),         # vcc = lignes où 16 > v0 (numéro de ligne)
        sop1("s_and_saveexec_b32", 2, VCC_LO),      # s2 = exec ; exec &= vcc
        vop1("v_mov_b32", 1, ONE),                  # seules les lignes 0 à 15 écrivent
        sop1("s_mov_b32", EXEC_LO, 2)))             # exec restauré
    assert wave.v[1] == [0x3F80_0000] * 16 + [0] * 16
    assert wave.exec == 0xFFFF_FFFF and wave.s[2] == 0xFFFF_FFFF and wave.vcc == 0xFFFF


def test_literal_and_fused_multiply_add():
    pi = 0x4049_0FDB
    data = struct.pack("<4I", vop1("v_mov_b32", 3, LITERAL), pi, vop2("v_fmac_f32", 3, TWO, 3),
                       sopp("s_endpgm"))
    wave = rdna2_exec.run(data)                  # v3 = 2.0 × v3 + v3 = 3π
    assert bits_float(wave.v[3][7]) == pytest.approx(3 * 3.14159265, rel=1e-6)


def test_untranslated_instruction_is_named():
    data = struct.pack("<3I", 0xF03C_0000, 0, sopp("s_endpgm"))     # image_atomic_swap (famille MIMG)
    with pytest.raises(rdna2_exec.Untranslated, match="instruction non traduite : image_atomic_swap"):
        rdna2_exec.run(data)


def test_semantics_only_name_real_rdna2_instructions():
    known = set(MNEMONICS.values())
    named = set(SCALAR) | set(VECTOR) | set(VECTOR_COMPARE) | set(BRANCHES)
    assert sorted(named - known) == []
    done, total = rdna2_exec.coverage()
    assert 150 <= done < total


def vop3(name, vdst, src0, src1=128, src2=128, neg=0, abs_=0, clamp=0, omod=0, sdst=0, op_sel=0):
    """Deux mots VOP3 (src* = codes source 9 bits, 128 = constante 0 ; op_sel : bits 14:11)."""
    low = 0x35 << 26 | OPCODES["VOP3", name] << 16 | clamp << 15 | (abs_ | sdst | op_sel << 3) << 8 | vdst
    high = neg << 29 | omod << 27 | src2 << 18 | src1 << 9 | src0
    return low, high


def test_vop3_fma_with_source_modifiers():
    wave = rdna2_exec.run(program(
        vop1("v_cvt_f32_u32", 1, V + 0),
        *vop3("v_fma_f32", 2, V + 1, TWO, ONE),                     # v2 = v1 × 2 + 1
        *vop3("v_fma_f32", 3, V + 1, TWO, ONE, neg=0b001)))         # v3 = -v1 × 2 + 1
    assert [bits_float(x) for x in wave.v[2][:4]] == [1.0, 3.0, 5.0, 7.0]
    assert [bits_float(x) for x in wave.v[3][:4]] == [1.0, -1.0, -3.0, -5.0]


def test_vop3_integer_and_bitfield_operations():
    wave = rdna2_exec.run(program(
        *vop3("v_mad_u32_u24", 1, V + 0, const(3), V + 0),          # 3 × ligne + ligne
        *vop3("v_bfe_u32", 2, V + 0, const(1), const(2)),           # (ligne >> 1) & 3
        *vop3("v_add3_u32", 3, V + 0, V + 0, const(7)),
        *vop3("v_med3_i32", 4, V + 0, const(4), const(9))))         # ligne bornée à [4, 9]
    assert wave.v[1][5] == 20 and wave.v[2][6] == 3 and wave.v[3][10] == 27
    assert [wave.v[4][lane] for lane in (0, 6, 31)] == [4, 6, 9]


def test_vop3_compare_writes_a_scalar_mask():
    wave = rdna2_exec.run(program(*vop3("v_cmp_gt_u32", 4, const(16), V + 0)))
    assert wave.s[4] == 0xFFFF and wave.vcc == 0


def test_vop3b_carry_chain():
    minus_one = 193
    wave = rdna2_exec.run(program(
        *vop3("v_add_co_u32", 5, minus_one, V + 0, sdst=6),         # ligne - 1, retenue si ligne ≥ 1
        *vop3("v_add_co_ci_u32", 6, const(0), const(0), 6, sdst=7)))  # v6 = retenue entrante
    assert wave.v[5][0] == 0xFFFF_FFFF and wave.v[5][3] == 2
    assert wave.s[6] == 0xFFFF_FFFE and wave.v[6][:3] == [0, 1, 1]


def test_lane_operations():
    minus_one = 193
    wave = rdna2_exec.run(program(
        *vop3("v_mbcnt_lo_u32_b32", 7, minus_one, const(0)),        # idiome : numéro de ligne
        *vop3("v_readlane_b32", 8, V + 0, const(5)),
        *vop3("v_writelane_b32", 9, const(42), const(3))))
    assert wave.v[7] == list(range(32)) and wave.s[8] == 5
    assert wave.v[9][3] == 42 and wave.v[9][2] == 0


def test_output_modifiers_clamp_and_multiply():
    wave = rdna2_exec.run(program(
        vop1("v_cvt_f32_u32", 1, V + 0),
        *vop3("v_mul_f32", 10, ONE, V + 1, clamp=1, omod=1)))       # min(1, 2 × ligne)
    assert [bits_float(x) for x in wave.v[10][:3]] == [0.0, 1.0, 1.0]


def test_vop3_text_matches_llvm_syntax():
    from emu import rdna2_operands

    data = struct.pack("<2I", *vop3("v_fma_f32", 2, V + 1, TWO, ONE, neg=0b001, abs_=0b001))
    assert rdna2_operands.text(data) == "v_fma_f32 v2, -|v1|, 2.0, 1.0"
    data = struct.pack("<2I", *vop3("v_mul_f32", 10, ONE, V + 1, clamp=1, omod=1))
    assert rdna2_operands.text(data) == "v_mul_f32_e64 v10, 1.0, v1 clamp mul:2"
