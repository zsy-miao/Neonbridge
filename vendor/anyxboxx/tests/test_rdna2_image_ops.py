"""Tests MIMG étendus : image_load/store/resinfo, bilinéaire, adressages, sample_c et gather4.

Vérifie la parité bit-à-bit entre l'interpréteur (emu/rdna2_exec_image.py) et le HLSL généré
(emu/rdna2_hlsl_image.py), sur CPU et sur le GPU (@gpu).
"""
import struct

import pytest

from emu import rdna2_exec, rdna2_exec_image, rdna2_gpu, rdna2_graphics as graphics
from emu import rdna2_hlsl, rdna2_hlsl_graphics, rdna2_image as images
from emu.rdna2_exec_ops import float_bits
from emu.rdna2_hlsl_ops import Untranslatable
from test_rdna2_exec import program, vop1, vop2
from test_rdna2_exec_memory import OUTPUT, global_
from test_rdna2_graphics import exp, vintrp, words
from test_rdna2_image import (LITERAL, MEMORY_SIZE, S_SHARP, T_SHARP, TEXTURE, gpu, mimg,
                              run_interpreter, texture_bytes)

ZERO, ONE_INT, NEG_ONE_INT = 128, 129, 193
HALF, ONE_F = 0xF0, 0xF2
DST_TEXTURE = 0x70000
T_DST = 20                                                # s[20:27]


def _pack_tsharp(base_sgpr, address, fmt, width, height, swizzle=(4, 5, 6, 7)):
    return {base_sgpr + i: w for i, w in enumerate(images.encode_image(address, fmt, width, height, swizzle))}


def _pack_ssharp(base_sgpr=S_SHARP, **kwargs):
    return {base_sgpr + i: w for i, w in enumerate(images.encode_sampler(**kwargs))}


def _setup_grid(fmt, width, height, data=None, swizzle=(4, 5, 6, 7), **sampler_kwargs):
    memory = bytearray(MEMORY_SIZE)
    raw = data if data is not None else texture_bytes(fmt, width, height)
    memory[TEXTURE:TEXTURE + len(raw)] = raw
    sgprs = {2: OUTPUT, 3: 0, **_pack_tsharp(T_SHARP, TEXTURE, fmt, width, height, swizzle),
             **_pack_ssharp(S_SHARP, **sampler_kwargs)}
    return memory, sgprs


def test_image_descriptor_rejects_mipmapped_levels():
    single = images.image(images.encode_image(TEXTURE, images.FORMAT_8_8_8_8_UNORM, 8, 4))
    assert (single["base_level"], single["last_level"]) == (0, 0)
    for base_level, last_level in ((0, 2), (1, 1), (1, 3)):
        t_words = images.encode_image(TEXTURE, images.FORMAT_8_8_8_8_UNORM, 8, 4,
                                      base_level=base_level, last_level=last_level)
        with pytest.raises(images.ImageError, match="niveaux mip"):
            images.image(t_words)
    memory, sgprs = _setup_grid(images.FORMAT_8_8_8_8_UNORM, 8, 4)
    for i, word in enumerate(images.encode_image(TEXTURE, images.FORMAT_8_8_8_8_UNORM, 8, 4, last_level=3)):
        sgprs[T_SHARP + i] = word
    for op in ("image_sample", "image_get_resinfo"):
        code = program(*mimg(op, 3, 1, dmask=0xF))
        with pytest.raises(rdna2_exec.Untranslated, match="niveaux mip"):
            run_interpreter(code, sgprs, memory)


