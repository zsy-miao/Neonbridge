"""Tests MIMG phase 2 : formats 2D additionnels (8_UNORM, 8_8_UNORM, 32_FLOAT, 32_32_FLOAT),
décalages signés (_o) et LOD explicite (_l / _mip), dans l'interpréteur et sur GPU (@gpu).
"""
import struct

import pytest

from emu import rdna2_gpu, rdna2_hlsl, rdna2_image as images
from emu.rdna2_exec_ops import float_bits
from test_rdna2_exec import program, vop1, vop2
from test_rdna2_exec_memory import OUTPUT, global_
from test_rdna2_graphics import words
from test_rdna2_image import LITERAL, MEMORY_SIZE, S_SHARP, T_SHARP, TEXTURE, gpu, mimg, run_interpreter

ZERO, ONE_INT = 128, 129
HALF, ONE_F = 0xF0, 0xF2
DST_TEXTURE = 0x70000
T_DST = 20                                                # s[20:27]


def _pack_offset(ox, oy):
    """Encodage RDNA 2 Table 8.9 : off_x sur bits [5:0], off_y sur bits [13:8] (signés 6 bits)."""
    return (ox & 0x3F) | ((oy & 0x3F) << 8)


def _pack_tsharp(base_sgpr, address, fmt, width, height, swizzle=(4, 5, 6, 7)):
    return {base_sgpr + i: w for i, w in enumerate(images.encode_image(address, fmt, width, height, swizzle))}


def _pack_ssharp(base_sgpr=S_SHARP, **kwargs):
    return {base_sgpr + i: w for i, w in enumerate(images.encode_sampler(**kwargs))}


def _format_texture(fmt, width, height):
    data = bytearray()
    for y in range(height):
        for x in range(width):
            if fmt == images.FORMAT_8_UNORM:
                data.append((x * 29 + y * 53 + 11) & 0xFF)
            elif fmt == images.FORMAT_8_8_UNORM:
                data += bytes(((x * 29 + 7) & 0xFF, (y * 59 + 13) & 0xFF))
            elif fmt == images.FORMAT_32_FLOAT:
                data += struct.pack("<f", (x + y * width + 1) / 32.0)
            elif fmt == images.FORMAT_32_32_FLOAT:
                data += struct.pack("<2f", x + 0.25, y - 0.5)
            else:
                raise ValueError(fmt)
    return bytes(data)


def _setup(fmt, width, height, data=None, swizzle=(4, 5, 6, 7), **sampler_kwargs):
    memory = bytearray(MEMORY_SIZE)
    raw = data if data is not None else _format_texture(fmt, width, height)
    memory[TEXTURE:TEXTURE + len(raw)] = raw
    sgprs = {2: OUTPUT, 3: 0, **_pack_tsharp(T_SHARP, TEXTURE, fmt, width, height, swizzle),
             **_pack_ssharp(S_SHARP, **sampler_kwargs)}
    return memory, sgprs


@pytest.mark.parametrize("fmt", [
    images.FORMAT_8_UNORM,
    images.FORMAT_8_8_UNORM,
    images.FORMAT_32_FLOAT,
    images.FORMAT_32_32_FLOAT,
])
def test_new_formats_load_and_store_round_trip(fmt):
    memory, sgprs = _setup(fmt, 8, 4)
    sgprs.update(_pack_tsharp(T_DST, DST_TEXTURE, fmt, 8, 4))
    code = program(
        vop2("v_and_b32", 1, 128 + 7, 0),                                 # x = lane & 7
        vop2("v_lshrrev_b32", 2, 128 + 3, 0),                             # y = lane >> 3
        *mimg("image_load", 3, 1, srsrc=T_SHARP, dmask=0xF),
        *mimg("image_store", 3, 1, srsrc=T_DST, dmask=0xF),
        *mimg("image_load", 8, 1, srsrc=T_DST, dmask=0xF),
        vop2("v_lshlrev_b32", 12, 128 + 4, 0),
        *global_("global_store_dwordx4", addr=12, data=8, saddr=2))
    final = run_interpreter(code, sgprs, memory)
    tex_len = 8 * 4 * images.TEXEL_BYTES[fmt]
    assert final[DST_TEXTURE:DST_TEXTURE + tex_len] == memory[TEXTURE:TEXTURE + tex_len]
    for lane in (0, 5, 17, 31):
        got = words(final, OUTPUT + lane * 16, 4)
        assert got[2] == 0 and got[3] == images.ONE_BITS
        if fmt in (images.FORMAT_8_UNORM, images.FORMAT_32_FLOAT):
            assert got[1] == 0


