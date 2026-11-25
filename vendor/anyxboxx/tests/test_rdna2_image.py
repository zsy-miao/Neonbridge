"""Textures RDNA 2 : descripteurs T#/S# et image_sample, dans l'interpréteur puis sur le GPU du PC.

Chaque programme tourne dans l'interpréteur de référence et sur le GPU (ou WARP) : les mémoires
finales, ou les exports de chaque pixel, doivent être identiques au bit près.
"""
import struct

import pytest

from emu import rdna2_exec, rdna2_exec_graphics, rdna2_gpu, rdna2_graphics as graphics
from emu import rdna2_hlsl, rdna2_hlsl_graphics, rdna2_image as images
from emu.rdna2_exec_ops import float_bits
from emu.rdna2_hlsl_ops import Untranslatable
from emu.rdna2_ops import MNEMONICS
from test_rdna2_exec import program, vop1, vop2
from test_rdna2_exec_memory import OUTPUT, global_
from test_rdna2_graphics import HEIGHT, VERTEX_SHADER, WIDTH, exp, run_vertex_pass, vintrp, words

OPCODES = {(family, name): opcode for (family, opcode), name in MNEMONICS.items()}
HALF, LITERAL = 0xF0, 255
MEMORY_SIZE = 0x80000
TEXTURE = 0x60000
T_SHARP, S_SHARP = 8, 16                          # s[8:15] et s[16:19]
gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc ou anyxboxx_gpu_run.exe absent")


