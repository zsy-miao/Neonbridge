"""Tests for SDWA, DPP16, and DPP8 execution (rdna2_exec_ext) and HLSL recompilation
(rdna2_hlsl_ext) for RDNA 2 VOP1, VOP2, and VOPC instructions (#21).
"""
import pytest

from emu import rdna2_exec, rdna2_gpu, rdna2_hlsl
from emu.rdna2_exec_ops import float_bits
from test_rdna2_exec import V, const, program, vop1, vop2, vopc
from test_rdna2_hlsl import compare, store_lane, with_lane_offset

SDWA, DPP16, DPP8, DPP8_FI = 0xF9, 0xFA, 0xE9, 0xEA
BYTE_0, BYTE_1, BYTE_2, BYTE_3, WORD_0, WORD_1, DWORD = range(7)
UNUSED_PAD, UNUSED_SEXT, UNUSED_PRESERVE = 0, 1, 2
gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc ou anyxboxx_gpu_run.exe absent")


def sdwa_word(src0, dst_sel=DWORD, dst_unused=UNUSED_PAD, clamp=0, omod=0,
              src0_sel=DWORD, src0_sext=0, src0_neg=0, src0_abs=0, src0_scalar=0,
              src1_sel=DWORD, src1_sext=0, src1_neg=0, src1_abs=0, src1_scalar=0):
    return (
        (src0 & 0xFF)
        | ((dst_sel & 7) << 8)
        | ((dst_unused & 3) << 11)
        | ((clamp & 1) << 13)
        | ((omod & 3) << 14)
        | ((src0_sel & 7) << 16)
        | ((src0_sext & 1) << 19)
        | ((src0_neg & 1) << 20)
        | ((src0_abs & 1) << 21)
        | ((src0_scalar & 1) << 23)
        | ((src1_sel & 7) << 24)
        | ((src1_sext & 1) << 27)
        | ((src1_neg & 1) << 28)
        | ((src1_abs & 1) << 29)
        | ((src1_scalar & 1) << 31)
    )


def dpp16_word(src0, ctrl, fi=0, bound_ctrl=0, src0_neg=0, src0_abs=0,
               src1_neg=0, src1_abs=0, bank_mask=0xF, row_mask=0xF):
    return (
        (src0 & 0xFF)
        | ((ctrl & 0x1FF) << 8)
        | ((fi & 1) << 18)
        | ((bound_ctrl & 1) << 19)
        | ((src0_neg & 1) << 20)
        | ((src0_abs & 1) << 21)
        | ((src1_neg & 1) << 22)
        | ((src1_abs & 1) << 23)
        | ((bank_mask & 0xF) << 24)
        | ((row_mask & 0xF) << 28)
    )


def dpp8_word(src0, selectors):
    word = src0 & 0xFF
    for idx, sel in enumerate(selectors):
        word |= (sel & 7) << (8 + 3 * idx)
    return word


def run_wave(words, init_v=None, init_s=None, exec_mask=0xFFFF_FFFF):
    wave = rdna2_exec.Wave()
    wave.s[rdna2_exec.EXEC_LO] = exec_mask
    for reg, vals in (init_v or {}).items():
        wave.v[reg] = list(vals)
    for reg, val in (init_s or {}).items():
        wave.s[reg] = val
    rdna2_exec.run(program(*words), wave)
    return wave


def test_sdwa_byte_word_extract_sext_and_dst_unused():
    # v1 = 0x80FE_7F01, v2 = 0x1122_3344
    # v_add_nc_u32_sdwa v2, sext(v1), v1 dst_sel:BYTE_1 dst_unused:UNUSED_PRESERVE src0_sel:BYTE_2 src1_sel:BYTE_0
    # src0 = sext(0xFE) = 0xFFFFFFFE (-2), src1 = 0x01 -> sum = 0xFFFFFFFF -> BYTE_1 = 0xFF00, preserve other bytes
    ins = (
        vop2("v_add_nc_u32", 2, SDWA, 1),
        sdwa_word(1, dst_sel=BYTE_1, dst_unused=UNUSED_PRESERVE, src0_sel=BYTE_2, src0_sext=1, src1_sel=BYTE_0),
    )
    wave = run_wave(ins, init_v={1: [0x80FE_7F01] * 32, 2: [0x1122_3344] * 32})
    assert wave.v[2][0] == 0x1122_FF44

    # UNUSED_SEXT on BYTE_0 of 0x0000_0085 -> 0xFFFF_FF85
    ins_sext = (
        vop1("v_mov_b32", 3, SDWA),
        sdwa_word(1, dst_sel=BYTE_0, dst_unused=UNUSED_SEXT, src0_sel=BYTE_3),
    )
    wave2 = run_wave(ins_sext, init_v={1: [0x8500_0000] * 32})
    assert wave2.v[3][0] == 0xFFFF_FF85

    # UNUSED_PAD on WORD_1 of 0x1234_5678 (src0_sel=WORD_0 -> 0x5678 -> placed at WORD_1 -> 0x5678_0000)
    ins_pad = (
        vop1("v_mov_b32", 3, SDWA),
        sdwa_word(1, dst_sel=WORD_1, dst_unused=UNUSED_PAD, src0_sel=WORD_0),
    )
    wave3 = run_wave(ins_pad, init_v={1: [0x1234_5678] * 32, 3: [0xDEAD_BEEF] * 32})
    assert wave3.v[3][0] == 0x5678_0000