def test_sampler_descriptor_extended_round_trip():
    encoded = images.encode_sampler(
        clamp_x=images.CLAMP_MIRROR, clamp_y=images.CLAMP_BORDER, unnormalized=1,
        mag_filter=images.FILTER_LINEAR, min_filter=images.FILTER_LINEAR,
        depth_compare=images.COMPARE_LEQUAL, border_color=images.BORDER_OPAQUE_WHITE)
    parsed = images.sampler(encoded)
    assert parsed == {
        "clamp_x": images.CLAMP_MIRROR, "clamp_y": images.CLAMP_BORDER,
        "depth_compare": images.COMPARE_LEQUAL, "unnormalized": 1,
        "mag_filter": images.FILTER_LINEAR, "min_filter": images.FILTER_LINEAR,
        "border_color": images.BORDER_OPAQUE_WHITE,
    }
    with pytest.raises(images.ImageError, match="adressage 4"):
        images.sampler(images.encode_sampler(clamp_x=4))
    with pytest.raises(images.ImageError, match="anisotrope"):
        images.sampler(images.encode_sampler(mag_filter=2))
    with pytest.raises(images.ImageError, match="bordure 3"):
        images.sampler(images.encode_sampler(border_color=3))


def test_image_load_in_bounds_oob_and_swizzle():
    memory, sgprs = _setup_grid(images.FORMAT_8_8_8_8_UNORM, 4, 4, swizzle=(6, 5, 4, 1))
    # Met un S# volontairement invalide : image_load ne doit pas le lire.
    sgprs[S_SHARP + 2] = 3 << 20
    # Ligne n : x = (n & 7) - 1 (donc -1..6, teste négatif et >= 4), y = n >> 3 (0..3)
    code = program(
        vop2("v_and_b32", 1, 128 + 7, 0),
        vop2("v_add_nc_u32", 1, NEG_ONE_INT, 1),                          # v1 = x
        vop2("v_lshrrev_b32", 2, 128 + 3, 0),                             # v2 = y
        *mimg("image_load", 3, 1, dmask=0b1101),                          # écrit R, B, A dans v3..v5
        vop1("v_mov_b32", 6, ZERO),
        vop2("v_lshlrev_b32", 7, 128 + 4, 0),
        *global_("global_store_dwordx4", addr=7, data=3, saddr=2))
    final = run_interpreter(code, sgprs, memory)
    for lane in range(32):
        x = (lane & 7) - 1
        y = lane >> 3
        got = words(final, OUTPUT + lane * 16, 3)
        if 0 <= x < 4 and 0 <= y < 4:
            px = memory[TEXTURE + (y * 4 + x) * 4:TEXTURE + (y * 4 + x) * 4 + 4]
            assert got == [images.UNORM8[px[2]], images.UNORM8[px[0]], images.ONE_BITS]
        else:
            assert got == [0, 0, 0]


