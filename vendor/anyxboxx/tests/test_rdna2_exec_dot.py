"""Produits scalaires entiers VOP3P (v_dot2_i32_i16, v_dot2_u32_u16, v_dot4_i32_i8, v_dot4_u32_u8,
v_dot8_i32_i4, v_dot8_u32_u4) : interpréteur de référence, traduction HLSL et parité GPU (wave32, wave64).

Valeurs attendues calculées à la main (arithmétique en commentaire) d'après les descriptions de la
spécification XML d'AMD (RDNA 2) et le pseudo-code RDNA 3 d'AMD de v_dot4_u32_u8 et v_dot8_u32_u4.
"""
import pytest

from emu import rdna2_exec, rdna2_gpu, rdna2_hlsl
from test_rdna2_exec import V, const, program, vop1, vop2
from test_rdna2_exec_memory import OUTPUT
from test_rdna2_hlsl import compare, store_lane, with_lane_offset
import test_rdna2_hlsl_wave64 as wave64

VOP3P_PREFIX = 0x33 << 26
VOP3P_OPCODES = {"v_dot2_i32_i16": 0x14, "v_dot2_u32_u16": 0x15, "v_dot4_i32_i8": 0x16, "v_dot4_u32_u8": 0x17,
                 "v_dot8_i32_i4": 0x18, "v_dot8_u32_u4": 0x19, "v_dot2_f32_f16": 0x13, "v_pk_add_u16": 0x0A}
LITERAL, MINUS_ONE = 255, 193
gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc ou anyxboxx_gpu_run.exe absent")


def vop3p(name, vdst, src0, src1, src2=128, op_sel=0, op_sel_hi=0b111, neg_lo=0, neg_hi=0, clamp=0):
    """Deux mots VOP3P ; op_sel_hi vaut 1 partout par défaut, comme l'encodage de LLVM."""
    low = (VOP3P_PREFIX | VOP3P_OPCODES[name] << 16 | clamp << 15 | (op_sel_hi >> 2 & 1) << 14 | op_sel << 11
           | neg_hi << 8 | vdst)
    high = neg_lo << 29 | (op_sel_hi & 0b11) << 27 | src2 << 18 | src1 << 9 | src0
    return low, high


def mov(vdst, value):
    return vop1("v_mov_b32", vdst, LITERAL), value & 0xFFFF_FFFF


def run(*words):
    return rdna2_exec.run(program(*words))


@pytest.mark.parametrize("name, a, b, c, expected", [
    ("v_dot4_u32_u8", 0x0102_0304, 0x0506_0708, 10, 80),          # 4×8 + 3×7 + 2×6 + 1×5 = 70, + 10
    ("v_dot4_u32_u8", 0xFFFF_FFFF, 0xFFFF_FFFF, 0xFFFF_FFFF, 0x3F803),   # 4 × 255² = 260100, - 1 (mod 2^32)
    # octets de a : 127, -128, 2, -1 ; de b : 2, 127, -1, 1 → 254 - 16256 - 2 - 1 = -16005, + 5 = -16000
    ("v_dot4_i32_i8", 0xFF02_807F, 0x01FF_7F02, 5, 0xFFFF_C180),
    # a : 32767, -32768 ; b : 2, 32767 → 65534 - 1073709056 = -1073643522, - 1 = -1073643523
    ("v_dot2_i32_i16", 0x8000_7FFF, 0x7FFF_0002, 0xFFFF_FFFF, 0xC001_7FFD),
    ("v_dot2_u32_u16", 0xFFFF_0003, 0x0002_0004, 0x10, 0x2001A),  # 3×4 + 65535×2 = 131082, + 16 = 131098
    ("v_dot8_u32_u4", 0x1234_5678, 0x1111_1111, 0, 36),          # 8 + 7 + … + 1
    ("v_dot8_u32_u4", 0xFFFF_FFFF, 0xFFFF_FFFF, 0, 1800),        # 8 × 15²
    # quartets de a (bas → haut) : 1, 2, 3, 4, 5, 6, 7, -8 ; de b : 0, -1, 0, -1, 0, -1, 0, -1
    # → -2 - 4 - 6 + 8 = -4, + 3 = -1
    ("v_dot8_i32_i4", 0x8765_4321, 0xF0F0_F0F0, 3, 0xFFFF_FFFF),
])
def test_dot_products(name, a, b, c, expected):
    wave = run(*mov(1, a), *mov(2, b), *mov(3, c), *vop3p(name, 4, V + 1, V + 2, V + 3))
    assert wave.v[4] == [expected] * 32


