"""Instructions VOP3 ajoutées : v_mul_lo_i32, v_sad_u8, v_sad_hi_u8, v_lerp_u8, v_cube*,
v_mad_u64_u32, v_mad_i64_i32, et formes VOP3 de v_readfirstlane_b32 et v_nop.

Valeurs attendues calculées à la main d'après la documentation publique d'AMD (« RDNA 2
Instruction Set Architecture ») ; v_cube* suit aussi le repli constant de LLVM (AMDGPU).
"""
import pytest

from emu import rdna2_exec, rdna2_gpu, rdna2_hlsl
from emu.rdna2_exec_ops import bits_float, float_bits
from test_rdna2_exec import EXEC_LO, V, const, program, sop1, vop1, vop2, vop3
from test_rdna2_hlsl import compare, store_lane, with_lane_offset

LITERAL, NULL, MINUS_ONE = 255, 125, 193
gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc ou anyxboxx_gpu_run.exe absent")


def run(*words):
    return rdna2_exec.run(program(*words))


def mov(vdst, value):
    return vop1("v_mov_b32", vdst, LITERAL), value & 0xFFFF_FFFF


def movf(vdst, value):
    return mov(vdst, float_bits(value))


def test_integer_and_byte_operations():
    wave = run(
        *mov(1, -3), *mov(2, 0x0102_0304), *mov(3, 0x0403_0201),
        *vop3("v_mul_lo_i32", 4, V + 1, const(5)),
        *vop3("v_sad_u8", 5, V + 2, V + 3, const(10)),
        *vop3("v_sad_hi_u8", 6, V + 2, V + 3, const(10)),
        *mov(7, 0x00FF_0102), *mov(8, 0x00FF_0304), *mov(9, 0x0000_0101),
        *vop3("v_lerp_u8", 10, V + 7, V + 8, V + 9))
    assert wave.v[4][0] == 0xFFFF_FFF1                          # -15
    assert wave.v[5][0] == 18                                    # 3 + 1 + 1 + 3 + 10
    assert wave.v[6][0] == 8 << 16 | 10
    assert wave.v[10][0] == 0x00FF_0203                          # (2+4+1)>>1, (1+3+1)>>1, (255+255)>>1, 0


@pytest.mark.parametrize("x, y, z, face, sc, tc, ma", [
    (0.5, -2.0, 1.0, 3.0, 0.5, -1.0, -4.0),                     # axe y négatif
    (-3.0, 1.0, 2.0, 1.0, 2.0, -1.0, -6.0),                     # axe x négatif
    (1.0, 1.0, 1.0, 4.0, 1.0, -1.0, 2.0),                       # égalité : z l'emporte
    (0.25, 0.5, -0.75, 5.0, -0.25, -0.5, -1.5),                 # axe z négatif
])
def test_cube_map_coordinates(x, y, z, face, sc, tc, ma):
    wave = run(
        *movf(1, x), *movf(2, y), *movf(3, z),
        *(word for i, name in enumerate(("v_cubeid_f32", "v_cubesc_f32", "v_cubetc_f32", "v_cubema_f32"))
          for word in vop3(name, 4 + i, V + 1, V + 2, V + 3)))
    assert [bits_float(wave.v[4 + i][0]) for i in range(4)] == [face, sc, tc, ma]


def test_mad_u64_u32_carries_out_of_64_bits():
    wave = run(
        *mov(1, 0xFFFF_FFFF), *mov(4, 0xFFFF_FFFF), *mov(5, 0xFFFF_FFFF),
        *vop3("v_mad_u64_u32", 6, V + 1, V + 1, V + 4, sdst=40),      # (2^32-1)² + 2^64-1
        *vop3("v_mad_u64_u32", 8, const(2), const(3), const(1), sdst=41),
        *vop3("v_mad_u64_u32", 10, const(2), const(3), MINUS_ONE, sdst=42))   # -1 étendu : 6 + 2^64 - 1
    assert (wave.v[6][0], wave.v[7][0], wave.s[40]) == (0, 0xFFFF_FFFE, 0xFFFF_FFFF)
    assert (wave.v[8][0], wave.v[9][0], wave.s[41]) == (7, 0, 0)
    assert (wave.v[10][0], wave.v[11][0], wave.s[42]) == (5, 0, 0xFFFF_FFFF)


