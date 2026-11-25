"""Atomiques mémoire ajoutés à l'interpréteur RDNA 2 : cmpswap, inc/dec, csub, flottants, formes
64 bits (_x2, _u64…), atomiques DS restants, ds_wrxchg2 et ds_read2st64/write2st64.

Valeurs attendues calculées à la main d'après la documentation publique d'AMD (« RDNA 2
Instruction Set Architecture », chapitres FLAT, MUBUF et DS).
"""
import struct

import pytest

from emu import rdna2_exec, rdna2_gpu, rdna2_hlsl
from emu.rdna2_exec_atomic import lds, operation, vector
from emu.rdna2_exec_ops import float_bits
from test_rdna2_exec import const, program, vop1, vop2
from test_rdna2_exec_memory import BUFFER, OUTPUT, ds, global_, mubuf, set_descriptor, set_pair
from test_rdna2_hlsl import compare, store_lane, with_lane_offset

LITERAL = 255
GLC = 1 << 14                    # bit glc d'un mot MUBUF
gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc ou anyxboxx_gpu_run.exe absent")
NAN32 = 0x7FC0_0000


def double_bits(value):
    return struct.unpack("<Q", struct.pack("<d", value))[0]


@pytest.mark.parametrize("kind, width, old, a, b, expected", [
    ("inc", 32, 5, 5, 0, 0), ("inc", 32, 4, 5, 0, 5),
    ("dec", 32, 0, 7, 0, 7), ("dec", 32, 9, 7, 0, 7), ("dec", 32, 3, 7, 0, 2),
    ("csub", 32, 10, 3, 0, 7), ("csub", 32, 2, 3, 0, 0),
    ("wrap", 32, 10, 3, 100, 7), ("wrap", 32, 2, 3, 100, 102),
    ("mskor", 32, 0xFF, 0x0F, 0x100, 0x1F0),
    ("rsub", 32, 3, 10, 0, 7),
    ("cmpswap", 32, 5, 9, 5, 9), ("cmpswap", 32, 5, 9, 6, 5),         # a = valeur écrite, b = comparée
    ("cmpst", 32, 5, 5, 9, 9), ("cmpst", 32, 5, 6, 9, 5),             # DS : a = comparée, b = écrite
    ("fcmpswap", 32, 0x8000_0000, 7, 0, 7),                           # -0 == +0
    ("fcmpswap", 32, NAN32, 7, NAN32, NAN32),                         # NaN jamais égal
    ("fmin", 32, NAN32, float_bits(1.0), 0, float_bits(1.0)),
    ("fmin", 32, float_bits(2.0), float_bits(1.0), 0, float_bits(1.0)),
    ("fmax", 32, float_bits(2.0), NAN32, 0, float_bits(2.0)),
    ("fadd", 32, float_bits(1.5), float_bits(2.25), 0, float_bits(3.75)),
    ("fmax", 64, double_bits(-1.0), double_bits(0.5), 0, double_bits(0.5)),
    ("add", 64, (1 << 64) - 1, 1, 0, 0),
    ("smin", 64, (1 << 64) - 1, 1, 0, (1 << 64) - 1),                # -1 < 1
    ("umin", 64, (1 << 64) - 1, 1, 0, 1),
    ("inc", 64, 1 << 32, 1 << 33, 0, (1 << 32) + 1),
])
def test_atomic_operations(kind, width, old, a, b, expected):
    assert operation(kind, width)(old, a, b) == expected


def test_names_are_parsed_to_width_and_operands():
    assert vector("global_atomic_cmpswap_x2")[0::2] == (64, 4)
    assert vector("buffer_atomic_inc")[0::2] == (32, 1)
    assert vector("global_atomic_add") is None                       # déjà couvert ailleurs
    assert lds("ds_cmpst_rtn_f64")[0::2] == (64, True)
    assert lds("ds_min_f32")[0::2] == (32, False)
    assert lds("ds_wrxchg_b32") is None                              # n'existe qu'avec _rtn


def lane_addresses(shift=3):
    return vop2("v_lshlrev_b32", 1, const(shift), 0)                 # v1 = ligne × 2^shift


def test_global_cmpswap_x2_swaps_only_matching_lanes():
    wave = rdna2_exec.Wave()
    wave.memory.write(OUTPUT, struct.pack("<32Q", *[lane << 32 | 7 for lane in range(32)]))
    set_pair(wave, 0, OUTPUT)
    rdna2_exec.run(program(
        lane_addresses(),
        vop1("v_mov_b32", 4, LITERAL), 0xAAAA_AAAA, vop1("v_mov_b32", 5, const(1)),   # valeur écrite
        vop1("v_mov_b32", 6, const(7)), vop1("v_mov_b32", 7, const(3)),              # comparée : 3 << 32 | 7
        *global_("global_atomic_cmpswap_x2", addr=1, data=4, vdst=8, saddr=0, glc=1)), wave)
    after = struct.unpack("<32Q", wave.memory.read(OUTPUT, 256))
    assert after == tuple((1 << 32 | 0xAAAA_AAAA) if lane == 3 else lane << 32 | 7 for lane in range(32))
    assert wave.v[8] == [7] * 32 and wave.v[9] == list(range(32))