def test_image_load_mip_and_store_mip_respect_mip0_and_oob():
    memory, sgprs = _setup(images.FORMAT_32_FLOAT, 8, 4)
    sgprs.update(_pack_tsharp(T_DST, DST_TEXTURE, images.FORMAT_32_FLOAT, 8, 4))
    # Lignes paires : mip = 0 ; lignes impaires : mip = 1 (OOB → lit 0, n'écrit rien)
    code = program(
        vop2("v_and_b32", 1, 128 + 7, 0),                                 # x
        vop2("v_lshrrev_b32", 2, 128 + 3, 0),                             # y
        vop2("v_and_b32", 3, ONE_INT, 0),                                 # mip = lane & 1
        *mimg("image_load_mip", 4, 1, srsrc=T_SHARP, dmask=0xF),
        *mimg("image_store_mip", 4, 1, srsrc=T_DST, dmask=0x1),
        vop2("v_lshlrev_b32", 8, 128 + 4, 0),
        *global_("global_store_dwordx4", addr=8, data=4, saddr=2))
    final = run_interpreter(code, sgprs, memory)
    for lane in range(8):
        if lane % 2 == 0:
            expected_r = struct.unpack_from("<I", memory, TEXTURE + lane * 4)[0]
            assert words(final, OUTPUT + lane * 16, 4) == [expected_r, 0, 0, images.ONE_BITS]
            assert words(final, DST_TEXTURE + lane * 4, 1) == [expected_r]
        else:
            assert words(final, OUTPUT + lane * 16, 4) == [0, 0, 0, 0]
            assert words(final, DST_TEXTURE + lane * 4, 1) == [0]


def test_offset_variants_sample_compare_and_gather4():
    # Grille 4×4 R32F : texel(x, y) = y * 4 + x + 1
    grid = struct.pack("<16f", *[float(i + 1) for i in range(16)])
    memory, sgprs = _setup(
        images.FORMAT_32_FLOAT, 4, 4, data=grid,
        clamp_x=images.CLAMP_WRAP, clamp_y=images.CLAMP_WRAP,
        mag_filter=images.FILTER_POINT, min_filter=images.FILTER_POINT,
        depth_compare=images.COMPARE_LESS,
    )
    # Coordonnées (s, t) = (0.125, 0.125) → texel (0, 0) ; offset (-1, +2) → wrap → (3, 2) = 12.0
    off = _pack_offset(-1, 2)
    for op, addrs_init, extra_regs in [
        ("image_sample_o", [off, float_bits(0.125), float_bits(0.125)], (1, 2, 3)),
        ("image_sample_lz_o", [off, float_bits(0.125), float_bits(0.125)], (1, 2, 3)),
        ("image_sample_l_o", [off, float_bits(0.125), float_bits(0.125), float_bits(0.0)], (1, 2, 3, 4)),
    ]:
        setup_insts = [w for i, val in enumerate(addrs_init) for w in (vop1("v_mov_b32", 1 + i, LITERAL), val)]
        code = program(
            *setup_insts,
            *mimg(op, 6, 1, dmask=0x1),
            vop1("v_mov_b32", 10, ZERO),
            *global_("global_store_dword", addr=10, data=6, saddr=2))
        final = run_interpreter(code, sgprs, memory)
        assert words(final, OUTPUT, 1)[0] == float_bits(12.0), op

    # image_sample_c_l_o (5 registres d'adresse via NSA : offset, z_ref, s, t, lod)
    # z_ref = 10.0 < texel(3, 2)=12.0 → 1.0
    code_clo = program(
        vop1("v_mov_b32", 10, LITERAL), off,
        vop1("v_mov_b32", 12, LITERAL), float_bits(10.0),
        vop1("v_mov_b32", 14, LITERAL), float_bits(0.125),
        vop1("v_mov_b32", 16, LITERAL), float_bits(0.125),
        vop1("v_mov_b32", 18, ZERO),
        *mimg("image_sample_c_l_o", 6, 0, dmask=0x1, nsa_addrs=(10, 12, 14, 16, 18)),
        vop1("v_mov_b32", 20, ZERO),
        *global_("global_store_dword", addr=20, data=6, saddr=2))
    final_clo = run_interpreter(code_clo, sgprs, memory)
    assert words(final_clo, OUTPUT, 1)[0] == images.ONE_BITS

    # image_gather4_o / _lz_o / _l_o à (0.25, 0.25) → footprint (0,0)..(1,1) + offset (+1, +1) → (1,1)..(2,2)
    # Ordre gather4 : (1,2)=10, (2,2)=11, (2,1)=7, (1,1)=6
    off_g = _pack_offset(1, 1)
    for op, vals in [
        ("image_gather4_o", [off_g, float_bits(0.25), float_bits(0.25)]),
        ("image_gather4_lz_o", [off_g, float_bits(0.25), float_bits(0.25)]),
        ("image_gather4_l_o", [off_g, float_bits(0.25), float_bits(0.25), float_bits(0.0)]),
    ]:
        setup_insts = [w for i, val in enumerate(vals) for w in (vop1("v_mov_b32", 1 + i, LITERAL), val)]
        code_g = program(
            *setup_insts,
            *mimg(op, 6, 1, dmask=0x1),
            vop1("v_mov_b32", 10, ZERO),
            *global_("global_store_dwordx4", addr=10, data=6, saddr=2))
        final_g = run_interpreter(code_g, sgprs, memory)
        assert words(final_g, OUTPUT, 4) == [float_bits(v) for v in (10.0, 11.0, 7.0, 6.0)], op


