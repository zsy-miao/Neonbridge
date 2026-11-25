"""Écritures scalaires en mémoire : s_store_*, s_buffer_store_*, s_atomic_*, s_buffer_atomic_*.

Valeurs attendues calculées à la main (sémantique des atomiques vectoriels, spécification XML CDNA 1
pour ces opcodes) ; le test GPU vérifie le HLSL contre l'interpréteur.
"""
import struct

import pytest

from emu import rdna2_exec, rdna2_gpu, rdna2_hlsl
from test_rdna2_exec import program, sop1, vop1
from test_rdna2_exec_memory import BUFFER, INPUT, OUTPUT, TABLE, set_descriptor, set_pair, smem
from test_rdna2_hlsl import compare, store_lane, with_lane_offset

LITERAL, NULL = 255, 125
GLC = 1 << 16
gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc ou anyxboxx_gpu_run.exe absent")


def smem_glc(name, sdata, sbase, offset=0, soffset=NULL):
    low, high = smem(name, sdata, sbase, offset, soffset)
    return low | GLC, high


def smov(sdst, value):
    return sop1("s_mov_b32", sdst, LITERAL), value & 0xFFFF_FFFF


def run(words, table=None):
    wave = rdna2_exec.Wave()
    wave.memory.write_words(TABLE, table or [0] * 16)
    set_pair(wave, 4, TABLE)
    set_descriptor(wave, 12, TABLE, records=16)                      # buffer de 16 octets
    rdna2_exec.run(program(*words), wave)
    return wave


def test_scalar_stores_and_buffer_bounds():
    wave = run([
        *smov(8, 0x1111), *smov(9, 0x2222), *smov(10, 0x3333), *smov(11, 0x4444), *smov(30, 32),
        *smem("s_store_dword", 8, 4),                                  # TABLE + 0
        *smem("s_store_dwordx2", 9, 4, offset=8),                      # TABLE + 8, + 12
        *smem("s_store_dwordx4", 8, 4, offset=-16, soffset=30),        # TABLE + 32 - 16 = + 16
        *smem("s_buffer_store_dword", 11, 12, offset=4),               # dans le buffer
        *smem("s_buffer_store_dword", 11, 12, offset=16),              # 16 + 4 > 16 : rien
        *smem("s_buffer_store_dwordx2", 10, 12, offset=12)])           # 12 + 8 > 16 : rien
    assert wave.memory.read_words(TABLE, 9) == [0x1111, 0x4444, 0x2222, 0x3333,
                                                0x1111, 0x2222, 0x3333, 0x4444, 0]


def test_scalar_atomics_return_the_old_value_with_glc():
    wave = run([
        *smov(8, 3), *smem_glc("s_atomic_add", 8, 4),                  # 10 + 3
        *smov(9, 5), *smem("s_atomic_sub", 9, 4, offset=4),            # 20 - 5, sans glc
        *smov(10, -1), *smem_glc("s_atomic_smin", 10, 4, offset=8),    # min(-16, -1) = -16
        *smov(11, 1), *smem_glc("s_atomic_smax", 11, 4, offset=12),    # max(-16, 1) = 1
        *smov(14, 99), *smov(15, 7), *smem_glc("s_atomic_cmpswap", 14, 4, offset=16),   # 7 = 7 : 99
        *smov(16, 98), *smov(17, 8), *smem_glc("s_atomic_cmpswap", 16, 4, offset=20)],  # 6 ≠ 8 : 6 reste
        [10, 20, 0xFFFF_FFF0, 0xFFFF_FFF0, 7, 6])
    assert wave.memory.read_words(TABLE, 6) == [13, 15, 0xFFFF_FFF0, 1, 99, 6]
    assert wave.s[8:12] == [10, 5, 0xFFFF_FFF0, 0xFFFF_FFF0]       # s9 garde sa donnée (pas de glc)
    assert wave.s[14:18] == [7, 7, 6, 8]


def test_scalar_inc_dec_and_buffer_bounds():
    wave = run([
        *smov(8, 4), *smem_glc("s_buffer_atomic_inc", 8, 12),          # 4 ≥ 4 : 0
        *smov(9, 4), *smem_glc("s_buffer_atomic_inc", 9, 12, offset=4),  # 2 < 4 : 3
        *smov(10, 9), *smem_glc("s_buffer_atomic_dec", 10, 12, offset=8),  # 0 : repart à 9
        *smov(11, 9), *smem_glc("s_buffer_atomic_dec", 11, 12, offset=12),  # 5 : 4
        *smov(16, 1), *smem_glc("s_buffer_atomic_add", 16, 12, offset=16)],  # hors du buffer : rien
        [4, 2, 0, 5, 50])
    assert wave.memory.read_words(TABLE, 5) == [0, 3, 9, 4, 50]
    assert wave.s[8:12] == [4, 2, 0, 5] and wave.s[16] == 1