def test_global_inc_wraps_at_the_limit_and_returns_the_old_value():
    wave = rdna2_exec.Wave()
    wave.memory.write_words(OUTPUT, [lane % 4 for lane in range(32)])
    set_pair(wave, 0, OUTPUT)
    rdna2_exec.run(program(
        lane_addresses(2), vop1("v_mov_b32", 4, const(2)),
        *global_("global_atomic_inc", addr=1, data=4, vdst=5, saddr=0, glc=1)), wave)
    assert wave.memory.read_words(OUTPUT, 4) == [1, 2, 0, 0]          # 0→1, 1→2, 2→0, 3→0 (3 ≥ 2)
    assert wave.v[5][:4] == [0, 1, 2, 3]


def test_buffer_dec_x2_respects_the_descriptor():
    wave = rdna2_exec.Wave()
    wave.memory.write(BUFFER, struct.pack("<3Q", 0, 5, 1 << 40))
    set_descriptor(wave, 20, BUFFER, records=16)                     # deux valeurs 64 bits accessibles
    low, high = mubuf("buffer_atomic_dec_x2", 1, 4, 20, offen=1)
    rdna2_exec.run(program(
        lane_addresses(), vop1("v_mov_b32", 4, const(9)), vop1("v_mov_b32", 5, const(0)),
        low | GLC, high), wave)
    assert struct.unpack("<3Q", wave.memory.read(BUFFER, 24)) == (9, 4, 1 << 40)   # 0 → DATA, 5 → 4
    assert wave.v[4][:3] == [0, 5, 9] and wave.v[5][:3] == [0, 0, 0]  # hors limites : rien renvoyé


def test_lds_compare_store_uses_the_ds_operand_order():
    wave = rdna2_exec.run(program(
        vop1("v_mov_b32", 1, const(0)), vop1("v_mov_b32", 2, const(5)),
        *ds("ds_write_b32", addr=1, data0=2),
        vop1("v_mov_b32", 3, const(9)),
        *ds("ds_cmpst_rtn_b32", addr=1, data0=2, data1=3, vdst=4),     # 5 == 5 : écrit 9
        *ds("ds_cmpst_rtn_b32", addr=1, data0=2, data1=2, vdst=5)))    # 9 != 5 : inchangé
    assert int.from_bytes(wave.lds[:4], "little") == 9
    assert (wave.v[4][0], wave.v[5][0]) == (5, 9)


def test_lds_64_bit_add_carries_into_the_high_word():
    wave = rdna2_exec.run(program(
        vop1("v_mov_b32", 1, const(0)),
        vop1("v_mov_b32", 2, LITERAL), 0xFFFF_FFFF, vop1("v_mov_b32", 3, const(0)),
        *ds("ds_write_b64", addr=1, data0=2),
        vop1("v_mov_b32", 4, const(1)), vop1("v_mov_b32", 5, const(0)),
        *ds("ds_add_rtn_u64", addr=1, data0=4, vdst=6)))               # 32 lignes ajoutent 1
    assert int.from_bytes(wave.lds[:8], "little") == 0xFFFF_FFFF + 32
    assert (wave.v[6][31], wave.v[7][31]) == (0x1E, 1)                # avant la dernière : 2^32 + 30


def test_lds_exchange2_st64_swaps_two_elements_64_apart():
    wave = rdna2_exec.run(program(
        vop1("v_mov_b32", 1, const(0)),
        vop1("v_mov_b32", 2, const(11)), vop1("v_mov_b32", 3, const(22)),
        *ds("ds_write2st64_b32", addr=1, data0=2, data1=3, offset0=0, offset1=1),
        vop1("v_mov_b32", 4, const(33)), vop1("v_mov_b32", 5, const(44)),
        *ds("ds_wrxchg2st64_rtn_b32", addr=1, data0=4, data1=5, vdst=6, offset0=0, offset1=1),
        *ds("ds_read2st64_b32", addr=1, vdst=8, offset0=0, offset1=1)))
    assert int.from_bytes(wave.lds[256:260], "little") == 44           # 1 × 64 éléments × 4 octets
    assert (wave.v[6][0], wave.v[7][0]) == (11, 22)
    assert (wave.v[8][0], wave.v[9][0]) == (33, 44)


def test_recompiler_names_untranslated_atomics():
    with pytest.raises(rdna2_hlsl.Untranslatable, match="global_atomic_cmpswap"):
        rdna2_hlsl.translate(program(*global_("global_atomic_cmpswap", addr=1, data=2, saddr=0)))


def st64_program():
    return with_lane_offset(
        vop2("v_add_nc_u32", 2, const(5), 0),
        *ds("ds_write2st64_b32", addr=31, data0=0, data1=2, offset0=0, offset1=2),
        *ds("ds_read2st64_b32", addr=31, vdst=4, offset0=2, offset1=0),
        *store_lane(4, 0), *store_lane(5, 1))


@gpu
def test_lds_read2st64_and_write2st64_match_on_gpu():
    expected, actual = compare(st64_program())
    assert actual == expected
    lanes = struct.unpack("<32I", expected[OUTPUT:OUTPUT + 128])
    assert lanes == tuple(lane + 5 for lane in range(32))             # l'élément écrit 128 plus loin