def test_image_store_and_reload_float32_and_unorm8():
    # Vérifie le round-trip exact des 256 valeurs UNORM8
    for c in range(256):
        assert rdna2_exec_image.float_to_unorm8(images.UNORM8[c]) == c
    assert rdna2_exec_image.float_to_unorm8(float_bits(-0.5)) == 0
    assert rdna2_exec_image.float_to_unorm8(float_bits(1.5)) == 255
    assert rdna2_exec_image.float_to_unorm8(0x7FC00000) == 0              # NaN → 0

    # 1) FORMAT_32_32_32_32_FLOAT : écriture complète puis écriture partielle (dmask=0b0101) + OOB ignoré
    memory, sgprs = _setup_grid(images.FORMAT_32_32_32_32_FLOAT, 4, 4)
    sgprs.update(_pack_tsharp(T_DST, DST_TEXTURE, images.FORMAT_32_32_32_32_FLOAT, 4, 4))
    code_f32 = program(
        vop2("v_and_b32", 1, 128 + 3, 0),                                 # v1 = lane & 3 (0..3)
        vop2("v_lshrrev_b32", 2, 128 + 2, 0),                             # v2 = lane >> 2 (0..7 : >=4 est OOB)
        vop1("v_cvt_f32_u32", 3, 256),                                    # R = float(lane)
        vop1("v_mov_b32", 4, HALF),                                       # G = 0.5
        vop1("v_mov_b32", 5, ONE_F),                                      # B = 1.0
        vop1("v_mov_b32", 6, LITERAL), float_bits(-2.25),                 # A = -2.25
        *mimg("image_store", 3, 1, srsrc=T_DST, dmask=0xF),
        # Écrase seulement R et B avec dmask=0b0101
        vop1("v_mov_b32", 8, LITERAL), float_bits(9.5),
        vop1("v_mov_b32", 9, LITERAL), float_bits(-3.0),
        *mimg("image_store", 8, 1, srsrc=T_DST, dmask=0b0101),
        *mimg("image_load", 10, 1, srsrc=T_DST, dmask=0xF),
        vop2("v_lshlrev_b32", 14, 128 + 4, 0),
        *global_("global_store_dwordx4", addr=14, data=10, saddr=2))
    final = run_interpreter(code_f32, sgprs, memory)
    for lane in range(16):
        assert words(final, OUTPUT + lane * 16, 4) == [
            float_bits(9.5), float_bits(0.5), float_bits(-3.0), float_bits(-2.25)]
    for lane in range(16, 32):
        assert words(final, OUTPUT + lane * 16, 4) == [0, 0, 0, 0]
    assert final[DST_TEXTURE + 16 * 16:DST_TEXTURE + 32 * 16] == bytes(16 * 16)

    # 2) FORMAT_8_8_8_8_UNORM : écriture complète puis écriture partielle (dmask=0b1010 → G et A)
    memory_u8, sgprs_u8 = _setup_grid(images.FORMAT_8_8_8_8_UNORM, 4, 4)
    sgprs_u8.update(_pack_tsharp(T_DST, DST_TEXTURE, images.FORMAT_8_8_8_8_UNORM, 4, 4))
    code_u8 = program(
        vop2("v_and_b32", 1, 128 + 3, 0),
        vop2("v_lshrrev_b32", 2, 128 + 2, 0),
        vop1("v_mov_b32", 3, LITERAL), images.UNORM8[42],
        vop1("v_mov_b32", 4, LITERAL), images.UNORM8[100],
        vop1("v_mov_b32", 5, LITERAL), images.UNORM8[200],
        vop1("v_mov_b32", 6, ONE_F),
        *mimg("image_store", 3, 1, srsrc=T_DST, dmask=0xF),
        vop1("v_mov_b32", 8, LITERAL), images.UNORM8[77],                 # nouveau G
        vop1("v_mov_b32", 9, ZERO),                                       # nouveau A = 0
        *mimg("image_store", 8, 1, srsrc=T_DST, dmask=0b1010),
        *mimg("image_load", 10, 1, srsrc=T_DST, dmask=0xF),
        vop2("v_lshlrev_b32", 14, 128 + 4, 0),
        *global_("global_store_dwordx4", addr=14, data=10, saddr=2))
    final_u8 = run_interpreter(code_u8, sgprs_u8, memory_u8)
    assert list(final_u8[DST_TEXTURE:DST_TEXTURE + 4]) == [42, 77, 200, 0]
    assert words(final_u8, OUTPUT, 4) == [images.UNORM8[42], images.UNORM8[77], images.UNORM8[200], 0]


def test_image_get_resinfo_mip0_and_oob_mip():
    memory, sgprs = _setup_grid(images.FORMAT_8_8_8_8_UNORM, 19, 7, data=b"", swizzle=(0, 0, 0, 0))
    # Ligne 0 : mip = 0 ; ligne 1 : mip = 1
    code = program(
        vop2("v_and_b32", 1, ONE_INT, 0),                                 # v1 = lane & 1
        *mimg("image_get_resinfo", 3, 1, dmask=0xF),
        vop2("v_lshlrev_b32", 7, 128 + 4, 0),
        *global_("global_store_dwordx4", addr=7, data=3, saddr=2))
    final = run_interpreter(code, sgprs, memory)
    assert words(final, OUTPUT, 4) == [19, 7, 1, 1]                       # swizzle ignoré
    assert words(final, OUTPUT + 16, 4) == [0, 0, 0, 1]                   # mip hors plage