def mimg(name, vdata, vaddr, srsrc=T_SHARP, ssamp=S_SHARP, dmask=0xF, dim=images.DIM_2D, unorm=0, nsa_addrs=None):
    op = OPCODES["MIMG", name]
    nsa = 1 if nsa_addrs else 0
    low = 0x3C << 26 | (op & 0x7F) << 18 | unorm << 12 | dmask << 8 | dim << 3 | nsa << 1 | (op >> 7) & 1
    first_addr = nsa_addrs[0] if nsa_addrs else vaddr
    high = (ssamp // 4) << 21 | (srsrc // 4) << 16 | vdata << 8 | first_addr
    if not nsa_addrs:
        return low, high
    extra = sum((reg & 0xFF) << (8 * i) for i, reg in enumerate(nsa_addrs[1:]))
    return low, high, extra


def texture_bytes(fmt, width, height):
    """Texels distincts : (x, y) → couleur, en 8_8_8_8_UNORM ou 32_32_32_32_FLOAT."""
    data = bytearray()
    for y in range(height):
        for x in range(width):
            if fmt == images.FORMAT_8_8_8_8_UNORM:
                data += bytes((x * 30 + 7, (y * 60 + 3) & 0xFF, (x * y * 17) & 0xFF, 255 - x))
            else:
                data += struct.pack("<4f", x + 0.25, y - 0.5, x * y / 8, -1.0)
    return bytes(data)


def descriptors(fmt, width, height, clamp, swizzle=(4, 5, 6, 7), address=TEXTURE):
    words_t = images.encode_image(address, fmt, width, height, swizzle)
    sampler = images.encode_sampler(clamp, clamp)
    return {**{T_SHARP + i: w for i, w in enumerate(words_t)}, **{S_SHARP + i: w for i, w in enumerate(sampler)}}


def sampling_program(dmask=0xF):
    """Ligne n : s = (n + 0,5) / 8, t = 0,5 ; écrit 16 octets à OUTPUT + n × 16."""
    return program(
        vop1("v_cvt_f32_u32", 1, 256),                                    # v1 = n
        vop2("v_add_f32", 1, HALF, 1),
        vop2("v_mul_f32", 1, LITERAL, 1), float_bits(0.125),
        vop1("v_mov_b32", 2, HALF),
        *mimg("image_sample", 3, 1, dmask=dmask),
        vop2("v_lshlrev_b32", 7, 128 + 4, 0),                             # v7 = n × 16
        *global_("global_store_dwordx4", addr=7, data=3, saddr=2))


def run_interpreter(data, sgprs, memory):
    wave = rdna2_exec.Wave()
    wave.memory.write(0, bytes(memory))
    for number, value in sgprs.items():
        wave.s[number] = value
    rdna2_exec.run(data, wave)
    return wave.memory.read(0, MEMORY_SIZE)


def setup(fmt, width, height, clamp, swizzle=(4, 5, 6, 7)):
    memory = bytearray(MEMORY_SIZE)
    texture = texture_bytes(fmt, width, height)
    memory[TEXTURE:TEXTURE + len(texture)] = texture
    return memory, {2: OUTPUT, 3: 0, **descriptors(fmt, width, height, clamp, swizzle)}


def test_descriptors_round_trip():
    found = images.image(images.encode_image(0x12345600, images.FORMAT_8_8_8_8_UNORM, 300, 17, (6, 5, 4, 1)))
    assert (found["address"], found["width"], found["height"], found["swizzle"]) == (0x12345600, 300, 17, [6, 5, 4, 1])
    assert images.sampler(images.encode_sampler(images.CLAMP_WRAP, images.CLAMP_LAST_TEXEL))["clamp_y"] == 2


@pytest.mark.parametrize("clamp, expected_x", [(images.CLAMP_WRAP, lambda n: n % 8),
                                               (images.CLAMP_LAST_TEXEL, lambda n: min(n, 7))])
def test_interpreter_samples_the_nearest_texel(clamp, expected_x):
    memory, sgprs = setup(images.FORMAT_8_8_8_8_UNORM, 8, 4, clamp)
    final = run_interpreter(sampling_program(), sgprs, memory)
    for lane in (0, 5, 9, 31):
        x, y = expected_x(lane), 2
        texel = memory[TEXTURE + (y * 8 + x) * 4:TEXTURE + (y * 8 + x) * 4 + 4]
        assert words(final, OUTPUT + lane * 16, 4) == [images.UNORM8[c] for c in texel], f"ligne {lane}"


def test_dmask_and_dst_sel_pick_the_written_components():
    memory, sgprs = setup(images.FORMAT_32_32_32_32_FLOAT, 4, 4, images.CLAMP_LAST_TEXEL, swizzle=(6, 5, 4, 1))
    final = run_interpreter(sampling_program(dmask=0b1001), sgprs, memory)
    texel = struct.unpack_from("<4I", memory, TEXTURE + (2 * 4 + 0) * 16)    # ligne 0 : texel (0, 2)
    assert words(final, OUTPUT, 2) == [texel[2], images.ONE_BITS]           # composantes x (= z) et w (= 1)


def test_unsupported_cases_are_named():
    memory, sgprs = setup(images.FORMAT_8_8_8_8_UNORM, 8, 4, images.CLAMP_WRAP)
    one_d = program(*mimg("image_sample", 3, 1, dim=0))
    with pytest.raises(rdna2_exec.Untranslated, match="dimension 0"):
        run_interpreter(one_d, sgprs, memory)
    with pytest.raises(Untranslatable, match="dimension 0"):
        rdna2_hlsl.translate(one_d)
    with pytest.raises(rdna2_exec.Untranslated, match="anisotrope"):
        run_interpreter(sampling_program(), {**sgprs, S_SHARP + 2: 2 << 20}, memory)
    with pytest.raises(rdna2_exec.Untranslated, match="GFX10_FORMAT_16_16_16_16_FLOAT"):
        run_interpreter(sampling_program(), {**sgprs, **descriptors(71, 8, 4, images.CLAMP_WRAP)}, memory)


@gpu
@pytest.mark.parametrize("fmt, clamp, swizzle, dmask", [
    (images.FORMAT_8_8_8_8_UNORM, images.CLAMP_WRAP, (4, 5, 6, 7), 0xF),
    (images.FORMAT_32_32_32_32_FLOAT, images.CLAMP_LAST_TEXEL, (4, 5, 6, 7), 0xF),
    (images.FORMAT_8_8_8_8_UNORM, images.CLAMP_LAST_TEXEL, (6, 5, 4, 1), 0b1011),
])
def test_compute_sampling_matches_the_interpreter(fmt, clamp, swizzle, dmask):
    memory, sgprs = setup(fmt, 8, 4, clamp, swizzle)
    data = sampling_program(dmask)
    expected = run_interpreter(data, sgprs, memory)
    actual = rdna2_gpu.run(rdna2_hlsl.translate(data, sgprs), bytes(memory))
    assert actual[:MEMORY_SIZE] == expected


PIXEL_TEXTURE = 0x7C000                          # après PIXEL_RECORDS (64 pixels)
TEXTURED_PIXEL_SHADER = program(
    *[word for channel in range(2) for word in (*vintrp("v_interp_p1_f32", 2 + channel, 0, 0, channel),
                                                 *vintrp("v_interp_p2_f32", 2 + channel, 1, 0, channel))],
    *mimg("image_sample", 4, 2),                                          # (s, t) = param0.xy
    *exp(graphics.TARGET_MRT0, (4, 5, 6, 7), done=True))


@gpu
def test_textured_pixel_shader_matches_the_interpreter():
    vertex_hlsl, _, after_vertices = run_vertex_pass()
    memory = bytearray(after_vertices)
    records = memory[graphics.VERTEX_RECORDS:graphics.VERTEX_RECORDS + 6 * graphics.VERTEX_STRIDE]
    area = graphics.parameters(bytes(records), 2)
    memory[graphics.PARAMETERS:graphics.PARAMETERS + len(area)] = area
    texture = texture_bytes(images.FORMAT_8_8_8_8_UNORM, 8, 8)
    memory[PIXEL_TEXTURE:PIXEL_TEXTURE + len(texture)] = texture
    sgprs = descriptors(images.FORMAT_8_8_8_8_UNORM, 8, 8, images.CLAMP_WRAP, address=PIXEL_TEXTURE)
    pixel_hlsl = rdna2_hlsl_graphics.translate_pixel(TEXTURED_PIXEL_SHADER, WIDTH, sgprs)
    after = rdna2_gpu.draw(vertex_hlsl, pixel_hlsl, bytes(memory), 6, WIDTH, HEIGHT)
    colors = set()
    for pixel in range(WIDTH * HEIGHT):
        record = graphics.PIXEL_RECORDS + pixel * graphics.PIXEL_STRIDE
        flag, i_bits, j_bits, primitive = words(after, record, 4)
        assert flag, f"pixel {pixel} non dessiné"
        expected = rdna2_exec_graphics.run_pixel(TEXTURED_PIXEL_SHADER, i_bits, j_bits, primitive, sgprs,
                                                 bytes(memory))
        assert words(after, record + 16, 4) == expected[graphics.TARGET_MRT0], f"pixel {pixel}"
        colors.add(tuple(expected[graphics.TARGET_MRT0]))
    assert len(colors) > 4                                                 # la texture varie vraiment


def test_vertex_shader_is_still_valid_with_image_helpers():
    assert "ImageTexel" in rdna2_hlsl_graphics.translate_vertex(VERTEX_SHADER)
