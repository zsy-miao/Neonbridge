"""Accès mémoire typés recompilés en HLSL : mêmes octets que l'interpréteur de référence, sur GPU ou WARP.

Chaque famille de formats lit les mêmes octets pseudo-aléatoires (plus des valeurs limites sur les
premières lignes) à travers plusieurs V# ; les composantes sont écrites en mémoire et comparées.
Valeurs calculées à la main : tests/test_rdna2_exec_format.py.
"""
import random
import struct

import pytest

from emu import rdna2_exec, rdna2_gpu, rdna2_hlsl, rdna2_hlsl_graphics
from emu import rdna2_exec_graphics, rdna2_graphics as graphics
from emu.rdna2_hlsl_ops import Untranslatable
import test_rdna2_hlsl_wave64 as wave64
from test_rdna2_exec import const, program, sop1, vop1, vop2
from test_rdna2_exec_format import XYZW, typed
from test_rdna2_exec_memory import BUFFER, INPUT, OUTPUT
from test_rdna2_graphics import exp
from test_rdna2_hlsl import MEMORY_SIZE, store_lane, with_lane_offset

LITERAL, LANES, STRIDE = 255, 32, 16
ELEMENTS = BUFFER + 0x1000                  # éléments écrits par les tests d'écriture
gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc ou anyxboxx_gpu_run.exe absent")


def element_bytes(seed):
    """32 éléments de 16 octets : lignes 0 à 3 aux limites, les autres pseudo-aléatoires."""
    generator = random.Random(seed)
    data = bytearray(generator.getrandbits(8) for _ in range(LANES * STRIDE))
    data[0:16] = bytes(16)                                           # zéros
    data[16:32] = b"\xff" * 16                                       # tout à 1 : -1, max UNORM, NaN f16
    data[32:48] = bytes.fromhex("0080 0080 0080 0080 0100 0100 0100 0100")      # -2^15 ; 1 (dénormal f16)
    data[48:64] = bytes.fromhex("0180 ff7f 007c 00fc 01fc ff7b 0004 8000")      # -32767, 32767, ±inf, NaN…
    return bytes(data)


def descriptor(fmt, records=LANES, stride=STRIDE, selects=XYZW, base=BUFFER):
    """{SGPR: valeur} d'un V# (base 32 bits) à placer à partir d'un multiple de 4."""
    return [base, stride << 16, records, fmt << 12 | selects]


def run_compare(words, descriptors, memory=b"", extra_sgprs=None):
    """Programme v31 = ligne × 4, puis `words` ; descripteurs en s24, s28, s32…"""
    sgprs = {0: INPUT, 2: OUTPUT, **(extra_sgprs or {})}
    for index, words4 in enumerate(descriptors):
        sgprs.update({24 + 4 * index + i: value for i, value in enumerate(words4)})
    data = with_lane_offset(*words)
    image = bytearray(MEMORY_SIZE)
    image[:len(memory)] = memory
    wave = rdna2_exec.Wave()
    wave.memory.write(0, bytes(image))
    for number, value in sgprs.items():
        wave.s[number] = value
    rdna2_exec.run(data, wave)
    expected = wave.memory.read(0, MEMORY_SIZE)
    return expected, rdna2_gpu.run(rdna2_hlsl.translate(data, sgprs), bytes(image))


def loads(specs):
    """(nom, V# n°, options) → chargement dans v8… puis une case de sortie par composante."""
    words, slot = [], 0
    for name, rsrc_index, options in specs:
        count = len(name.rsplit("_", 1)[1])
        words += typed(name, rsrc=24 + 4 * rsrc_index, **options)
        for component in range(count):
            words += store_lane(8 + component, slot)
            slot += 1
    assert slot <= 16
    return words


def memory_with(*blocks):
    memory = bytearray(MEMORY_SIZE)
    for address, data in blocks:
        memory[address:address + len(data)] = data
    return bytes(memory)