@pytest.mark.parametrize("clamp, border, coords, expected_x", [
    (images.CLAMP_MIRROR, images.BORDER_TRANS_BLACK, [-1.5, -0.5, 0.5, 3.5, 4.5, 7.5], [1, 0, 0, 3, 3, 0]),
    (images.CLAMP_MIRROR_ONCE_LAST_TEXEL, images.BORDER_TRANS_BLACK, [-5.5, -1.5, -0.5, 2.5, 6.5], [3, 1, 0, 2, 3]),
    (images.CLAMP_BORDER, images.BORDER_OPAQUE_BLACK, [-0.5, 0.5, 3.5, 4.5], [-1, 0, 3, -1]),
    (images.CLAMP_BORDER, images.BORDER_OPAQUE_WHITE, [-1.5, 1.5, 4.5], [-1, 1, -1]),
    (images.CLAMP_MIRROR_ONCE_BORDER, images.BORDER_OPAQUE_WHITE, [-4.5, -3.5, -0.5, 3.5, 4.5], [-1, 3, 0, 3, -1]),
])
def test_clamp_modes_mirror_mirror_once_and_border(clamp, border, coords, expected_x):
    memory, sgprs = _setup_grid(images.FORMAT_32_32_32_32_FLOAT, 4, 2, clamp_x=clamp, clamp_y=clamp,
                                unnormalized=1, border_color=border)
    for x_coord, exp_x in zip(coords, expected_x):
        code = program(
            vop1("v_mov_b32", 1, LITERAL), float_bits(x_coord),
            vop1("v_mov_b32", 2, HALF),
            *mimg("image_sample_lz", 3, 1, dmask=0xF),
            vop1("v_mov_b32", 7, ZERO),
            *global_("global_store_dwordx4", addr=7, data=3, saddr=2))
        final = run_interpreter(code, sgprs, memory)
        got = words(final, OUTPUT, 4)
        if exp_x < 0:
            assert got == rdna2_exec_image.BORDER_COLORS[border]
        else:
            assert got == list(struct.unpack_from("<4I", memory, TEXTURE + exp_x * 16))


def test_bilinear_filtering_exact_float32_and_lz():
    # Grille 2×2 RGBA32F simple : (0,0)=0, (1,0)=2, (0,1)=4, (1,1)=8 sur le canal R
    grid = struct.pack("<16f",
                       0.0, 1.0, 0.0, 1.0,
                       2.0, 3.0, 1.0, 1.0,
                       4.0, 5.0, 2.0, 1.0,
                       8.0, 9.0, 3.0, 1.0)
    memory, sgprs = _setup_grid(images.FORMAT_32_32_32_32_FLOAT, 2, 2, data=grid,
                                clamp_x=images.CLAMP_LAST_TEXEL, clamp_y=images.CLAMP_LAST_TEXEL,
                                mag_filter=images.FILTER_LINEAR, min_filter=images.FILTER_LINEAR)
    # (s, t) = (0.5, 0.5) → u = 1.0, u - 0.5 = 0.5 (milieu exact des 4 texels) → R = (0+2+4+8)/4 = 3.5
    # (s, t) = (0.375, 0.625) → fx = 0.25, fy = 0.75 → top = 0.5, bot = 5.0, res = 0.5 + (5.0 - 0.5)*0.75 = 3.875
    for op, (s, t), expected_r in [
        ("image_sample", (0.25, 0.25), 0.0),                              # centre du texel (0, 0)
        ("image_sample", (0.5, 0.5), 3.5),                                # centre exact des 4 texels
        ("image_sample_lz", (0.375, 0.625), 3.875),
    ]:
        code = program(
            vop1("v_mov_b32", 1, LITERAL), float_bits(s),
            vop1("v_mov_b32", 2, LITERAL), float_bits(t),
            *mimg(op, 3, 1, dmask=0xF),
            vop1("v_mov_b32", 7, ZERO),
            *global_("global_store_dwordx4", addr=7, data=3, saddr=2))
        final = run_interpreter(code, sgprs, memory)
        assert words(final, OUTPUT, 1)[0] == float_bits(expected_r)

    # Bilinéaire + CLAMP_BORDER au bord gauche (s = 0.0 → x0 = -1 bordure blanche 1.0, x1 = 0 texel 0.0, fx = 0.5)
    memory_b, sgprs_b = _setup_grid(images.FORMAT_32_32_32_32_FLOAT, 2, 2, data=grid,
                                    clamp_x=images.CLAMP_BORDER, clamp_y=images.CLAMP_LAST_TEXEL,
                                    mag_filter=images.FILTER_LINEAR, min_filter=images.FILTER_LINEAR,
                                    border_color=images.BORDER_OPAQUE_WHITE)
    code_b = program(
        vop1("v_mov_b32", 1, ZERO),
        vop1("v_mov_b32", 2, LITERAL), float_bits(0.25),
        *mimg("image_sample_lz", 3, 1, dmask=0x1),
        vop1("v_mov_b32", 7, ZERO),
        *global_("global_store_dword", addr=7, data=3, saddr=2))
    final_b = run_interpreter(code_b, sgprs_b, memory_b)
    assert words(final_b, OUTPUT, 1)[0] == float_bits(0.5)


