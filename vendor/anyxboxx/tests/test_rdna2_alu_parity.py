"""Parity between the RDNA 2 reference interpreter and HLSL recompiler for 32-bit ALU, VOP3,
carry, v_mul_legacy_f32, and v_perm_b32 instructions."""
import math

import pytest

from emu import rdna2_exec, rdna2_exec_ops, rdna2_exec_vop3, rdna2_gpu, rdna2_hlsl
from emu.rdna2_exec_ops import bits_float, float_bits
from test_rdna2_exec import OPCODES, V, const, program, sop1, sop2, sopc, vop1, vop2, vop3
from test_rdna2_hlsl import compare, store_lane, with_lane_offset

LITERAL = 255
gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc ou anyxboxx_gpu_run.exe absent")


def sopk(name, sdst, simm16):
    return 0b1011 << 28 | OPCODES["SOPK", name] << 23 | sdst << 16 | (simm16 & 0xFFFF)


def test_v_mul_legacy_f32_zero_kills_nan_and_inf():
    nan_bits = 0x7FC0_0000
    inf_bits = 0x7F80_0000
    neg_zero = 0x8000_0000
    code = program(
        vop1("v_mov_b32", 1, LITERAL), nan_bits,
        vop1("v_mov_b32", 2, LITERAL), inf_bits,
        vop1("v_mov_b32", 3, LITERAL), float_bits(-2.0),
        vop1("v_mov_b32", 4, LITERAL), float_bits(3.0),
        vop2("v_mul_legacy_f32", 5, const(0), 1),             # 0.0 * NaN == +0.0
        vop2("v_mul_legacy_f32", 6, LITERAL, 2), neg_zero,    # -0.0 * +Inf == +0.0
        *vop3("v_mul_legacy_f32", 7, V + 3, V + 4, 0),        # -2.0 * 3.0 == -6.0
    )
    wave = rdna2_exec.Wave()
    rdna2_exec.run(code, wave)
    assert wave.v[5][0] == 0
    assert wave.v[6][0] == 0
    assert bits_float(wave.v[7][0]) == -6.0

    hlsl = rdna2_hlsl.translate(code)
    assert "uint MulLegacyF32(" in hlsl
    assert hlsl.count("MulLegacyF32(") >= 4


def test_v_perm_b32_all_selector_modes():
    src0 = 0x80_7F_02_01    # bytes 4..7: b4=0x01, b5=0x02, b6=0x7F, b7=0x80 (sign bits: b5=0, b7=1)
    src1 = 0x70_80_AB_CD    # bytes 0..3: b0=0xCD, b1=0xAB, b2=0x80, b3=0x70 (sign bits: b1=1, b3=0)
    sel_bytes = 0x07_04_02_00      # [b0=CD, b2=80, b4=01, b7=80] -> 0x800180CD
    sel_zero_ext = 0x0C_0C_0C_00   # RDNA 2 zero-extension of byte 0 (sel 12 -> 0x00): 0x000000CD
    sel_signs = 0x0D_0B_0A_08      # [sign(b1)=FF, sign(b5)=00, sign(b7)=FF, 13->FF] -> 0xFFFF00FF
    sel_sign_b3 = 0x0C_09_09_0C    # [0x00, sign(b3)=00, sign(b3)=00, 0x00] -> 0x00000000
    code = program(
        vop1("v_mov_b32", 1, LITERAL), src0,
        vop1("v_mov_b32", 2, LITERAL), src1,
        vop1("v_mov_b32", 3, LITERAL), sel_bytes,
        vop1("v_mov_b32", 4, LITERAL), sel_zero_ext,
        vop1("v_mov_b32", 5, LITERAL), sel_signs,
        vop1("v_mov_b32", 9, LITERAL), sel_sign_b3,
        *vop3("v_perm_b32", 6, V + 1, V + 2, V + 3),
        *vop3("v_perm_b32", 7, V + 1, V + 2, V + 4),
        *vop3("v_perm_b32", 8, V + 1, V + 2, V + 5),
        *vop3("v_perm_b32", 10, V + 1, V + 2, V + 9),
    )
    wave = rdna2_exec.Wave()
    rdna2_exec.run(code, wave)
    assert wave.v[6][0] == 0x8001_80CD
    assert wave.v[7][0] == 0x0000_00CD
    assert wave.v[8][0] == 0xFFFF_00FF
    assert wave.v[10][0] == 0x0000_0000

    hlsl = rdna2_hlsl.translate(code)
    assert "uint PermB32(" in hlsl
    assert hlsl.count("PermB32(") >= 5