def test_64_bit_scalar_atomics():
    wave = run([
        *smov(8, 1), *smov(9, 0), *smem_glc("s_atomic_add_x2", 8, 4),             # 0x1_FFFFFFFF + 1
        *smov(10, 0x10), *smov(11, 0x20), *smov(12, 3), *smov(13, 4),             # donnée, puis valeur comparée
        *smem_glc("s_atomic_cmpswap_x2", 10, 4, offset=8),                         # 4:3 = 4:3 : 20:10
        *smov(18, 0), *smov(19, 5), *smem("s_atomic_umax_x2", 18, 4, offset=16),  # 5:0 > 4:FFFFFFFF
        *smov(20, 7), *smov(21, 7), *smem_glc("s_atomic_dec_x2", 20, 4, offset=24)],  # 0 : repart à 7:7
        [0xFFFF_FFFF, 1, 3, 4, 0xFFFF_FFFF, 4, 0, 0])
    assert wave.memory.read_words(TABLE, 8) == [0, 2, 0x10, 0x20, 0, 5, 7, 7]
    assert wave.s[8:12] == [0xFFFF_FFFF, 1, 3, 4] and wave.s[20:22] == [0, 0]


def test_scratch_store_is_still_named():
    with pytest.raises(rdna2_exec.Untranslated, match="s_scratch_store_dword"):
        rdna2_exec.run(program(*smem("s_scratch_store_dword", 8, 4)))


def test_recompiler_names_64_bit_scalar_atomics():
    with pytest.raises(rdna2_hlsl.Untranslatable, match="s_atomic_add_x2"):
        rdna2_hlsl.translate(program(*smem("s_atomic_add_x2", 8, 0)))


ATOMICS = [("s_atomic_add", 3, True), ("s_atomic_sub", 5, True), ("s_atomic_smin", -1, True),
           ("s_atomic_smax", 7, True), ("s_atomic_umin", 2, False), ("s_atomic_or", 0x9000_0000, True),
           ("s_atomic_and", 0xFF0, True), ("s_atomic_or", 0x0F, True), ("s_atomic_xor", 0xFFFF, True),
           ("s_atomic_swap", 0x1234, True)]


def smem_program():
    ops = []
    for index, (name, value, glc) in enumerate(ATOMICS):               # s[8:17], INPUT + 4 × i
        ops += [*smov(8 + index, value), *(smem_glc if glc else smem)(name, 8 + index, 0, offset=4 * index)]
    ops += [
        *smov(18, 99), *smov(19, 77), *smem_glc("s_atomic_cmpswap", 18, 0, offset=40),     # égal : 99
        *smov(24, 98), *smov(25, 1), *smem_glc("s_atomic_cmpswap", 24, 0, offset=44),      # différent
        *smov(26, 4), *smem_glc("s_buffer_atomic_inc", 26, 20),                             # BUFFER + 0
        *smov(27, 9), *smem_glc("s_buffer_atomic_dec", 27, 20, offset=4),
        *smov(28, 6), *smem_glc("s_buffer_atomic_umax", 28, 20, offset=8),
        *smov(29, 1), *smem_glc("s_buffer_atomic_add", 29, 20, offset=16),                  # hors du buffer
        *smov(30, 0x1000), *smem("s_store_dwordx4", 8, 2, soffset=30),                     # OUTPUT + 0x1000
        *smem("s_store_dwordx2", 18, 2, offset=0x1010), *smem("s_store_dword", 24, 2, offset=0x1018),
        *smem("s_buffer_store_dword", 26, 20, offset=12),
        *smem("s_buffer_store_dwordx2", 27, 20, offset=12),                                 # 12 + 8 > 16
        *smem("s_load_dwordx4", 40, 0), *smem("s_load_dwordx2", 44, 0, offset=40),        # relus après
    ]
    stores = []
    for slot, register in enumerate([*range(8, 20), *range(24, 30), *range(40, 46)]):
        stores += [vop1("v_mov_b32", 1, register), *store_lane(1, slot)]
    return with_lane_offset(*ops, *stores)


def smem_memory():
    image = bytearray(BUFFER + 16)
    image[INPUT:INPUT + 48] = struct.pack("<12I", 10, 20, 0xFFFF_FFF0, 3, 9, 9, 0xF0F0, 0xF0F0, 0xF0F0, 1,
                                          77, 5)
    image[BUFFER:BUFFER + 16] = struct.pack("<4I", 4, 0, 50, 60)
    return bytes(image)


def test_hlsl_translates_scalar_writes():
    hlsl = rdna2_hlsl.translate(smem_program())
    for token in ("InterlockedCompareExchange(", "g_memory.InterlockedMin(a, asint(", "ReadLane(lane, old, 0u)"):
        assert token in hlsl


@gpu
def test_scalar_writes_match_interpreter_on_gpu():
    expected, actual = compare(smem_program(), memory=smem_memory())
    assert actual == expected
    assert struct.unpack_from("<4I", actual, OUTPUT + 0x1000) == (10, 20, 0xFFFF_FFF0, 3)   # anciennes valeurs