def test_image_sample_c_and_c_lz_point_and_bilinear_pcf():
    # Carte de profondeur 2×2 : (0,0)=0.2, (1,0)=0.4, (0,1)=0.6, (1,1)=0.8
    depth = struct.pack("<16f",
                        0.2, 0.0, 0.0, 1.0,
                        0.4, 0.0, 0.0, 1.0,
                        0.6, 0.0, 0.0, 1.0,
                        0.8, 0.0, 0.0, 1.0)
    # 1) PCF bilinéaire avec LESS au centre (0.5, 0.5) : z_ref = 0.5 < {0.6, 0.8} (2 sur 4 → 0.5)
    memory, sgprs = _setup_grid(images.FORMAT_32_32_32_32_FLOAT, 2, 2, data=depth,
                                clamp_x=images.CLAMP_LAST_TEXEL, clamp_y=images.CLAMP_LAST_TEXEL,
                                mag_filter=images.FILTER_LINEAR, min_filter=images.FILTER_LINEAR,
                                depth_compare=images.COMPARE_LESS)
    for op, z_ref, expected in [
        ("image_sample_c", 0.1, 1.0),                                     # < les 4 texels → 1.0
        ("image_sample_c_lz", 0.3, 0.75),                                 # < 3 sur 4 → 0.75
        ("image_sample_c_lz", 0.5, 0.5),                                  # < 2 sur 4 → 0.5
        ("image_sample_c", 0.7, 0.25),                                    # < 1 sur 4 → 0.25
        ("image_sample_c_lz", 0.9, 0.0),                                  # < 0 sur 4 → 0.0
    ]:
        code = program(
            vop1("v_mov_b32", 1, LITERAL), float_bits(z_ref),             # v1 = z_ref
            vop1("v_mov_b32", 2, HALF),                                   # v2 = s
            vop1("v_mov_b32", 3, HALF),                                   # v3 = t
            *mimg(op, 4, 1, dmask=0xF),
            vop1("v_mov_b32", 8, ZERO),
            *global_("global_store_dwordx4", addr=8, data=4, saddr=2))
        final = run_interpreter(code, sgprs, memory)
        assert words(final, OUTPUT, 4) == [float_bits(expected)] * 3 + [images.ONE_BITS]

    # 2) Point compare : vérifie NEVER, EQUAL, NOTEQUAL (avec NaN), GREATER, ALWAYS
    for func, z_bits, expected in [
        (images.COMPARE_NEVER, float_bits(0.0), 0),
        (images.COMPARE_EQUAL, float_bits(0.2), images.ONE_BITS),
        (images.COMPARE_GREATER, float_bits(0.5), images.ONE_BITS),
        (images.COMPARE_NOTEQUAL, 0x7FC00000, images.ONE_BITS),           # NaN != 0.2 est vrai
        (images.COMPARE_LESS, 0x7FC00000, 0),                             # NaN < 0.2 est faux
        (images.COMPARE_ALWAYS, float_bits(9.0), images.ONE_BITS),
    ]:
        memory_p, sgprs_p = _setup_grid(images.FORMAT_32_32_32_32_FLOAT, 2, 2, data=depth,
                                        mag_filter=images.FILTER_POINT, depth_compare=func)
        code_p = program(
            vop1("v_mov_b32", 1, LITERAL), z_bits,
            vop1("v_mov_b32", 2, LITERAL), float_bits(0.25),
            vop1("v_mov_b32", 3, LITERAL), float_bits(0.25),
            *mimg("image_sample_c_lz", 4, 1, dmask=0x1),
            vop1("v_mov_b32", 8, ZERO),
            *global_("global_store_dword", addr=8, data=4, saddr=2))
        final_p = run_interpreter(code_p, sgprs_p, memory_p)
        assert words(final_p, OUTPUT, 1)[0] == expected


