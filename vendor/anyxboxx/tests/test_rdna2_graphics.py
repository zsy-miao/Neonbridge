"""Vertex et pixel shaders RDNA 2 : interpréteur (VINTRP, EXP), HLSL, puis dessin sur le GPU du PC.

Le GPU dessine avec le VS et le PS recompilés ; chaque sommet et chaque pixel enregistre ses
entrées et ses exports, que l'interpréteur de référence rejoue : les mots doivent être identiques.
"""
import struct

import pytest

from emu import rdna2_exec, rdna2_exec_graphics, rdna2_gpu, rdna2_graphics as graphics, rdna2_hlsl_graphics
from emu.rdna2_exec_ops import bits_float, float_bits
from emu.rdna2_hlsl_ops import Untranslatable
from emu.rdna2 import ISA_ONLY_MNEMONICS
from emu.rdna2_ops import MNEMONICS
from test_rdna2_exec import ONE, const, program, vop2
from test_rdna2_exec_memory import INPUT, ds, global_

OPCODES = {(family, name): opcode for (family, opcode), name in {**MNEMONICS, **ISA_ONLY_MNEMONICS}.items()}
HALF = 0xF0                                         # constante en ligne 0.5
WIDTH = HEIGHT = 8
SGPRS = {0: INPUT, 1: 0}
gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc ou anyxboxx_gpu_run.exe absent")


def vintrp(name, vdst, vsrc, attr, chan):
    return (0x32 << 26 | vdst << 18 | OPCODES["VINTRP", name] << 16 | attr << 10 | chan << 8 | vsrc,)


def exp(target, sources, enable=0xF, done=False, compr=False):
    low = 0x3E << 26 | done << 11 | compr << 10 | target << 4 | enable
    return low, sum(register << (8 * index) for index, register in enumerate(sources))


VERTEX_SHADER = program(
    vop2("v_lshlrev_b32", 1, const(5), 0),                            # v1 = sommet × 32
    *global_("global_load_dwordx4", addr=1, vdst=2, saddr=0),           # v2-v5 : position
    *global_("global_load_dwordx4", addr=1, vdst=6, saddr=0, offset=16),  # v6-v9 : couleur
    vop2("v_add_f32", 9, ONE, 9),                                      # alpha + 1
    *exp(graphics.TARGET_POS0, (2, 3, 4, 5), done=True),
    *exp(graphics.TARGET_PARAM0, (6, 7, 8, 9)))

PIXEL_SHADER = program(
    *[word for channel in range(4) for word in (*vintrp("v_interp_p1_f32", 2 + channel, 0, 0, channel),
                                                 *vintrp("v_interp_p2_f32", 2 + channel, 1, 0, channel))],
    vop2("v_mul_f32", 5, HALF, 5),                                     # alpha × 0,5
    *exp(graphics.TARGET_MRT0, (2, 3, 4, 5), done=True))


def vertex_data():
    """Deux triangles couvrant tout l'écran, couleur différente à chaque sommet."""
    corners = [(-1, -1), (1, -1), (-1, 1), (1, -1), (1, 1), (-1, 1)]
    data = bytearray()
    for index, (x, y) in enumerate(corners):
        data += struct.pack("<8f", x, y, 0.5, 1.0, 0.125 * index, 1.0 - 0.0625 * index, 0.375, 0.25 + index / 16)
    return bytes(data)


def initial_memory():
    memory = bytearray(graphics.MEMORY_SIZE)
    data = vertex_data()
    memory[INPUT:INPUT + len(data)] = data
    return memory


def words(memory, address, count):
    return list(struct.unpack_from(f"<{count}I", memory, address))


def test_interpolation_follows_the_isa_formulas():
    memory = bytearray(graphics.MEMORY_SIZE)
    struct.pack_into("<3f", memory, graphics.parameter_address(1, 0, 2), 2.0, 4.0, 8.0)   # P0, P10, P20
    shader = program(*vintrp("v_interp_p1_f32", 2, 0, 0, 2), *vintrp("v_interp_p2_f32", 2, 1, 0, 2),
                     *vintrp("v_interp_mov_f32", 3, 2, 0, 2), *exp(graphics.TARGET_MRT0, (2, 3, 2, 3), 0b0011))
    exports = rdna2_exec_graphics.run_pixel(shader, float_bits(0.25), float_bits(0.5), 1, {}, bytes(memory))
    value, copied = (bits_float(word) for word in exports[graphics.TARGET_MRT0][:2])
    assert value == 2.0 + 4.0 * 0.25 + 8.0 * 0.5 and copied == 2.0
    assert exports[graphics.TARGET_MRT0][2:] == [0, 0]                 # canaux désactivés