def test_all_32bit_exec_alu_ops_are_covered_by_hlsl():
    covered = rdna2_hlsl.coverage_names()
    missing_vector = (set(rdna2_exec_ops.VECTOR) | set(rdna2_exec_vop3.VECTOR3)
                      | set(rdna2_exec_vop3.CARRY) | set(rdna2_exec_vop3.LANE_OPERATIONS)) - covered
    assert missing_vector == set()

    wide = ("_b64", "_u64", "_i64", "_b64_b32")       # 64 bits : pas encore traduits en HLSL
    scalar_32 = {name for name in rdna2_exec_ops.SCALAR if not name.endswith(wide)}
    missing_scalar = scalar_32 - covered
    assert missing_scalar == set()


def test_hlsl_32bit_scalar_parity_emission():
    code = program(
        sop1("s_mov_b32", 4, LITERAL), 0xFFFF_FFFF,
        sop2("s_add_u32", 5, 4, const(1)),                    # s5 = 0, scc = 1
        sop2("s_addc_u32", 6, const(10), const(20)),          # s6 = 31, scc = 0
        sop2("s_subb_u32", 7, const(5), const(10)),           # s7 = -5, scc = 1
        sop1("s_cmov_b32", 8, const(42)),                     # s8 = 42 (scc == 1)
        sopk("s_cmovk_i32", 9, 7),                            # s9 = 7
        sopk("s_addk_i32", 9, -2),                            # s9 = 5
        sopk("s_mulk_i32", 9, 3),                             # s9 = 15
        sop2("s_min_i32", 10, 7, 9),                          # s10 = -5
        sop2("s_max_i32", 11, 7, 9),                          # s11 = 15
        sop1("s_abs_i32", 12, 10),                            # s12 = 5
        sop1("s_sext_i32_i8", 13, LITERAL), 0x80,             # s13 = -128
        sop1("s_sext_i32_i16", 14, LITERAL), 0x8000,          # s14 = -32768
        sop1("s_brev_b32", 15, const(1)),                     # s15 = 0x80000000
        sop1("s_bcnt1_i32_b32", 16, const(0xF)),              # s16 = 4
        sop1("s_ff1_i32_b32", 17, const(8)),                  # s17 = 3
        sop1("s_flbit_i32_b32", 18, const(8)),                # s18 = 28
        sop2("s_bfm_b32", 19, const(4), const(8)),            # s19 = 0x00000F00
        sop2("s_orn2_b32", 20, const(1), const(3)),
        sop2("s_nand_b32", 21, const(1), const(3)),
        sop2("s_nor_b32", 22, const(1), const(2)),
        sop2("s_xnor_b32", 23, const(1), const(3)),
        sopc("s_bitcmp1_b32", 19, const(8)),
        sopc("s_bitcmp0_b32", 19, const(0)),
        sop1("s_xor_saveexec_b32", 24, const(0)),
    )
    wave = rdna2_exec.Wave()
    rdna2_exec.run(code, wave)
    assert wave.s[6] == 31
    assert wave.s[8] == 42
    assert wave.s[9] == 15
    assert wave.s[12] == 5
    assert wave.s[13] == 0xFFFF_FF80
    assert wave.s[14] == 0xFFFF_8000
    assert wave.s[15] == 0x8000_0000
    assert wave.s[16] == 4
    assert wave.s[17] == 3
    assert wave.s[18] == 28
    assert wave.s[19] == 0x0000_0F00

    hlsl = rdna2_hlsl.translate(code)
    for token in ("reversebits(", "countbits(", "firstbitlow(", "firstbithigh(", "if (scc)"):
        assert token in hlsl