def test_image_gather4_and_gather4_lz_order_and_nsa():
    # Grille 2×2 RGBA32F : R = (10, 20, 30, 40), G = (1, 2, 3, 4)
    grid = struct.pack("<16f",
                       10.0, 1.0, 0.0, 1.0,                               # (0, 0)
                       20.0, 2.0, 0.0, 1.0,                               # (1, 0)
                       30.0, 3.0, 0.0, 1.0,                               # (0, 1)
                       40.0, 4.0, 0.0, 1.0)                               # (1, 1)
    memory, sgprs = _setup_grid(images.FORMAT_32_32_32_32_FLOAT, 2, 2, data=grid,
                                clamp_x=images.CLAMP_LAST_TEXEL, clamp_y=images.CLAMP_LAST_TEXEL)
    # Ordre RDNA 2 / D3D12 : (x0, y1), (x1, y1), (x1, y0), (x0, y0)
    # Avec NSA : s dans v10, t dans v12 ; dmask=0x1 (R) puis dmask=0x2 (G)
    for op, dmask, expected in [
        ("image_gather4", 0x1, [30.0, 40.0, 20.0, 10.0]),
        ("image_gather4_lz", 0x2, [3.0, 4.0, 2.0, 1.0]),
    ]:
        code = program(
            vop1("v_mov_b32", 10, HALF),
            vop1("v_mov_b32", 12, HALF),
            *mimg(op, 4, 0, dmask=dmask, nsa_addrs=(10, 12)),
            vop1("v_mov_b32", 8, ZERO),
            *global_("global_store_dwordx4", addr=8, data=4, saddr=2))
        final = run_interpreter(code, sgprs, memory)
        assert words(final, OUTPUT, 4) == [float_bits(v) for v in expected]

    # dmask à plusieurs bits refusé pour gather4
    bad = program(*mimg("image_gather4", 4, 1, dmask=0x3))
    with pytest.raises(rdna2_exec.Untranslated, match="un seul canal"):
        run_interpreter(bad, sgprs, memory)
    with pytest.raises(Untranslatable, match="un seul canal"):
        rdna2_hlsl.translate(bad)


