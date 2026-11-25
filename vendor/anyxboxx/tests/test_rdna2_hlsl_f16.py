"""Demi-précision (f16) dans le recompilateur HLSL : la mémoire finale du GPU (ou de WARP) doit être celle
de l'interpréteur de référence, arrondis, dénormaux, infinis et NaN compris."""
import random
import struct

import pytest

from emu import rdna2_gpu, rdna2_hlsl, rdna2_hlsl_graphics
from test_rdna2_exec import EXEC_LO, LITERAL, ONE, V, const, program, sop1, sop2, vop1, vop2, vop3
from test_rdna2_exec_memory import INPUT, global_
from test_rdna2_f16 import REFUSED_VOP3
from test_rdna2_graphics import PIXEL_SHADER, VERTEX_SHADER, WIDTH
from test_rdna2_hlsl import compare, store_lane, with_lane_offset

gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc ou anyxboxx_gpu_run.exe absent")
NAMES = ["v_add_f16", "v_sub_f16", "v_subrev_f16", "v_mul_f16", "v_fmac_f16", "v_fmamk_f16", "v_fmaak_f16",
         "v_max_f16", "v_min_f16", "v_ldexp_f16", "v_cvt_f16_f32", "v_cvt_f32_f16", "v_fma_f16"]

# Une ligne du wave par rangée (a, b, c : moitiés f16 ; w : mot f32) : arrondis à égalité, dépassements,
# dénormaux, ±0, infinis, NaN, exposants de ldexp extrêmes ; les dernières lignes sont tirées au hasard.
ROWS = [(0x3C00, 0x4000, 0x3C00, 0x3F80_0000), (0x3C01, 0x3C01, 0x0000, 0x477F_F000),
        (0x101F, 0x3BC4, 0x3C00, 0x477F_EFFF), (0x7BFF, 0x7BFF, 0x7BFF, 0x3F80_1000),
        (0x7C00, 0xFC00, 0x3C00, 0x3F80_3000), (0x0001, 0x3800, 0x8001, 0x0000_0001),
        (0x0003, 0x3800, 0x0000, 0x8000_0001), (0x8000, 0x0000, 0x8000, 0x33C0_0000),
        (0x0001, 0x8001, 0x0000, 0x3300_0000), (0x7E00, 0x3C00, 0x7E00, 0x3300_0001),
        (0xFE01, 0x7C01, 0x3C00, 0x7FC0_0001), (0x7C00, 0x0000, 0x0000, 0xFF80_0000),
        (0x3C00, 0x1000, 0x8000, 0x387F_FFFF), (0x3C01, 0x1000, 0x0000, 0xC77F_E000),
        (0x0400, 0x8001, 0x0400, 0x3880_0000), (0x03FF, 0x0001, 0x83FF, 0x7F80_0000),
        (0x3C00, 0xFFE8, 0x0000, 0x0000_0000), (0x3C00, 0xFFE7, 0x8000, 0x8000_0000),
        (0x3E00, 0xFFE7, 0x3C00, 0x4780_0000), (0x3C00, 0x7FFF, 0xBC00, 0x3880_1000),
        (0x3C00, 0x8000, 0x3C00, 0x3380_0000), (0xBC00, 0x0010, 0x7BFF, 0xC000_0000)]
ROWS += [tuple(draw.getrandbits(bits) for bits in (16, 16, 16, 32))       # un générateur par ligne : a, b, c, w
         for draw in (random.Random(115 + lane) for lane in range(len(ROWS), 32))]
# v1, v2, v4 ← a, b, c (moitiés hautes marquées par la ligne), v5 ← w, v6 ← graine de la destination.
LOADS = [word for slot, register in enumerate((1, 2, 4, 5, 6))
         for word in global_("global_load_dword", addr=31, vdst=register, saddr=0, offset=slot * 128)]


def image():
    memory = bytearray(INPUT + 5 * 128)
    for lane, (a, b, c, w) in enumerate(ROWS):
        words = ((0xA500 | lane) << 16 | a, (0x5A00 | lane) << 16 | b, (0x3300 | lane) << 16 | c, w, 0x1234_5678 ^ lane)
        for slot, word in enumerate(words):
            struct.pack_into("<I", memory, INPUT + slot * 128 + lane * 4, word)
    return bytes(memory)


def into_v10(*operations):
    """Chaque opération écrit v10, d'abord rempli avec la graine v6, puis v10 est rangé dans son emplacement.
    Le déplacement de global_store tient sur 12 bits signés : la sortie s[2] avance de 2 Kio tous les 16 emplacements."""
    words = []
    for index, operation in enumerate(operations):
        if index and index % 16 == 0:
            words += [sop2("s_add_u32", 2, 2, LITERAL), 0x800]
        words += [vop1("v_mov_b32", 10, V + 6), *operation, *store_lane(10, index % 16)]
    return words


def test_compute_translation_carries_the_f16_helpers():
    hlsl = rdna2_hlsl.translate(program(vop2("v_add_f16", 3, V + 1, 2)))
    assert "uint F32ToF16(uint f)" in hlsl
    assert "v[3] = (v[3] & 0xFFFF0000u) | F16Add((v[1] & 0xFFFFu), (v[2] & 0xFFFFu));" in hlsl


def test_vertex_and_pixel_shaders_carry_the_f16_helpers():
    assert "uint F16ToF32(uint h)" in rdna2_hlsl_graphics.translate_vertex(VERTEX_SHADER)
    assert "uint F16ToF32(uint h)" in rdna2_hlsl_graphics.translate_pixel(PIXEL_SHADER, WIDTH)