def test_explicit_lod_selects_min_or_mag_filter():
    # Grille 2×2 R32F : (0,0)=0, (1,0)=2, (0,1)=4, (1,1)=8
    grid = struct.pack("<4f", 0.0, 2.0, 4.0, 8.0)
    # mag = POINT, min = LINEAR : lod = 0.0 → POINT (texel(1,1)=8.0), lod = 1.0 → LINEAR (moyenne=3.5)
    memory, sgprs = _setup(
        images.FORMAT_32_FLOAT, 2, 2, data=grid,
        clamp_x=images.CLAMP_LAST_TEXEL, clamp_y=images.CLAMP_LAST_TEXEL,
        mag_filter=images.FILTER_POINT, min_filter=images.FILTER_LINEAR,
        depth_compare=images.COMPARE_LESS,
    )
    for lod, expected in [(0.0, 8.0), (1.0, 3.5)]:
        code = program(
            vop1("v_mov_b32", 1, HALF),
            vop1("v_mov_b32", 2, HALF),
            vop1("v_mov_b32", 3, LITERAL), float_bits(lod),
            *mimg("image_sample_l", 4, 1, dmask=0x1),
            vop1("v_mov_b32", 8, ZERO),
            *global_("global_store_dword", addr=8, data=4, saddr=2))
        final = run_interpreter(code, sgprs, memory)
        assert words(final, OUTPUT, 1)[0] == float_bits(expected)


def test_hlsl_translation_covers_new_formats_offsets_and_lod():
    code = program(
        *mimg("image_load_mip", 4, 1, dmask=0xF),
        *mimg("image_store_mip", 4, 1, dmask=0x3),
        *mimg("image_sample_l_o", 4, 1, dmask=0xF),
        *mimg("image_sample_c_l_o", 4, 1, dmask=0x1),
        *mimg("image_gather4_l_o", 4, 1, dmask=0x1))
    hlsl = rdna2_hlsl.translate(code)
    for token in ("ImageOffset(", "ImageUseLinear(", "InterlockedAnd(", "InterlockedOr(",
                  "ImageLoad(t, v[1], v[2], v[3])", "ImageStore(t, v[1], v[2], v[3], 3u"):
        assert token in hlsl


@gpu
@pytest.mark.parametrize("fmt", [
    images.FORMAT_8_UNORM,
    images.FORMAT_8_8_UNORM,
    images.FORMAT_32_FLOAT,
    images.FORMAT_32_32_FLOAT,
])
def test_gpu_new_formats_load_store_sample_match_interpreter(fmt):
    memory, sgprs = _setup(fmt, 8, 4, clamp_x=images.CLAMP_WRAP, clamp_y=images.CLAMP_MIRROR,
                           mag_filter=images.FILTER_POINT, min_filter=images.FILTER_LINEAR)
    sgprs.update(_pack_tsharp(T_DST, DST_TEXTURE, fmt, 8, 4))
    code = program(
        vop2("v_and_b32", 1, 128 + 7, 0),
        vop2("v_lshrrev_b32", 2, 128 + 3, 0),
        vop1("v_mov_b32", 3, ZERO),
        *mimg("image_load_mip", 4, 1, srsrc=T_SHARP, dmask=0xF),
        *mimg("image_store_mip", 4, 1, srsrc=T_DST, dmask=0xF),
        *mimg("image_load_mip", 8, 1, srsrc=T_DST, dmask=0xF),
        vop2("v_lshlrev_b32", 12, 128 + 4, 0),
        *global_("global_store_dwordx4", addr=12, data=8, saddr=2))
    expected = run_interpreter(code, sgprs, memory)
    actual = rdna2_gpu.run(rdna2_hlsl.translate(code, sgprs), bytes(memory))
    assert actual[:MEMORY_SIZE] == expected