def test_mad_i64_i32_uses_signed_operands():
    wave = run(
        *mov(1, -2), *mov(4, 10), *mov(5, 0),
        *vop3("v_mad_i64_i32", 6, V + 1, const(3), V + 4, sdst=NULL),  # -6 + 10
        *vop3("v_mad_i64_i32", 8, MINUS_ONE, const(1), const(0), sdst=NULL))
    assert (wave.v[6][0], wave.v[7][0]) == (4, 0)
    assert (wave.v[8][0], wave.v[9][0]) == (0xFFFF_FFFF, 0xFFFF_FFFF)


def test_mad_i64_i32_carry_out_is_named():
    with pytest.raises(rdna2_exec.Untranslated, match="v_mad_i64_i32"):
        run(*vop3("v_mad_i64_i32", 6, const(1), const(1), const(0), sdst=40))


def test_vop3_readfirstlane_and_nop():
    wave = run(
        sop1("s_mov_b32", EXEC_LO, LITERAL), 0xFFFF_FFE0,              # lignes 5 à 31
        *vop3("v_readfirstlane_b32", 40, V + 0),
        *vop3("v_nop", 0, 0))
    assert wave.s[40] == 5
    assert wave.v[0] == list(range(32))


def vop3_parity_program():
    ops = [
        vop2("v_mul_u32_u24", 1, LITERAL, 0), 0x0001_0203,             # valeurs propres à chaque ligne
        vop2("v_xor_b32", 2, LITERAL, 1), 0x8040_2010,
        vop1("v_cvt_f32_i32", 3, V + 0), vop2("v_subrev_f32", 3, LITERAL, 3), float_bits(15.5),
        *vop3("v_mul_lo_i32", 4, V + 2, LITERAL), 0xFFFF_FFFD,
        *vop3("v_sad_u8", 5, V + 1, V + 2, V + 0),
        *vop3("v_sad_hi_u8", 6, V + 1, V + 2, V + 0),
        *vop3("v_lerp_u8", 7, V + 1, V + 2, V + 0),
        *vop3("v_cubeid_f32", 8, V + 3, LITERAL, V + 3), float_bits(4.0),
        *vop3("v_cubesc_f32", 9, V + 3, V + 3, LITERAL), float_bits(-8.0),
        *vop3("v_cubetc_f32", 10, LITERAL, V + 3, V + 3), float_bits(3.0),
        *vop3("v_cubema_f32", 11, V + 3, LITERAL, V + 3), float_bits(-2.0),
        vop1("v_mov_b32", 13, LITERAL), 0xFFFF_FFF0,
        *vop3("v_mad_u64_u32", 14, V + 2, V + 1, V + 12, sdst=40),     # S2 = v[12:13]
        *vop3("v_mad_i64_i32", 16, V + 2, V + 1, V + 12, sdst=NULL),
        vop1("v_mov_b32", 18, 40),
        *vop3("v_readfirstlane_b32", 41, V + 2), vop1("v_mov_b32", 19, 41),
    ]
    stores = []
    for slot, register in enumerate(range(4, 20)):
        stores += store_lane(register, slot)
    return with_lane_offset(vop1("v_mov_b32", 12, V + 0), *ops, *stores)


def test_hlsl_translates_the_new_vop3_operations():
    hlsl = rdna2_hlsl.translate(vop3_parity_program())
    for token in ("SadU8(", "LerpU8(", "Cube(", "MulHiI32(x, y)", "MulHiU32(x, y)"):
        assert token in hlsl


@gpu
def test_new_vop3_operations_match_interpreter_on_gpu():
    expected, actual = compare(vop3_parity_program())
    assert actual == expected