def test_hlsl_vop3_and_carry_parity_emission():
    code = program(
        vop1("v_mov_b32", 1, LITERAL), 0x8000_0001,
        vop1("v_mov_b32", 2, LITERAL), 0x0000_0004,
        vop1("v_bfrev_b32", 3, V + 1),
        vop1("v_ffbl_b32", 4, V + 2),
        vop1("v_ffbh_u32", 5, V + 2),
        vop1("v_cvt_f32_ubyte0", 6, V + 1),
        vop1("v_cvt_f32_ubyte3", 7, V + 1),
        vop1("v_sin_f32", 8, const(0)),
        vop1("v_cos_f32", 9, const(0)),
        vop1("v_rndne_f32", 10, LITERAL), float_bits(2.5),
        vop2("v_xnor_b32", 11, V + 1, 2),
        vop2("v_mul_hi_u32_u24", 12, LITERAL, 2), 0x00FF_FFFF,
        vop2("v_mul_i32_i24", 13, LITERAL, 2), 0x00FF_FFFF,    # -1 * 4 == -4
        *vop3("v_mul_hi_u32", 14, V + 1, V + 2, 0),
        *vop3("v_mul_hi_i32", 15, V + 1, V + 2, 0),
        *vop3("v_bfe_i32", 16, V + 1, const(0), const(4)),
        *vop3("v_alignbit_b32", 17, V + 1, V + 2, const(4)),
        *vop3("v_alignbyte_b32", 18, V + 1, V + 2, const(1)),
        *vop3("v_sad_u32", 19, V + 2, const(10), const(1)),
        *vop3("v_xad_u32", 20, V + 2, const(3), const(5)),
        *vop3("v_add_lshl_u32", 21, V + 2, const(1), const(2)),
        *vop3("v_bfm_b32", 22, const(4), const(8), 0),
        *vop3("v_ldexp_f32", 23, LITERAL, const(3), 0), float_bits(1.5),
        *vop3("v_fma_legacy_f32", 24, const(0), LITERAL, V + 10), 0x7FC0_0000,
        *vop3("v_min3_i32", 25, V + 1, V + 2, const(0)),
        *vop3("v_max3_i32", 26, V + 1, V + 2, const(0)),
        *vop3("v_med3_u32", 27, V + 1, V + 2, const(10)),
        *vop3("v_mbcnt_hi_u32_b32", 28, const(0), const(7), 0),
        *vop3("v_subrev_co_u32", 29, const(10), const(3), 0, sdst=30),
        *vop3("v_sub_co_ci_u32", 30, const(3), const(3), 30, sdst=31),
        *vop3("v_subrev_co_ci_u32", 31, const(3), const(3), 31, sdst=32),
    )
    wave = rdna2_exec.Wave()
    rdna2_exec.run(code, wave)
    assert wave.v[3][0] == 0x8000_0001
    assert wave.v[4][0] == 2
    assert wave.v[5][0] == 29
    assert bits_float(wave.v[6][0]) == 1.0
    assert bits_float(wave.v[7][0]) == 128.0
    assert math.isclose(bits_float(wave.v[9][0]), 1.0)
    assert bits_float(wave.v[10][0]) == 2.0
    assert wave.v[13][0] == 0xFFFF_FFFC
    assert wave.v[14][0] == 2
    assert wave.v[15][0] == 0xFFFF_FFFE
    assert wave.v[19][0] == 7
    assert wave.v[20][0] == 12
    assert wave.v[21][0] == 20
    assert wave.v[22][0] == 0x0000_0F00
    assert bits_float(wave.v[23][0]) == 12.0
    assert bits_float(wave.v[24][0]) == 2.0
    assert wave.v[28][0] == 7
    assert wave.s[30] == 0xFFFF_FFFF
    assert wave.s[31] == 0xFFFF_FFFF

    hlsl = rdna2_hlsl.translate(code)
    for token in ("MulHiU32(", "MulHiI32(", "BfeI32(", "AlignBit(", "SadU32(", "FmaLegacyF32("):
        assert token in hlsl