def test_vertex_shader_exports_are_recorded_by_the_interpreter():
    exports = rdna2_exec_graphics.run_vertex(VERTEX_SHADER, 4, SGPRS, bytes(initial_memory()))
    position = [bits_float(word) for word in exports[graphics.TARGET_POS0]]
    color = [bits_float(word) for word in exports[graphics.TARGET_PARAM0]]
    assert position == [1.0, 1.0, 0.5, 1.0] and color == [0.5, 0.75, 0.375, 1.5]


def test_parameters_are_vertex_differences():
    records = bytearray(3 * graphics.VERTEX_STRIDE)
    for vertex, value in enumerate((1.0, 3.0, 7.0)):
        struct.pack_into("<f", records, vertex * graphics.VERTEX_STRIDE + 16, value)   # param0.x
    area = graphics.parameters(bytes(records), 1)
    assert struct.unpack_from("<3f", area, 0) == (1.0, 2.0, 6.0)


def test_graphics_translation_names_what_it_cannot_do():
    with pytest.raises(Untranslatable, match="compr"):
        rdna2_hlsl_graphics.translate_pixel(program(*exp(0, (2, 3, 4, 5), compr=True)), WIDTH)
    with pytest.raises(Untranslatable, match="LDS"):
        rdna2_hlsl_graphics.translate_vertex(program(*ds("ds_write_b32", 0, data0=1)))
    with pytest.raises(Untranslatable, match="VS"):
        rdna2_hlsl_graphics.translate_vertex(program(*vintrp("v_interp_p1_f32", 2, 0, 0, 0)))
    with pytest.raises(rdna2_exec.Untranslated, match="compr"):
        rdna2_exec_graphics.run_pixel(program(*exp(0, (2, 3, 4, 5), compr=True)), 0, 0, 0, {}, b"")


def test_hlsl_stages_have_their_signatures():
    vertex = rdna2_hlsl_graphics.translate_vertex(VERTEX_SHADER, SGPRS)
    pixel = rdna2_hlsl_graphics.translate_pixel(PIXEL_SHADER, WIDTH)
    assert "VSOut main(uint vertex : SV_VertexID)" in vertex and "o_out.position = asfloat(e);" in vertex
    assert "float4 t0 : SV_Target0;" in pixel and "precise float m" in pixel


def run_vertex_pass():
    vertex_hlsl = rdna2_hlsl_graphics.translate_vertex(VERTEX_SHADER, SGPRS)
    memory = initial_memory()
    return vertex_hlsl, memory, rdna2_gpu.draw(vertex_hlsl, None, bytes(memory), 6, WIDTH, HEIGHT)


@gpu
def test_vertex_shader_on_the_gpu_matches_the_interpreter():
    _, memory, after = run_vertex_pass()
    for vertex in range(6):
        expected = rdna2_exec_graphics.run_vertex(VERTEX_SHADER, vertex, SGPRS, bytes(memory))
        record = graphics.VERTEX_RECORDS + vertex * graphics.VERTEX_STRIDE
        assert words(after, record, 4) == expected[graphics.TARGET_POS0], f"sommet {vertex}, pos0"
        assert words(after, record + 16, 4) == expected[graphics.TARGET_PARAM0], f"sommet {vertex}, param0"


@gpu
def test_pixel_shader_on_the_gpu_matches_the_interpreter():
    vertex_hlsl, _, after_vertices = run_vertex_pass()
    memory = bytearray(after_vertices)
    records = memory[graphics.VERTEX_RECORDS:graphics.VERTEX_RECORDS + 6 * graphics.VERTEX_STRIDE]
    area = graphics.parameters(bytes(records), 2)
    memory[graphics.PARAMETERS:graphics.PARAMETERS + len(area)] = area
    pixel_hlsl = rdna2_hlsl_graphics.translate_pixel(PIXEL_SHADER, WIDTH)
    after = rdna2_gpu.draw(vertex_hlsl, pixel_hlsl, bytes(memory), 6, WIDTH, HEIGHT)
    covered = 0
    for pixel in range(WIDTH * HEIGHT):
        record = graphics.PIXEL_RECORDS + pixel * graphics.PIXEL_STRIDE
        flag, i_bits, j_bits, primitive = words(after, record, 4)
        if not flag:
            continue
        covered += 1
        expected = rdna2_exec_graphics.run_pixel(PIXEL_SHADER, i_bits, j_bits, primitive, {}, bytes(memory))
        assert words(after, record + 16, 4) == expected[graphics.TARGET_MRT0], f"pixel {pixel}"
    assert covered == WIDTH * HEIGHT