def check(words, descriptors, memory, extra_sgprs=None):
    expected, actual = run_compare(words, descriptors, memory, extra_sgprs)
    assert actual == expected


INDEXED = {"vaddr": 0, "idxen": 1}                      # index = v0 = ligne


@gpu
@pytest.mark.parametrize("formats", [
    (65, 66, 57, 51),            # 16_16_16_16_UNORM/SNORM, 8_8_8_8_SNORM, 2_10_10_10_SNORM
    (56, 50, 44, 45),            # 8_8_8_8_UNORM, 2_10_10_10_UNORM, 10_10_10_2_UNORM/SNORM
])
def test_normalized_formats_match(formats):
    memory = memory_with((BUFFER, element_bytes(1)))
    words = loads([("buffer_load_format_xyzw", i, INDEXED) for i in range(4)])
    check(words, [descriptor(fmt) for fmt in formats], memory)


@gpu
def test_normalized_three_component_and_narrow_formats_match():
    # 10_11_11 et 11_11_10 UNORM/SNORM (xyz) ; 16_16_UNORM et 8_8_SNORM (xy).
    selects = 4 | 5 << 3 | 6 << 6 | 1 << 9
    words = loads([("buffer_load_format_xyz", i, INDEXED) for i in range(4)]
                  + [("buffer_load_format_xy", 4, INDEXED)])
    descriptors = [descriptor(fmt, selects=selects) for fmt in (30, 31, 37, 38)] + [descriptor(23, selects=0o1054)]
    check(words, descriptors, memory_with((BUFFER, element_bytes(2))))


@gpu
@pytest.mark.parametrize("formats", [
    (67, 68, 69, 70),            # 16_16_16_16_USCALED/SSCALED/UINT/SINT
    (58, 59, 61, 71),            # 8_8_8_8_USCALED/SSCALED/SINT, 16_16_16_16_FLOAT
    (47, 52, 49, 77),            # 10_10_10_2_SSCALED, 2_10_10_10_USCALED, 10_10_10_2_SINT, 32_32_32_32_FLOAT
])
def test_scaled_integer_and_float_formats_match(formats):
    memory = memory_with((BUFFER, element_bytes(3)))
    words = loads([("buffer_load_format_xyzw", i, INDEXED) for i in range(4)])
    check(words, [descriptor(fmt) for fmt in formats], memory)


@gpu
def test_dst_sel_bounds_and_unaligned_elements_match():
    # V# 0 : 8_8_8_8_UNORM brut (pas nul), décalage v1 = ligne × 5 (élément à cheval sur deux mots),
    #        DST_SEL W, 1, X, 0 ; 100 octets : les lignes 20 et plus sont hors du buffer.
    # V# 1 : 16_16_16_16_SINT brut à la ligne × 6 + 2 (élément sur trois mots), DST_SEL Z, Y, 1, W.
    # V# 2 : 16_FLOAT indexé, 20 éléments, DST_SEL X, 0, 1, 1.
    selects0 = 7 | 1 << 3 | 4 << 6 | 0 << 9
    selects1 = 6 | 5 << 3 | 1 << 6 | 7 << 9
    words = [vop2("v_mul_u32_u24", 1, const(5), 0), vop2("v_mul_u32_u24", 2, const(6), 0),
             vop2("v_add_nc_u32", 2, const(2), 2)]
    words += loads([("buffer_load_format_xyzw", 0, {"vaddr": 1, "offen": 1}),
                    ("buffer_load_format_xyzw", 1, {"vaddr": 2, "offen": 1}),
                    ("buffer_load_format_xyzw", 2, INDEXED)])
    descriptors = [descriptor(56, records=100, stride=0, selects=selects0),
                   descriptor(70, records=LANES * 6, stride=0, selects=selects1),
                   descriptor(13, records=20, stride=2, selects=4 | 1 << 6 | 1 << 9)]
    check(words, descriptors, memory_with((BUFFER, element_bytes(4))))