def test_sdwa_float_abs_neg_omod_clamp_and_scalar_src():
    # v_add_f32_sdwa v3, -|s4|, v1 mul:2 clamp -> (-|0.25| + 1.0) * 2 = 1.5 -> clamped to 1.0
    ins = (
        vop2("v_add_f32", 3, SDWA, 1),
        sdwa_word(4, dst_sel=DWORD, clamp=1, omod=1, src0_sel=DWORD, src0_neg=1, src0_abs=1, src0_scalar=1),
    )
    wave = run_wave(ins, init_v={1: [float_bits(1.0)] * 32}, init_s={4: float_bits(-0.25)})
    assert wave.v[3][0] == float_bits(1.0)


def test_sdwa_vopc_custom_sdst():
    # v_cmp_eq_u32_sdwa s8, v1, v2 src0_sel:BYTE_0 src1_sel:BYTE_0 (sd=1, sdst=8)
    ext = (1 & 0xFF) | (8 << 8) | (1 << 15) | (BYTE_0 << 16) | (BYTE_0 << 24)
    ins = (vopc("v_cmp_eq_u32", SDWA, 2), ext)
    v1 = [0xAA00_0000 | (i & 3) for i in range(32)]
    v2 = [0xBB00_0002 for _ in range(32)]
    wave = run_wave(ins, init_v={1: v1, 2: v2})
    want = sum(1 << i for i in range(32) if (i & 3) == 2)
    assert wave.s[8] == want


def test_dpp16_quad_perm_row_ops_masks_and_bound_ctrl():
    lanes = list(range(32))
    # quad_perm:[3,2,1,0] reverses every 4-lane quad
    qp = 3 | (2 << 2) | (1 << 4) | (0 << 6)
    wave = run_wave((vop1("v_mov_b32", 2, DPP16), dpp16_word(1, qp)), init_v={1: lanes})
    assert wave.v[2] == [i ^ 3 for i in range(32)]

    # row_shl:1 with bound_ctrl=1 (lane 15 and 31 get 0 instead of preserving dst)
    wave_shl = run_wave(
        (vop1("v_mov_b32", 2, DPP16), dpp16_word(1, 0x101, bound_ctrl=1)),
        init_v={1: [100 + i for i in range(32)], 2: [999] * 32},
    )
    for i in range(32):
        expected = 0 if (i & 15) == 15 else 100 + i + 1
        assert wave_shl.v[2][i] == expected

    # row_shr:1 with bound_ctrl=0 preserves dst on lane 0 and 16
    wave_shr = run_wave(
        (vop1("v_mov_b32", 2, DPP16), dpp16_word(1, 0x111, bound_ctrl=0)),
        init_v={1: [100 + i for i in range(32)], 2: [777] * 32},
    )
    for i in range(32):
        expected = 777 if (i & 15) == 0 else 100 + i - 1
        assert wave_shr.v[2][i] == expected

    # row_mirror (0x140) with row_mask=0x1 (only row 0: lanes 0..15 write; row 1 preserves 555)
    wave_mir = run_wave(
        (vop1("v_mov_b32", 2, DPP16), dpp16_word(1, 0x140, row_mask=0x1, bank_mask=0xF)),
        init_v={1: lanes, 2: [555] * 32},
    )
    assert wave_mir.v[2][:16] == list(reversed(range(16)))
    assert wave_mir.v[2][16:] == [555] * 16