@gpu
@pytest.mark.parametrize("op", ["image_sample_o", "image_sample_lz_o", "image_sample_l", "image_sample_l_o"])
def test_gpu_offset_and_lod_sample_variants_match_interpreter(op):
    memory, sgprs = _setup(
        images.FORMAT_32_32_FLOAT, 8, 4,
        clamp_x=images.CLAMP_WRAP, clamp_y=images.CLAMP_MIRROR,
        mag_filter=images.FILTER_POINT, min_filter=images.FILTER_LINEAR,
    )
    off = _pack_offset(-2, 1)
    insts = [
        vop1("v_mov_b32", 10, LITERAL), off,                              # offset
        vop1("v_cvt_f32_u32", 11, 256),
        vop2("v_mul_f32", 11, LITERAL, 11), float_bits(0.0625),           # s
        vop1("v_mov_b32", 12, LITERAL), float_bits(0.375),                # t
        vop1("v_mov_b32", 13, ONE_F),                                     # lod = 1.0 → min_filter (LINEAR)
    ]
    base_addr = 10 if images.has_offset(op) else 11
    code = program(
        *insts,
        *mimg(op, 3, base_addr, dmask=0xF),
        vop2("v_lshlrev_b32", 7, 128 + 4, 0),
        *global_("global_store_dwordx4", addr=7, data=3, saddr=2))
    expected = run_interpreter(code, sgprs, memory)
    actual = rdna2_gpu.run(rdna2_hlsl.translate(code, sgprs), bytes(memory))
    assert actual[:MEMORY_SIZE] == expected


@gpu
@pytest.mark.parametrize("op", ["image_sample_c_l", "image_sample_c_o", "image_sample_c_lz_o", "image_sample_c_l_o"])
def test_gpu_offset_and_lod_compare_variants_match_interpreter(op):
    memory, sgprs = _setup(
        images.FORMAT_32_FLOAT, 8, 4,
        clamp_x=images.CLAMP_WRAP, clamp_y=images.CLAMP_LAST_TEXEL,
        mag_filter=images.FILTER_POINT, min_filter=images.FILTER_LINEAR,
        depth_compare=images.COMPARE_LESS,
    )
    off = _pack_offset(1, -1)
    insts = [
        vop1("v_mov_b32", 10, LITERAL), off,
        vop1("v_cvt_f32_u32", 11, 256),
        vop2("v_mul_f32", 11, LITERAL, 11), float_bits(0.03125),          # z_ref
        vop1("v_cvt_f32_u32", 12, 256),
        vop2("v_mul_f32", 12, LITERAL, 12), float_bits(0.0625),           # s
        vop1("v_mov_b32", 13, LITERAL), float_bits(0.375),                # t
        vop1("v_mov_b32", 14, ONE_F),                                     # lod = 1.0
    ]
    base_addr = 10 if images.has_offset(op) else 11
    code = program(
        *insts,
        *mimg(op, 3, base_addr, dmask=0xF),
        vop2("v_lshlrev_b32", 7, 128 + 4, 0),
        *global_("global_store_dwordx4", addr=7, data=3, saddr=2))
    expected = run_interpreter(code, sgprs, memory)
    actual = rdna2_gpu.run(rdna2_hlsl.translate(code, sgprs), bytes(memory))
    assert actual[:MEMORY_SIZE] == expected


@gpu
@pytest.mark.parametrize("op", ["image_gather4_l", "image_gather4_o", "image_gather4_lz_o", "image_gather4_l_o"])
def test_gpu_offset_and_lod_gather4_variants_match_interpreter(op):
    memory, sgprs = _setup(
        images.FORMAT_8_8_UNORM, 8, 4,
        clamp_x=images.CLAMP_WRAP, clamp_y=images.CLAMP_MIRROR,
    )
    off = _pack_offset(-1, 2)
    insts = [
        vop1("v_mov_b32", 10, LITERAL), off,
        vop1("v_cvt_f32_u32", 11, 256),
        vop2("v_mul_f32", 11, LITERAL, 11), float_bits(0.0625),
        vop1("v_mov_b32", 12, LITERAL), float_bits(0.375),
        vop1("v_mov_b32", 13, ZERO),
    ]
    base_addr = 10 if images.has_offset(op) else 11
    code = program(
        *insts,
        *mimg(op, 3, base_addr, dmask=0x2),
        vop2("v_lshlrev_b32", 7, 128 + 4, 0),
        *global_("global_store_dwordx4", addr=7, data=3, saddr=2))
    expected = run_interpreter(code, sgprs, memory)
    actual = rdna2_gpu.run(rdna2_hlsl.translate(code, sgprs), bytes(memory))
    assert actual[:MEMORY_SIZE] == expected