@gpu
def test_tbuffer_loads_match():
    # Formats de l'instruction : 8_8_8_8_SNORM, 16_16_FLOAT, 2_10_10_10_UINT, 32_32_32_UINT ; V# 0 en
    # 32_FLOAT et DST_SEL nuls (sans effet), V# 1 en FORMAT INVALID (lecture nulle). soffset = s40.
    words = loads([("tbuffer_load_format_xyzw", 0, {**INDEXED, "fmt": 57}),
                   ("tbuffer_load_format_xy", 0, {**INDEXED, "fmt": 29, "soffset": 40}),
                   ("tbuffer_load_format_xyzw", 0, {**INDEXED, "fmt": 54, "offset": 4}),
                   ("tbuffer_load_format_xyz", 0, {**INDEXED, "fmt": 72}),
                   ("tbuffer_load_format_x", 1, {**INDEXED, "fmt": 22})])
    descriptors = [descriptor(22, selects=0), descriptor(0, selects=0)]
    check(words, descriptors, memory_with((BUFFER, element_bytes(5))), extra_sgprs={40: 8})


def store_values():
    """v8 = ligne × 4099, v9 = 50000 - v8, v10 = v8 × 37, v11 = -v8 : valeurs dans et hors des champs."""
    return [vop2("v_mul_u32_u24", 8, LITERAL, 0), 4099,
            vop2("v_sub_nc_u32", 9, LITERAL, 8), 50000,
            vop2("v_mul_u32_u24", 10, const(37), 8),
            vop2("v_sub_nc_u32", 11, const(0), 8)]


@gpu
def test_integer_stores_match():
    # V# 0 : 8_UINT indexé, pas de 1 (4 lignes par mot : écritures partielles) ;
    # V# 1 : 16_16_SINT indexé, pas de 6 (moitiés de mots) ; V# 2 : 2_10_10_10_UINT, 30 éléments ;
    # V# 3 : 32_32_FLOAT brut, décalage ligne × 8 ; tbuffer 8_8_8_8_SINT (V# 2) et 11_11_10_UINT
    # avec un V# de FORMAT INVALID (V# 4) : rien d'écrit.
    selects2 = 4 | 5 << 3 | 1 << 6 | 1 << 9
    words = store_values() + [vop2("v_lshlrev_b32", 1, const(3), 0)]
    words += typed("buffer_store_format_x", vaddr=0, idxen=1, rsrc=24)
    words += typed("buffer_store_format_xy", vaddr=0, vdata=9, idxen=1, rsrc=28)
    words += typed("buffer_store_format_xyzw", vaddr=0, idxen=1, rsrc=32)
    words += typed("buffer_store_format_xy", vaddr=1, vdata=10, offen=1, rsrc=36)
    words += typed("tbuffer_store_format_xyzw", vaddr=0, idxen=1, rsrc=32, fmt=61, offset=0x200)
    words += typed("tbuffer_store_format_xyz", vaddr=0, idxen=1, rsrc=40, fmt=41)
    descriptors = [descriptor(5, stride=1, selects=4, base=ELEMENTS),
                   descriptor(28, stride=6, selects=selects2, base=ELEMENTS + 0x100),
                   descriptor(54, records=30, stride=4, base=ELEMENTS + 0x400),
                   descriptor(64, records=LANES * 8, stride=0, selects=selects2, base=ELEMENTS + 0x800),
                   descriptor(0, stride=4, base=ELEMENTS + 0xC00)]
    memory = memory_with((ELEMENTS, bytes([0xA5]) * 0x1000))
    check(words, descriptors, memory)


def test_untranslatable_forms_are_named():
    for words, message in [
        (typed("tbuffer_store_format_x", fmt=1), "8_UNORM"),
        (typed("tbuffer_load_format_x", fmt=36), "10_11_11_FLOAT"),
        (typed("tbuffer_load_format_xyzw", fmt=64), "4 composantes"),
        (typed("buffer_load_format_x", tfe=1), "tfe"),
        (typed("buffer_load_format_d16_x"), "buffer_load_format_d16_x"),
    ]:
        with pytest.raises(Untranslatable, match=message):
            rdna2_hlsl.translate(program(*words))