def test_hlsl_translation_emits_expected_helpers_and_barriers():
    code = program(
        *mimg("image_get_resinfo", 3, 1, dmask=0xF),
        *mimg("image_store", 3, 1, dmask=0xF),
        *mimg("image_load", 1, 1, dmask=0xF),                             # vdata chevauche vaddr
        *mimg("image_sample_lz", 4, 1, dmask=0xF),
        *mimg("image_sample_c_lz", 4, 1, dmask=0x1),
        *mimg("image_gather4_lz", 4, 1, dmask=0x2))
    hlsl = rdna2_hlsl.translate(code)
    for token in ("ImageResInfo(", "ImageStore(", "AllMemoryBarrierWithGroupSync();",
                  "ImageLoad(", "ImageSampleColor(", "ImageSampleCompare(", "ImageGather4("):
        assert token in hlsl
    # Pixel shader avec image_gather4_lz ne doit contenir aucune barrière compute
    ps = program(
        *vintrp("v_interp_p1_f32", 2, 0, 0, 0),
        *vintrp("v_interp_p2_f32", 2, 1, 0, 0),
        *vintrp("v_interp_p1_f32", 3, 0, 0, 1),
        *vintrp("v_interp_p2_f32", 3, 1, 0, 1),
        *mimg("image_gather4_lz", 4, 2, dmask=0x1),
        *exp(graphics.TARGET_MRT0, (4, 5, 6, 7), done=True))
    ps_hlsl = rdna2_hlsl_graphics.translate_pixel(ps, 8)
    assert "ImageGather4(" in ps_hlsl and "AllMemoryBarrierWithGroupSync" not in ps_hlsl


@gpu
@pytest.mark.parametrize("fmt", [images.FORMAT_8_8_8_8_UNORM, images.FORMAT_32_32_32_32_FLOAT])
def test_gpu_image_load_store_resinfo_matches_interpreter(fmt):
    memory, sgprs = _setup_grid(fmt, 8, 4, swizzle=(6, 5, 4, 1))
    sgprs.update(_pack_tsharp(T_DST, DST_TEXTURE, fmt, 8, 4, swizzle=(4, 5, 6, 7)))
    code = program(
        vop2("v_and_b32", 1, 128 + 7, 0),                                 # x = lane & 7 (0..7)
        vop2("v_lshrrev_b32", 2, 128 + 3, 0),                             # y = lane >> 3 (0..3)
        *mimg("image_load", 3, 1, srsrc=T_SHARP, dmask=0xF),
        *mimg("image_store", 3, 1, srsrc=T_DST, dmask=0xF),
        # Écrase partiellement R et B dans DST_TEXTURE
        vop1("v_mov_b32", 8, HALF),
        vop1("v_mov_b32", 9, ONE_F),
        *mimg("image_store", 8, 1, srsrc=T_DST, dmask=0b0101),
        *mimg("image_load", 10, 1, srsrc=T_DST, dmask=0xF),
        vop2("v_lshlrev_b32", 14, 128 + 4, 0),
        *global_("global_store_dwordx4", addr=14, data=10, saddr=2),
        # Stocke resinfo (mip = lane & 1) après les 32 texels
        vop2("v_and_b32", 15, ONE_INT, 0),
        *mimg("image_get_resinfo", 16, 15, srsrc=T_DST, dmask=0xF),
        vop2("v_add_nc_u32", 20, LITERAL, 14), 512,
        *global_("global_store_dwordx4", addr=20, data=16, saddr=2))
    expected = run_interpreter(code, sgprs, memory)
    actual = rdna2_gpu.run(rdna2_hlsl.translate(code, sgprs), bytes(memory))
    assert actual[:MEMORY_SIZE] == expected