def test_dot_product_reads_a_literal_and_inline_constants():
    wave = run(*mov(1, 0x0102_0304), *vop3p("v_dot4_u32_u8", 2, V + 1, LITERAL, MINUS_ONE), 0x0101_0101)
    assert wave.v[2][0] == 9                                   # 4 + 3 + 2 + 1 = 10, - 1


@pytest.mark.parametrize("modifiers, named", [
    ({"clamp": 1}, "clamp"), ({"op_sel": 1}, "op_sel"), ({"op_sel_hi": 0b011}, "op_sel_hi"),
    ({"neg_lo": 2}, "neg_lo"), ({"neg_hi": 4}, "neg_hi"),
])
def test_undocumented_modifiers_are_refused_by_name(modifiers, named):
    code = program(*vop3p("v_dot4_i32_i8", 2, V + 0, V + 0, **modifiers))
    with pytest.raises(rdna2_exec.Untranslated, match=f"v_dot4_i32_i8 avec {named}"):
        rdna2_exec.run(code)
    with pytest.raises(rdna2_hlsl.Untranslatable, match=f"v_dot4_i32_i8 avec {named}"):
        rdna2_hlsl.translate(code)


def test_f16_and_packed_forms_stay_named():
    for name in ("v_dot2_f32_f16", "v_pk_add_u16"):
        assert not rdna2_exec.executable("VOP3P", name)
        with pytest.raises(rdna2_exec.Untranslated, match=name):
            run(*vop3p(name, 2, V + 0, V + 0))
        with pytest.raises(rdna2_hlsl.Untranslatable, match=name):
            rdna2_hlsl.translate(program(*vop3p(name, 2, V + 0, V + 0)))


def dot_program(lane_offset, store):
    """Sources propres à chaque ligne (octets et quartets signés des deux signes), un résultat par opération."""
    ops = [
        vop2("v_mul_u32_u24", 1, LITERAL, 0), 0x0103_0507,
        vop2("v_xor_b32", 1, LITERAL, V + 1), 0x80F0_7F11,
        vop2("v_mul_u32_u24", 2, LITERAL, 0), 0x00F1_E3D5,
        vop2("v_xor_b32", 2, LITERAL, V + 2), 0x7F81_08F7,
        vop2("v_mul_u32_u24", 3, LITERAL, 0), 0x0081_2345,
        vop2("v_sub_nc_u32", 3, LITERAL, V + 3), 0x0400_0000,
    ]
    names = ("v_dot2_i32_i16", "v_dot2_u32_u16", "v_dot4_i32_i8", "v_dot4_u32_u8", "v_dot8_i32_i4", "v_dot8_u32_u4")
    for index, name in enumerate(names):
        ops += vop3p(name, 10 + index, V + 1, V + 2, V + 3)
    ops += [*vop3p("v_dot8_i32_i4", 16, LITERAL, V + 1, MINUS_ONE), 0x9F3C_A5E1,
            *vop3p("v_dot2_i32_i16", 17, V + 2, const(7), V + 0)]
    stores = [word for slot, register in enumerate(range(10, 18)) for word in store(register, slot)]
    return lane_offset(*ops, *stores)


def test_hlsl_uses_the_packed_dot_helper():
    hlsl = rdna2_hlsl.translate(dot_program(with_lane_offset, store_lane))
    assert "uint DotPacked(" in hlsl and hlsl.count("DotPacked(") == 9    # définition + 8 appels
    assert "DotPacked(v[1], v[2], v[3], 4u, true)" in hlsl


@gpu
def test_dot_products_match_interpreter_on_gpu():
    expected, actual = compare(dot_program(with_lane_offset, store_lane))
    assert actual == expected


@gpu
def test_dot_products_match_interpreter_on_gpu_in_wave64():
    expected, actual = wave64.compare(dot_program(wave64.with_lane_offset, wave64.store_lane))
    assert actual == expected


def test_parity_programs_write_lane_varying_results():
    """Garde-fou du test GPU : les résultats diffèrent d'une ligne à l'autre (sinon la parité prouverait peu)."""
    wave = rdna2_exec.Wave()
    wave.s[2] = OUTPUT
    rdna2_exec.run(dot_program(with_lane_offset, store_lane), wave)
    assert all(len(set(wave.v[register])) > 16 for register in range(10, 18))