@gpu
def test_wave64_typed_loads_and_stores_match():
    # V# en s20-s23 : 8_8_8_8_UNORM, pas de 8, 60 éléments (lignes 60 à 63 hors du buffer) ;
    # tbuffer 16_16_SINT écrit (v9, v10) à BUFFER + 0x800 + ligne × 8.
    words = [sop1("s_mov_b32", 21, LITERAL), 8 << 16, sop1("s_mov_b32", 22, const(60)),
             sop1("s_mov_b32", 23, LITERAL), 56 << 12 | XYZW, *store_values(),
             *typed("buffer_load_format_xyzw", vaddr=0, idxen=1, rsrc=20),
             *[word for c in range(4) for word in wave64.store_lane(8 + c, c)],
             *typed("tbuffer_store_format_xy", vaddr=0, vdata=9, idxen=1, rsrc=20, fmt=28, offset=0x800)]
    generator = random.Random(6)
    memory = memory_with((BUFFER, bytes(generator.getrandbits(8) for _ in range(64 * 8))))
    expected, actual = wave64.compare(wave64.with_lane_offset(*words), memory)
    assert actual == expected


VERTEX_FETCH = program(
    *typed("tbuffer_load_format_xy", vaddr=0, vdata=2, idxen=1, rsrc=4, fmt=64),     # position 32_32_FLOAT
    vop1("v_mov_b32", 4, const(0)),
    vop1("v_mov_b32", 5, 242),                                                     # z = 0, w = 1.0
    *typed("buffer_load_format_xyzw", vaddr=0, vdata=6, idxen=1, rsrc=8, offset=8),  # couleur 8_8_8_8_UNORM
    *exp(graphics.TARGET_POS0, (2, 3, 4, 5), done=True),
    *exp(graphics.TARGET_PARAM0, (6, 7, 8, 9)))


@gpu
def test_vertex_fetch_draws_like_the_interpreter():
    # Sommets de 12 octets : x, y (float32), puis RGBA8 ; V# en s4 (format 32_FLOAT) et s8 (8_8_8_8_UNORM,
    # DST_SEL Z, Y, X, W : ordre BGRA en mémoire).
    corners = [(-1, -1), (1, -1), (-1, 1), (1, -1), (1, 1), (-1, 1)]
    vertices = b"".join(struct.pack("<2f4B", x, y, 40 * i, 255 - 7 * i, 3 * i, 128 + i)
                        for i, (x, y) in enumerate(corners))
    memory = bytearray(graphics.MEMORY_SIZE)
    memory[INPUT:INPUT + len(vertices)] = vertices
    sgprs = {4: INPUT, 5: 12 << 16, 6: 6, 7: 22 << 12 | XYZW,
             8: INPUT, 9: 12 << 16, 10: 6, 11: 56 << 12 | (6 | 5 << 3 | 4 << 6 | 7 << 9)}
    vertex_hlsl = rdna2_hlsl_graphics.translate_vertex(VERTEX_FETCH, sgprs)
    after = rdna2_gpu.draw(vertex_hlsl, None, bytes(memory), 6, 8, 8)
    for vertex in range(6):
        expected = rdna2_exec_graphics.run_vertex(VERTEX_FETCH, vertex, sgprs, bytes(memory))
        record = graphics.VERTEX_RECORDS + vertex * graphics.VERTEX_STRIDE
        assert list(struct.unpack_from("<4I", after, record)) == expected[graphics.TARGET_POS0]
        assert list(struct.unpack_from("<4I", after, record + 16)) == expected[graphics.TARGET_PARAM0]
    assert expected[graphics.TARGET_POS0][3] == 0x3F800000