def test_wave64_translation_carries_the_f16_helpers():
    hlsl = rdna2_hlsl.translate(program(vop2("v_add_f16", 3, V + 1, 2), *vop3("v_fma_f16", 4, V + 1, V + 2, V + 3)),
                                wave_size=64)
    assert "uint F32ToF16(uint f)" in hlsl
    assert "F16Add((v[1] & 0xFFFFu), (v[2] & 0xFFFFu))" in hlsl
    assert "F16Fma(" in hlsl.split("[numthreads", 1)[1]


@pytest.mark.parametrize("code", [0xE9, 0xEA, 0xF9, 0xFA])
def test_sdwa_and_dpp_f16_forms_are_untranslatable(code):
    with pytest.raises(rdna2_hlsl.Untranslatable, match="v_add_f16.*SDWA/DPP"):
        rdna2_hlsl.translate(program(vop2("v_add_f16", 3, code, 2), 0))


def test_coverage_names_include_f16():
    assert set(NAMES) <= rdna2_hlsl.coverage_names()


@gpu
def test_f16_vop1_vop2_match_the_interpreter():
    operations = [(vop2(name, 10, V + 1, 2),) for name in ("v_add_f16", "v_sub_f16", "v_subrev_f16", "v_mul_f16",
                                                            "v_min_f16", "v_max_f16", "v_ldexp_f16", "v_fmac_f16")]
    operations += [(vop2("v_fmamk_f16", 10, V + 1, 2), 0x3BC4), (vop2("v_fmaak_f16", 10, V + 1, 2), 0x3C00),
                   (vop1("v_cvt_f16_f32", 10, V + 5),), (vop1("v_cvt_f32_f16", 10, V + 1),),
                   (vop2("v_add_f16", 10, ONE, 2),), (vop2("v_add_f16", 10, const(1), 2),),
                   (vop2("v_add_f16", 10, LITERAL, 2), 0xDEAD_3C00), (vop2("v_add_f16", 10, 4, 2),),
                   # x - x, 0 × x, 0 + x, 0 - x : aucun repli du pilote ne doit changer un infini, un NaN ou le signe de 0
                   (vop2("v_sub_f16", 10, V + 1, 1),), (vop2("v_mul_f16", 10, const(0), 1),),
                   (vop2("v_add_f16", 10, const(0), 1),), (vop2("v_sub_f16", 10, const(0), 1),)]
    expected, actual = compare(with_lane_offset(*LOADS, sop1("s_mov_b32", 4, LITERAL), 0xFFFF_3800,
                                                *into_v10(*operations)), memory=image())
    assert actual == expected


@gpu
def test_f16_exec_mask_and_aliasing_match_the_interpreter():
    expected, actual = compare(with_lane_offset(
        *LOADS, sop1("s_mov_b32", EXEC_LO, LITERAL), 0x5555_5555,
        vop2("v_add_f16", 1, V + 1, 2), vop2("v_fmac_f16", 2, V + 2, 2), vop1("v_cvt_f16_f32", 5, V + 5),
        sop1("s_mov_b32", EXEC_LO, 193),                                                         # EXEC = -1
        *store_lane(1, 0), *store_lane(2, 1), *store_lane(5, 2)), memory=image())
    assert actual == expected


@pytest.mark.parametrize("words, pattern", REFUSED_VOP3)
def test_unsourced_vop3_forms_are_untranslatable(words, pattern):
    with pytest.raises(rdna2_hlsl.Untranslatable, match=pattern):
        rdna2_hlsl.translate(program(*words))


@gpu
def test_f16_vop3_match_the_interpreter():
    operations = [vop3("v_add_f16", 10, V + 1, V + 2, op_sel=op_sel) for op_sel in (0b0001, 0b0010, 0b1000, 0b1011)]
    operations += [vop3(name, 10, V + 1, V + 2, neg=0b01, abs_=0b10)
                   for name in ("v_sub_f16", "v_mul_f16", "v_min_f16", "v_max_f16")]
    operations += [vop3("v_add_f16", 10, V + 1, V + 2, clamp=1), vop3("v_mul_f16", 10, V + 1, V + 2, clamp=1),
                   vop3("v_fma_f16", 10, V + 1, V + 2, V + 4), vop3("v_fma_f16", 10, V + 1, V + 2, V + 4, op_sel=0b0100),
                   vop3("v_fma_f16", 10, V + 1, V + 2, V + 4, neg=0b100), vop3("v_fmac_f16", 10, V + 1, V + 2),
                   (*vop3("v_fmamk_f16", 10, LITERAL, V + 2), 0x0000_4000),
                   vop3("v_cvt_f16_f32", 10, V + 5, neg=0b001, op_sel=0b1000),
                   vop3("v_cvt_f32_f16", 10, V + 1, op_sel=0b0001),
                   vop3("v_ldexp_f16", 10, V + 1, V + 2, op_sel=0b0010), vop3("v_ldexp_f16", 10, V + 1, const(3)),
                   (*vop3("v_add_f16", 10, V + 1, LITERAL), 0x1234_3800),
                   vop3("v_add_f16", 10, V + 1, const(0)), vop3("v_fma_f16", 10, V + 1, V + 2, const(0))]  # x + 0, x × y + 0
    expected, actual = compare(with_lane_offset(*LOADS, *into_v10(*operations)), memory=image())
    assert actual == expected
