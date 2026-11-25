"""Échanges entre lignes : ds_swizzle_b32 (QUAD_PERM, BITMASK_PERM), ds_permute_b32, ds_bpermute_b32.

Valeurs attendues calculées à la main d'après le pseudo-code d'AMD (DS_SWIZZLE_B32, DS_PERMUTE_B32,
DS_BPERMUTE_B32) ; le test GPU vérifie le HLSL contre l'interpréteur.
"""
import pytest

from emu import rdna2_exec, rdna2_gpu, rdna2_hlsl
from test_rdna2_exec import const, program, sop1, vop1, vop2
from test_rdna2_exec_memory import ds
from test_rdna2_hlsl import compare, store_lane, with_lane_offset

LITERAL, EXEC_LO = 255, 126
gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc ou anyxboxx_gpu_run.exe absent")


def swizzle(offset, vdst=1, data0=0):
    return ds("ds_swizzle_b32", addr=0, data0=data0, vdst=vdst, offset0=offset & 0xFF, offset1=offset >> 8)


@pytest.mark.parametrize("offset, expected", [
    (0x801B, [4 * (i // 4) + 3 - i % 4 for i in range(32)]),     # QUAD_PERM 3,2,1,0 : 0b00_01_10_11
    (0x401F, [i ^ 16 for i in range(32)]),                         # SWAP 16 : and 0x1F, xor 16
    (0x7C1F, [31 - i for i in range(32)]),                         # REVERSE 32 : and 0x1F, xor 31
    (0x0078, [(i & 0x18) | 3 for i in range(32)]),                 # BROADCAST 8, 3 : and 0x18, or 3
    (0x0000, [0] * 32),                                            # and 0 : tout le monde lit la ligne 0
])
def test_swizzle_modes(offset, expected):
    assert rdna2_exec.run(program(*swizzle(offset))).v[1] == expected


def test_swizzle_inactive_source_gives_zero():
    wave = rdna2_exec.run(program(
        vop1("v_mov_b32", 1, LITERAL), 0x77,
        sop1("s_mov_b32", EXEC_LO, LITERAL), 0xFFFF_0000,             # lignes 16 à 31
        *swizzle(0x401F)))                                             # SWAP 16 : lit les lignes 0 à 15
    assert wave.v[1] == [0x77] * 16 + [0] * 16


@pytest.mark.parametrize("offset", [0xC000, 0xE01F, 0x8100])
def test_rotate_fft_and_reserved_offsets_are_named(offset):
    with pytest.raises(rdna2_exec.Untranslated, match="ds_swizzle_b32"):
        rdna2_exec.run(program(*swizzle(offset)))
    with pytest.raises(rdna2_hlsl.Untranslatable, match="ds_swizzle_b32"):
        rdna2_hlsl.translate(program(*swizzle(offset)))


def test_permute_forwards_and_highest_lane_wins():
    wave = rdna2_exec.run(program(
        vop2("v_mul_u32_u24", 2, const(12), 0),                       # 3 × 4 × ligne : cible 3i % 32
        *ds("ds_permute_b32", addr=2, data0=0, vdst=3),
        vop2("v_lshrrev_b32", 4, const(1), 0), vop2("v_lshlrev_b32", 4, const(2), 4),   # cible i // 2
        *ds("ds_permute_b32", addr=4, data0=0, vdst=5),
        vop2("v_lshlrev_b32", 6, const(2), 0),
        *ds("ds_permute_b32", addr=6, data0=0, vdst=7, offset0=8)))   # cible i + 2
    assert wave.v[3] == [d * 11 % 32 for d in range(32)]               # 3 × 11 = 33 ≡ 1 (mod 32)
    assert wave.v[5] == [2 * d + 1 for d in range(16)] + [0] * 16      # 2d et 2d + 1 visent d : 2d + 1
    assert wave.v[7] == [(d - 2) % 32 for d in range(32)]


def test_permute_ignores_inactive_sources():
    wave = rdna2_exec.run(program(
        vop2("v_lshlrev_b32", 2, const(2), 0),
        sop1("s_mov_b32", EXEC_LO, LITERAL), 0x0000_FFFF,
        *ds("ds_permute_b32", addr=2, data0=0, vdst=3, offset0=64)))  # cible i + 16 : hors des actives
    assert wave.v[3] == [0] * 32


def test_bpermute_reads_zero_from_inactive_lanes():
    wave = rdna2_exec.run(program(
        vop2("v_add_nc_u32", 2, const(16), 0), vop2("v_lshlrev_b32", 2, const(2), 2),   # (i + 16) × 4
        sop1("s_mov_b32", EXEC_LO, LITERAL), 0x0000_FFFF,
        *ds("ds_bpermute_b32", addr=2, data0=0, vdst=3)))              # lit les lignes 16 à 31, inactives
    assert wave.v[3] == [0] * 32


def lanes_program():
    ops = [
        vop2("v_mul_u32_u24", 1, LITERAL, 0), 0x0001_0203,            # valeurs propres à chaque ligne
        vop2("v_xor_b32", 1, LITERAL, 1), 0x5A00_00A5,
        vop2("v_mul_u32_u24", 2, const(20), 0),                        # cible 5i % 32 (× 4)
        vop2("v_lshrrev_b32", 3, const(2), 0), vop2("v_lshlrev_b32", 3, const(2), 3),   # cible i // 4
        *swizzle(0x80E4, vdst=4, data0=1), *swizzle(0x80B1, vdst=5, data0=1),
        *swizzle(0x041F, vdst=6, data0=1), *swizzle(0x0078, vdst=7, data0=1),
        *swizzle(0x3C0F, vdst=8, data0=1), *swizzle(0x7FFF, vdst=9, data0=1),
        *ds("ds_permute_b32", addr=2, data0=1, vdst=10),
        *ds("ds_permute_b32", addr=3, data0=1, vdst=11, offset0=4),
        *ds("ds_bpermute_b32", addr=2, data0=1, vdst=12, offset0=12),
        sop1("s_mov_b32", EXEC_LO, LITERAL), 0x5A5A_C3C3,              # lignes actives mêlées
        *swizzle(0x401F, vdst=13, data0=1), *swizzle(0x801B, vdst=14, data0=1),
        *ds("ds_permute_b32", addr=3, data0=1, vdst=15),
        *ds("ds_bpermute_b32", addr=2, data0=1, vdst=16),
        sop1("s_mov_b32", EXEC_LO, LITERAL), 0xFFFF_FFFF,
    ]
    stores = []
    for slot, register in enumerate(range(4, 17)):
        stores += store_lane(register, slot)
    return with_lane_offset(*ops, *stores)


def test_hlsl_translates_lane_exchanges():
    hlsl = rdna2_hlsl.translate(lanes_program())
    assert "ForwardLane(lane," in hlsl and "((lane & ~3u) + ((0xE4u" in hlsl


@gpu
def test_lane_exchanges_match_interpreter_on_gpu():
    expected, actual = compare(lanes_program())
    assert actual == expected
