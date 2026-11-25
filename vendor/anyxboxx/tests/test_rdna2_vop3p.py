"""Texte LLVM des instructions VOP3P et suffixe _e32 des encodages vectoriels 32 bits.

Vecteurs des tests du désassembleur de LLVM (llvm/test/MC/Disassembler/AMDGPU, Apache 2.0 avec
exception LLVM) : gfx10_vop3p_literalv216.txt et gfx1011_dlops.txt tels quels ; gfx9_vop3p.txt et
gfx9_vop3p_opsel.txt réencodés en gfx10. Seuls les bits 31-23 diffèrent (0x1A7 en gfx9, 0x198 en
gfx10, VOP3PInstructions.td) et l'imprimante est la même ; v_mad_mix_f32 de gfx9 s'appelle
v_fma_mix_f32 en gfx10.
"""
import pytest

from emu import rdna2_operands

GFX10_PREFIX = 0xCC


def gfx10(*encoding):
    """Encodage VOP3P gfx9 (préfixe 0xD3, bit 23 à 1) ramené au préfixe gfx10."""
    data = bytearray(encoding)
    data[2] &= 0x7F
    data[3] = GFX10_PREFIX
    return bytes(data)


@pytest.mark.parametrize("encoding, expected", [
    # gfx10_vop3p_literalv216.txt
    ((0x01, 0x40, 0x0F, 0xCC, 0xF1, 0x04, 0x02, 0x1A), "v_pk_add_f16 v1, -0.5, v2"),
    ((0x01, 0x40, 0x0F, 0xCC, 0xF8, 0x04, 0x02, 0x1A), "v_pk_add_f16 v1, 0.15915494, v2"),
    ((0x01, 0x40, 0x0F, 0xCC, 0xC1, 0x04, 0x02, 0x1A), "v_pk_add_f16 v1, -1, v2"),
    ((0x01, 0x40, 0x0F, 0xCC, 0x02, 0x01, 0x01, 0x1A), "v_pk_add_f16 v1, v2, 0"),
    ((0x05, 0x40, 0x0F, 0xCC, 0x01, 0xFF, 0x01, 0x1A, 0x00, 0x01, 0x00, 0x00), "v_pk_add_f16 v5, v1, 0x100"),
    ((0x05, 0x40, 0x0A, 0xCC, 0xFF, 0x04, 0x02, 0x1A, 0x78, 0x56, 0x34, 0x12), "v_pk_add_u16 v5, 0x12345678, v2"),
    ((0x05, 0x40, 0x06, 0xCC, 0x81, 0x20, 0x01, 0x1A), "v_pk_ashrrev_i16 v5, 1, 16"),
    ((0x05, 0x40, 0x0E, 0xCC, 0xC1, 0x84, 0x0D, 0x1B), "v_pk_fma_f16 v5, -1, -2, -3"),
    ((0x05, 0x40, 0x0E, 0xCC, 0x01, 0x05, 0xFE, 0x1B, 0x56, 0x34, 0x12, 0xAF), "v_pk_fma_f16 v5, v1, v2, 0xaf123456"),
    ((0x05, 0x40, 0x00, 0xCC, 0xF2, 0xE8, 0xD9, 0x1B), "v_pk_mad_i16 v5, 1.0, 2.0, 4.0"),
    # gfx1011_dlops.txt
    ((0x00, 0x40, 0x13, 0xCC, 0x01, 0x05, 0x0E, 0x1C), "v_dot2_f32_f16 v0, v1, v2, v3"),
    ((0x00, 0x40, 0x16, 0xCC, 0x01, 0x05, 0x0E, 0x1C), "v_dot4_i32_i8 v0, v1, v2, v3"),
    ((0x00, 0x40, 0x19, 0xCC, 0x01, 0x05, 0x0E, 0x1C), "v_dot8_u32_u4 v0, v1, v2, v3"),
])
def test_gfx10_vop3p_follows_llvm(encoding, expected):
    assert rdna2_operands.text(bytes(encoding)) == expected