def test_dpp8_and_fi_respect_exec_mask():
    # dpp8:[7,6,5,4,3,2,1,0] reverses each 8-lane group; lane 7 is inactive in EXEC
    exec_mask = 0xFFFF_FFFF ^ (1 << 7)
    sels = [7, 6, 5, 4, 3, 2, 1, 0]
    v1 = [10 + i for i in range(32)]
    # fi=0 (0xE9): lane 0 reads lane 7 (inactive) -> preserves old v2[0] = 888
    w0 = run_wave((vop1("v_mov_b32", 2, DPP8), dpp8_word(1, sels)),
                  init_v={1: v1, 2: [888] * 32}, exec_mask=exec_mask)
    assert w0.v[2][0] == 888
    assert w0.v[2][1] == 16  # lane 1 reads lane 6 (active) -> 10 + 6 = 16
    # fi=1 (0xEA): lane 0 fetches inactive lane 7's VGPR -> 10 + 7 = 17
    w1 = run_wave((vop1("v_mov_b32", 2, DPP8_FI), dpp8_word(1, sels)),
                  init_v={1: v1, 2: [888] * 32}, exec_mask=exec_mask)
    assert w1.v[2][0] == 17


def test_hlsl_translates_sdwa_dpp16_and_dpp8():
    code = with_lane_offset(
        vop1("v_mov_b32", 1, const(5)),
        vop2("v_add_nc_u32", 2, SDWA, 1),
        sdwa_word(1, dst_sel=BYTE_1, dst_unused=UNUSED_PRESERVE, src0_sel=BYTE_0, src1_sel=BYTE_0),
        vop1("v_mov_b32", 3, DPP16),
        dpp16_word(1, 0x140, bound_ctrl=1),
        vop1("v_mov_b32", 4, DPP8_FI),
        dpp8_word(1, [7, 6, 5, 4, 3, 2, 1, 0]),
        *store_lane(2, 0),
        *store_lane(3, 1),
        *store_lane(4, 2),
    )
    hlsl = rdna2_hlsl.translate(code)
    assert "ReadLane(" in hlsl
    assert "0xFFu" in hlsl


@gpu
def test_sdwa_and_dpp_on_gpu_match_interpreter():
    code = with_lane_offset(
        vop2("v_add_nc_u32", 1, const(1), V + 0),
        vop2("v_add_nc_u32", 2, SDWA, 1),
        sdwa_word(1, dst_sel=WORD_0, dst_unused=UNUSED_PAD, src0_sel=BYTE_0, src1_sel=BYTE_0),
        vop1("v_mov_b32", 3, DPP16),
        dpp16_word(1, 0x101, bound_ctrl=1),
        vop1("v_mov_b32", 4, DPP8_FI),
        dpp8_word(1, [7, 6, 5, 4, 3, 2, 1, 0]),
        *store_lane(2, 0),
        *store_lane(3, 1),
        *store_lane(4, 2),
    )
    expected, actual = compare(code)
    assert actual == expected


def test_sdwa_integer_clamp_raises_and_cvt_sext_works():
    ins_clamp = program(
        vop2("v_add_nc_u32", 2, SDWA, 1),
        sdwa_word(1, dst_sel=DWORD, clamp=1, src0_sel=DWORD, src1_sel=DWORD),
    )
    with pytest.raises(rdna2_exec.Untranslated, match="v_add_nc_u32"):
        rdna2_exec.run(ins_clamp)
    with pytest.raises(rdna2_hlsl.Untranslatable, match="v_add_nc_u32"):
        rdna2_hlsl.translate(ins_clamp)

    # v_cvt_f32_i32_sdwa with sext(BYTE_0) on 0x0000_00FE -> -2 -> -2.0f
    wave = run_wave(
        (vop1("v_cvt_f32_i32", 2, SDWA), sdwa_word(1, dst_sel=DWORD, src0_sel=BYTE_0, src0_sext=1)),
        init_v={1: [0x1234_56FE] * 32},
    )
    assert wave.v[2][0] == float_bits(-2.0)


@gpu
def test_sdwa_cvt_f32_i32_sext_negative_byte_matches_gpu():
    code = with_lane_offset(
        vop1("v_mov_b32", 1, 255),
        0x1234_56FE,
        vop1("v_cvt_f32_i32", 2, SDWA),
        sdwa_word(1, dst_sel=DWORD, src0_sel=BYTE_0, src0_sext=1),
        *store_lane(2, 0),
    )
    expected, actual = compare(code)
    assert actual == expected