@gpu
def test_extended_alu_matches_interpreter_on_gpu():
    code = with_lane_offset(
        vop1("v_mov_b32", 1, LITERAL), 0x8000_0003,
        vop1("v_mov_b32", 2, LITERAL), 0x0102_0304,
        vop1("v_mov_b32", 3, LITERAL), 0x0D_0C_0B_00,
        vop1("v_mov_b32", 14, LITERAL), 0x7FC0_0000,
        vop1("v_mov_b32", 15, LITERAL), float_bits(2.5),
        *vop3("v_mul_hi_u32", 4, V + 1, V + 0, 0),
        *vop3("v_mul_hi_i32", 5, V + 1, V + 0, 0),
        *vop3("v_bfe_i32", 6, V + 1, const(0), const(4)),
        *vop3("v_alignbit_b32", 7, V + 1, V + 2, V + 0),
        *vop3("v_perm_b32", 8, V + 1, V + 2, V + 3),
        vop2("v_mul_legacy_f32", 9, const(0), 14),
        *vop3("v_subrev_co_u32", 10, const(10), V + 0, 0, sdst=12),
        *vop3("v_sub_co_ci_u32", 11, V + 0, const(10), 12, sdst=13),
        *vop3("v_subrev_co_ci_u32", 12, V + 0, const(10), 13, sdst=14),
        vop1("v_bfrev_b32", 13, V + 2),
        vop1("v_ffbl_b32", 16, V + 2),
        vop1("v_ffbh_u32", 17, V + 2),
        vop1("v_cvt_f32_ubyte0", 18, V + 2),
        vop1("v_cvt_f32_ubyte1", 19, V + 2),
        vop1("v_cvt_f32_ubyte2", 20, V + 2),
        vop1("v_cvt_f32_ubyte3", 21, V + 2),
        vop1("v_sin_f32", 22, const(0)),
        vop1("v_cos_f32", 23, const(0)),
        vop1("v_rndne_f32", 24, V + 15),
        vop2("v_xnor_b32", 25, V + 1, 2),
        vop2("v_mul_i32_i24", 26, LITERAL, 2), 0x00FF_FFFF,
        *vop3("v_sad_u32", 27, V + 0, const(10), const(1)),
        *vop3("v_xad_u32", 28, V + 0, const(3), const(5)),
        *vop3("v_add_lshl_u32", 29, V + 0, const(1), const(2)),
        *vop3("v_fma_legacy_f32", 30, const(0), V + 14, V + 15),
        *store_lane(4, 0), *store_lane(5, 1), *store_lane(6, 2), *store_lane(7, 3),
        *store_lane(8, 4), *store_lane(9, 5), *store_lane(10, 6), *store_lane(11, 7),
        *store_lane(12, 8), *store_lane(13, 9), *store_lane(16, 10), *store_lane(17, 11),
        *store_lane(18, 12), *store_lane(19, 13), *store_lane(20, 14), *store_lane(21, 15),
        *store_lane(22, 16), *store_lane(23, 17), *store_lane(24, 18), *store_lane(25, 19),
        *store_lane(26, 20), *store_lane(27, 21), *store_lane(28, 22), *store_lane(29, 23),
        *store_lane(30, 24),
    )
    expected, actual = compare(code)
    assert actual == expected
