"""Accès mémoire typés de l'interpréteur RDNA 2 (buffer_*_format et tbuffer_*_format).

Valeurs attendues calculées à la main : conversions de emu/rdna2_buffer_format.py (sources dans
son en-tête). Rappels float32 : 0.2 = 0x3E4CCCCD, 0.4 = 0x3ECCCCCD, 0.6 = 0x3F19999A,
0.8 = 0x3F4CCCCD, 1/3 = 0x3EAAAAAB, 2/3 = 0x3F2AAAAB, 1.0 = 0x3F800000, -1.0 = 0xBF800000.
"""
import struct

import pytest

from emu import rdna2, rdna2_exec
from emu.rdna2_ops import MNEMONICS
from test_rdna2_exec import program, vop1
from test_rdna2_exec_memory import BUFFER

OPCODES = {(family, name): opcode
           for (family, opcode), name in {**MNEMONICS, **rdna2.ISA_ONLY_MNEMONICS}.items()}
RSRC, NULL, LITERAL = 20, 125, 255
XYZW = 4 | 5 << 3 | 6 << 6 | 7 << 9                      # DST_SEL X, Y, Z, W
ONE, MINUS_ONE = 0x3F800000, 0xBF800000
FIFTHS = [0, 0x3E4CCCCD, 0x3ECCCCCD, 0x3F19999A, 0x3F4CCCCD, ONE]   # k × 51 / 255 = k / 5