@gpu
@pytest.mark.parametrize("clamp", sorted(images.SUPPORTED_CLAMPS))
@pytest.mark.parametrize("filt", [images.FILTER_POINT, images.FILTER_LINEAR])
def test_gpu_bilinear_and_clamp_modes_match_interpreter(clamp, filt):
    border = (images.BORDER_OPAQUE_WHITE if clamp == images.CLAMP_BORDER
              else images.BORDER_OPAQUE_BLACK if clamp == images.CLAMP_MIRROR_ONCE_BORDER
              else images.BORDER_TRANS_BLACK)
    memory, sgprs = _setup_grid(images.FORMAT_8_8_8_8_UNORM, 8, 4,
                                clamp_x=clamp, clamp_y=clamp,
                                mag_filter=filt, min_filter=filt, border_color=border)
    # Coordonnées normalisées s = (lane - 6) * 0.09375 (couvre < 0, [0, 1] fractionnaire et > 1)
    code = program(
        vop1("v_cvt_f32_u32", 1, 256),
        vop2("v_add_f32", 1, LITERAL, 1), float_bits(-6.0),
        vop2("v_mul_f32", 1, LITERAL, 1), float_bits(0.09375),
        vop1("v_mov_b32", 2, LITERAL), float_bits(0.375),
        *mimg("image_sample_lz", 3, 1, dmask=0xF),
        vop2("v_lshlrev_b32", 7, 128 + 4, 0),
        *global_("global_store_dwordx4", addr=7, data=3, saddr=2))
    expected = run_interpreter(code, sgprs, memory)
    actual = rdna2_gpu.run(rdna2_hlsl.translate(code, sgprs), bytes(memory))
    assert actual[:MEMORY_SIZE] == expected


@gpu
@pytest.mark.parametrize("op", ["image_sample_c", "image_sample_c_lz"])
@pytest.mark.parametrize("filt", [images.FILTER_POINT, images.FILTER_LINEAR])
def test_gpu_image_sample_c_pcf_matches_interpreter(op, filt):
    memory, sgprs = _setup_grid(images.FORMAT_32_32_32_32_FLOAT, 8, 4,
                                clamp_x=images.CLAMP_LAST_TEXEL, clamp_y=images.CLAMP_LAST_TEXEL,
                                mag_filter=filt, min_filter=filt,
                                depth_compare=images.COMPARE_LESS)
    code = program(
        vop1("v_cvt_f32_u32", 10, 256),
        vop2("v_mul_f32", 10, LITERAL, 10), float_bits(0.25),             # z_ref dans v10
        vop1("v_cvt_f32_u32", 12, 256),
        vop2("v_mul_f32", 12, LITERAL, 12), float_bits(0.03125),          # s dans v12
        vop1("v_mov_b32", 14, LITERAL), float_bits(0.375),                # t dans v14
        *mimg(op, 3, 0, dmask=0xF, nsa_addrs=(10, 12, 14)),
        vop2("v_lshlrev_b32", 7, 128 + 4, 0),
        *global_("global_store_dwordx4", addr=7, data=3, saddr=2))
    expected = run_interpreter(code, sgprs, memory)
    actual = rdna2_gpu.run(rdna2_hlsl.translate(code, sgprs), bytes(memory))
    assert actual[:MEMORY_SIZE] == expected


@gpu
@pytest.mark.parametrize("op", ["image_gather4", "image_gather4_lz"])
def test_gpu_image_gather4_matches_interpreter(op):
    memory, sgprs = _setup_grid(images.FORMAT_8_8_8_8_UNORM, 8, 4, swizzle=(6, 5, 4, 7),
                                clamp_x=images.CLAMP_WRAP, clamp_y=images.CLAMP_MIRROR)
    code = program(
        vop1("v_cvt_f32_u32", 1, 256),
        vop2("v_mul_f32", 1, LITERAL, 1), float_bits(0.0625),
        vop1("v_mov_b32", 2, LITERAL), float_bits(0.375),
        *mimg(op, 3, 1, dmask=0x2),
        vop2("v_lshlrev_b32", 7, 128 + 4, 0),
        *global_("global_store_dwordx4", addr=7, data=3, saddr=2))
    expected = run_interpreter(code, sgprs, memory)
    actual = rdna2_gpu.run(rdna2_hlsl.translate(code, sgprs), bytes(memory))
    assert actual[:MEMORY_SIZE] == expected