@pytest.mark.parametrize("encoding, expected", [                 # gfx9_vop3p.txt, gfx9_vop3p_opsel.txt
    ((0x05, 0xC0, 0x8F, 0xD3, 0x01, 0x05, 0x02, 0x18), "v_pk_add_f16 v5, v1, v2 clamp"),
    ((0x05, 0x42, 0x8F, 0xD3, 0x01, 0x05, 0x02, 0x18), "v_pk_add_f16 v5, v1, v2 neg_hi:[0,1]"),
    ((0x05, 0x40, 0x8F, 0xD3, 0x01, 0x05, 0x02, 0x38), "v_pk_add_f16 v5, v1, v2 neg_lo:[1,0]"),
    ((0x05, 0x58, 0x8F, 0xD3, 0x01, 0x05, 0x02, 0x18), "v_pk_add_f16 v5, v1, v2 op_sel:[1,1]"),
    ((0x05, 0x40, 0x8F, 0xD3, 0x01, 0x05, 0x02, 0x00), "v_pk_add_f16 v5, v1, v2 op_sel_hi:[0,0]"),
    ((0x05, 0x40, 0x8F, 0xD3, 0x01, 0x05, 0x02, 0x10), "v_pk_add_f16 v5, v1, v2 op_sel_hi:[0,1]"),
    ((0x05, 0x40, 0x8A, 0xD3, 0xF7, 0x04, 0x02, 0x18), "v_pk_add_u16 v5, -4.0, v2"),
    ((0x05, 0x00, 0xA0, 0xD3, 0x01, 0x05, 0x0E, 0x04), "v_fma_mix_f32 v5, v1, v2, v3"),
    ((0x05, 0x00, 0xA0, 0xD3, 0x01, 0x05, 0x0E, 0xE4), "v_fma_mix_f32 v5, -v1, -v2, -v3"),
    ((0x05, 0x04, 0xA0, 0xD3, 0x01, 0x05, 0x0E, 0x04), "v_fma_mix_f32 v5, v1, v2, |v3|"),
    ((0x05, 0x20, 0xA0, 0xD3, 0x01, 0x05, 0x0E, 0x04), "v_fma_mix_f32 v5, v1, v2, v3 op_sel:[0,0,1]"),
    ((0x05, 0x00, 0xA0, 0xD3, 0x01, 0x05, 0x0E, 0x0C), "v_fma_mix_f32 v5, v1, v2, v3 op_sel_hi:[1,0,0]"),
    ((0x05, 0x40, 0xA0, 0xD3, 0x01, 0x05, 0x0E, 0x1C), "v_fma_mix_f32 v5, v1, v2, v3 op_sel_hi:[1,1,1]"),
    ((0x05, 0x80, 0xA0, 0xD3, 0x01, 0x05, 0x0E, 0x04), "v_fma_mix_f32 v5, v1, v2, v3 clamp"),
])
def test_vop3p_modifiers_follow_llvm(encoding, expected):
    assert rdna2_operands.text(gfx10(*encoding)) == expected


def test_two_source_packed_ops_ignore_the_third_op_sel_hi():
    """Bit 14 (op_sel_hi de src2) sans troisième source : LLVM ne l'écrit pas."""
    assert rdna2_operands.text(bytes.fromhex("04000acc03030118")) == "v_pk_add_u16 v4, v3, 1"


def test_vop3p_literal_belongs_to_the_instruction():
    """Le littéral fait partie de l'instruction : le décodage reste aligné sur s_endpgm."""
    from emu import rdna2

    shader = bytes.fromhex("05400acc" "ff04021a" "78563412" "000081bf")   # v_pk_add_u16 + littéral, s_endpgm
    assert [(i.family, i.size) for i in rdna2.decode(shader)] == [("VOP3P", 3), ("SOPP", 1)]


def test_truncated_vop3p_literal_has_no_text():
    assert rdna2_operands.text(bytes((0x05, 0x40, 0x0A, 0xCC, 0xFF, 0x04, 0x02, 0x1A))) is None


@pytest.mark.parametrize("encoding, expected", [                 # gfx10_vop1.txt, gfx10_vop2.txt, gfx10_vopc.txt
    ((0xF0, 0x02, 0x0A, 0x7E), "v_mov_b32_e32 v5, 0.5"),
    ((0x01, 0x41, 0xFE, 0x7F), "v_fract_f32_e32 v255, v1"),
    ((0x01, 0x05, 0x0A, 0x30), "v_ashrrev_i32_e32 v5, v1, v2"),
    ((0x01, 0x05, 0x04, 0x7C), "v_cmp_eq_f32_e32 vcc_lo, v1, v2"),
    ((0x01, 0x07, 0x0A, 0x58, 0x41, 0x31, 0x21, 0x11), "v_fmamk_f32 v5, v1, 0x11213141, v3"),   # sans suffixe
    ((0x00, 0x00, 0x00, 0x7E), "v_nop"),                                                        # sans suffixe
    ((0x01, 0x05, 0xCE, 0x7E), "v_readfirstlane_b32 s103, v1"),                                 # sans suffixe
])
def test_32_bit_vector_encodings_carry_the_e32_suffix(encoding, expected):
    assert rdna2_operands.text(bytes(encoding)) == expected


def test_vintrp_carries_the_e32_suffix():                           # gfx9_vintrp.txt (même nom en gfx10)
    assert rdna2_operands.text(bytes.fromhex("000108c8")) == "v_interp_p1_f32_e32 v2, v0, attr0.y"