def typed(name, vaddr=0, vdata=8, offset=0, offen=0, idxen=0, fmt=0, lds=0, tfe=0, rsrc=RSRC, soffset=NULL):
    """Encodage MUBUF (buffer_*) ou MTBUF (tbuffer_*, format `fmt`) ; V# en s20-s23 par défaut."""
    if name.startswith("t"):
        op = OPCODES["MTBUF", name]
        low = 0x3A << 26 | fmt << 19 | (op & 7) << 16
        high_op = (op >> 3) << 21
    else:
        op = OPCODES["MUBUF", name]
        low = 0x38 << 26 | (op >> 7) << 25 | (op & 0x7F) << 18 | lds << 16
        high_op = 0
    low |= idxen << 13 | offen << 12 | offset
    return low, soffset << 24 | tfe << 23 | high_op | (rsrc // 4) << 16 | vdata << 8 | vaddr


def run(words, memory=b"", records=0, stride=0, word3=0, setup=()):
    wave = rdna2_exec.Wave()
    wave.memory.write(BUFFER, memory)
    wave.s[RSRC:RSRC + 4] = [BUFFER, stride << 16, records, word3]
    return rdna2_exec.run(program(*setup, *words), wave)


def word3(fmt, selects=XYZW):
    return fmt << 12 | selects


def test_unorm8_by_index_with_selects_and_bounds():
    # Élément i (pas de 4 octets) : octets 51 × (i % 6), 255 - 51 × (i % 6), 0, 255 ; 30 éléments.
    data = b"".join(bytes([51 * (i % 6), 255 - 51 * (i % 6), 0, 255]) for i in range(32))
    wave = run(typed("buffer_load_format_xyzw", idxen=1), data, records=30, stride=4, word3=word3(56))
    for lane in range(30):
        k = lane % 6
        assert [wave.v[8 + c][lane] for c in range(4)] == [FIFTHS[k], FIFTHS[5 - k], 0, ONE], lane
    assert [wave.v[8 + c][30] for c in range(4)] == [0, 0, 0, 0]          # index 30 >= 30 : hors du buffer


def test_dst_sel_constants_and_swaps():
    # 8_8_8_8_UINT, DST_SEL W, X, 1, 0 : (w, x, 1, 0) ; 1 vaut l'entier 1 pour UINT et 1.0 pour UNORM.
    data = bytes([1, 2, 3, 4])
    wave = run(typed("buffer_load_format_xyzw"), data, records=4, word3=word3(60, 7 | 4 << 3 | 1 << 6))
    assert [wave.v[8 + c][0] for c in range(4)] == [4, 1, 1, 0]
    wave = run(typed("buffer_load_format_xyzw"), data, records=4, word3=word3(56, 7 | 4 << 3 | 1 << 6))
    assert [wave.v[8 + c][0] for c in range(4)] == [0x3C808081, 0x3B808081, ONE, 0]
    # 4/255 = 2^-6 × 1.003921… : mantisse 0.003921… × 2^23 = 32896.50 → 0x8081, exposant 121 ; 1/255 : exposant 119.
    # Hors du buffer (records = 0) : les composantes lues valent 0, DST_SEL = 1 reste 1.0.
    wave = run(typed("buffer_load_format_xyzw"), data, records=0, word3=word3(56, 7 | 4 << 3 | 1 << 6))
    assert [wave.v[8 + c][0] for c in range(4)] == [0, 0, ONE, 0]


def test_signed_scaled_and_half_conversions():
    cases = [  # (format, octets, composantes attendues)
        (2, b"\x81", [MINUS_ONE]),                       # 8_SNORM : -127 / 127
        (2, b"\x80", [MINUS_ONE]),                       # -128 borné à -1
        (8, b"\xff\x7f", [ONE]),                         # 16_SNORM : 32767 / 32767
        (7, struct.pack("<H", 13107), [0x3E4CCCCD]),     # 16_UNORM : 13107 / 65535 = 0.2
        (3, b"\xc8", [0x43480000]),                      # 8_USCALED : 200.0
        (4, b"\x80", [0xC3000000]),                      # 8_SSCALED : -128.0
        (6, b"\xff", [0xFFFFFFFF]),                      # 8_SINT : -1 étendu
        (5, b"\xff", [255]),                             # 8_UINT
        (12, b"\x00\x80", [0xFFFF8000]),                 # 16_SINT : -32768
        (29, struct.pack("<2H", 0x3C00, 0xC000), [ONE, 0xC0000000]),        # 16_16_FLOAT : 1.0, -2.0
        (13, struct.pack("<H", 0x0001), [0x33800000]),   # 16_FLOAT : plus petit dénormal = 2^-24
        (13, struct.pack("<H", 0x7C01), [0x7F802000]),   # NaN signalant : charge utile << 13, pas de bit quiet
        (13, struct.pack("<H", 0xFC00), [0xFF800000]),   # -infini
    ]
    for fmt, data, expected in cases:
        name = "buffer_load_format_xy" if len(expected) == 2 else "buffer_load_format_x"
        wave = run(typed(name), data, records=4, word3=word3(fmt))
        assert [wave.v[8 + c][0] for c in range(len(expected))] == expected, fmt


def test_packed_formats():
    # 2_10_10_10_UNORM : x = 1023 (bits 9-0), y = 0, z = 341, w = 2 (bits 31-30) ;
    # 341 / 1023 = 1/3, 2 / 3 = 2/3.
    packed = struct.pack("<I", 1023 | 341 << 20 | 2 << 30)
    wave = run(typed("buffer_load_format_xyzw"), packed, records=4, word3=word3(50))
    assert [wave.v[8 + c][0] for c in range(4)] == [ONE, 0, 0x3EAAAAAB, 0x3F2AAAAB]
    # 2_10_10_10_SNORM : x = 0x200 (-512 → -1), y = 0x1FF (511 → 1), z = 0, w = 0b10 (-2 → -1).
    packed = struct.pack("<I", 0x200 | 0x1FF << 10 | 2 << 30)
    wave = run(typed("buffer_load_format_xyzw"), packed, records=4, word3=word3(51))
    assert [wave.v[8 + c][0] for c in range(4)] == [MINUS_ONE, ONE, 0, MINUS_ONE]
    # 10_10_10_2_UINT : x sur 2 bits (1-0), puis trois champs de 10 bits.
    packed = struct.pack("<I", 3 | 1000 << 2 | 5 << 12 | 1023 << 22)
    wave = run(typed("buffer_load_format_xyzw"), packed, records=4, word3=word3(48))
    assert [wave.v[8 + c][0] for c in range(4)] == [3, 1000, 5, 1023]
    # 10_11_11_SINT : x sur 11 bits (0x400 = -1024), y sur 11 bits (7), z sur 10 bits (0x3FF = -1).
    packed = struct.pack("<I", 0x400 | 7 << 11 | 0x3FF << 22)
    wave = run(typed("buffer_load_format_xyz"), packed, records=4, word3=word3(35, 4 | 5 << 3 | 6 << 6))
    assert [wave.v[8 + c][0] for c in range(3)] == [0xFFFFFC00, 7, 0xFFFFFFFF]


def test_tbuffer_uses_its_own_format_and_ignores_dst_sel():
    data = struct.pack("<4h", -2, 300, 7, 9)
    # V# : format 32_FLOAT (non INVALID) et DST_SEL tous à 0, sans effet pour MTBUF.
    wave = run(typed("tbuffer_load_format_xy", fmt=28), data, records=8, word3=word3(22, 0))
    assert [wave.v[8][0], wave.v[9][0]] == [0xFFFFFFFE, 300]                # 16_16_SINT
    wave = run(typed("tbuffer_load_format_xyz", fmt=72, offset=0), struct.pack("<3I", 5, 6, 7),
               records=12, word3=word3(22, 0))
    assert [wave.v[8 + c][0] for c in range(3)] == [5, 6, 7]                # 32_32_32_UINT
    # FORMAT INVALID dans le V# : accès désactivé, lecture nulle.
    wave = run(typed("tbuffer_load_format_xy", fmt=28), data, records=8, word3=0)
    assert [wave.v[8][0], wave.v[9][0]] == [0, 0]


def test_index_and_offset_address_a_structured_buffer():
    # Pas de 8 octets, index = v0 (ligne), décalage = v1 = 4 : deuxième mot de l'élément de la ligne.
    data = struct.pack("<64I", *[1000 + i for i in range(64)])
    wave = run(typed("buffer_load_format_x", vaddr=0, idxen=1, offen=1), data, records=32, stride=8,
               word3=word3(20), setup=[vop1("v_mov_b32", 1, 128 + 4)])
    assert wave.v[8] == [1000 + 2 * lane + 1 for lane in range(32)]


def test_integer_stores_saturate_to_the_field():
    setup = [vop1("v_mov_b32", 8, LITERAL), 70000, vop1("v_mov_b32", 9, 128 + 5)]
    wave = run(typed("buffer_store_format_xy"), bytes(8), records=8, word3=word3(27, 4 | 5 << 3 | 1 << 9),
               setup=setup)
    assert wave.memory.read(BUFFER, 4) == struct.pack("<2H", 0xFFFF, 5)       # 70000 saturé à 65535
    # 8_8_SINT : -200 borné à -128 (0x80), 100 gardé (0x64) ; écrit à l'octet 1 (alignement 1).
    setup = [vop1("v_mov_b32", 8, LITERAL), (-200) & 0xFFFFFFFF, vop1("v_mov_b32", 9, LITERAL), 100]
    wave = run(typed("buffer_store_format_xy", offset=1), b"\xaa" * 4, records=8, word3=word3(19, 4 | 5 << 3),
               setup=setup)
    assert wave.memory.read(BUFFER, 4) == b"\xaa\x80\x64\xaa"
    # tbuffer 2_10_10_10_UINT : (1024 → 1023, 1, 2, 7 → 3), V# FORMAT non INVALID.
    setup = [vop1("v_mov_b32", 8, LITERAL), 1024, vop1("v_mov_b32", 9, 128 + 1),
             vop1("v_mov_b32", 10, 128 + 2), vop1("v_mov_b32", 11, 128 + 7)]
    wave = run(typed("tbuffer_store_format_xyzw", fmt=54), bytes(4), records=4, word3=word3(22, 0), setup=setup)
    assert wave.memory.read_words(BUFFER, 1) == [1023 | 1 << 10 | 2 << 20 | 3 << 30]
    # FORMAT INVALID dans le V# : rien n'est écrit.
    wave = run(typed("tbuffer_store_format_xyzw", fmt=54), bytes(4), records=4, word3=0, setup=setup)
    assert wave.memory.read_words(BUFFER, 1) == [0]


@pytest.mark.parametrize("words, word3, message", [
    (typed("buffer_store_format_x"), word3(1, 4), "8_UNORM"),                     # écriture normalisée
    (typed("buffer_load_format_x"), word3(36, 4), "10_11_11_FLOAT"),              # flottant 11/10 bits
    (typed("buffer_load_format_x"), word3(0, 4), "INVALID"),
    (typed("buffer_load_format_xy"), word3(22, 4 | 5 << 3), "DST_SEL 5"),        # Y absent de 32_FLOAT
    (typed("buffer_store_format_xy"), word3(64, XYZW), "DST_SEL"),               # XYZW : écrit x et w (Leonida)
    (typed("tbuffer_load_format_xyzw", fmt=64), word3(22), "4 composantes"),
    (typed("buffer_load_format_x", lds=1), word3(22, 4), "lds"),
    (typed("buffer_load_format_x", tfe=1), word3(22, 4), "tfe"),
    (typed("buffer_load_format_x", offset=2), word3(22, 4), "non alignée"),
    (typed("buffer_load_format_d16_x"), word3(22, 4), "buffer_load_format_d16_x"),
    (typed("tbuffer_load_format_d16_x", fmt=22), word3(22, 4), "tbuffer_load_format_d16_x"),
    (typed("buffer_store_format_d16_x"), word3(22, 4), "buffer_store_format_d16_x"),
])
def test_unsupported_forms_are_named(words, word3, message):
    with pytest.raises(rdna2_exec.Untranslated, match=message):
        run(words, bytes(16), records=16, word3=word3)


def test_typed_buffer_instructions_count_in_coverage():
    names = {name for name in MNEMONICS.values() if "buffer_" in name and "_format_" in name}
    run_by_interpreter = {name for name in names if rdna2_exec.executable(
        "MTBUF" if name.startswith("t") else "MUBUF", name)}
    assert run_by_interpreter == {name for name in names if "_d16" not in name}
    assert len(run_by_interpreter) == 14
