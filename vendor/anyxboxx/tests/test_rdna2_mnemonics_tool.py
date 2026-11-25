"""tools/rdna2_mnemonics.py : lignes d'encodage sans « encoding: » et seconde source, les définitions gfx10
des fichiers .td de LLVM (VOP3PInstructions.td, VOP3Instructions.td ; Apache 2.0 avec exception LLVM)."""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
import rdna2_mnemonics  # noqa: E402

TD_EXCERPT = """
multiclass VOP3P_Real_gfx10<bits<8> op> {
defm V_PK_MAD_I16     : VOP3P_Real_gfx10_gfx11_gfx12_gfx13<0x00>;
defm V_PK_ADD_F16     : VOP3P_Real_gfx10_gfx11_gfx12_gfx13<0x0f>;
defm V_DOT2_I32_I16 : VOP3P_Real_gfx10 <0x14>;
defm V_FMA_MIX_F32    : VOP3P_Real_gfx10_gfx11_gfx12_gfx13_Triple<0x20>;
defm V_PK_ADD_BF16    : VOP3P_Real_gfx12<0x23>;
"""

# Extrait de VOP3Instructions.td : les lignes sans gfx10 (gfx11+, gfx6/gfx7 seuls) ne comptent pas,
# le nom d'assembleur de « _with_name » l'emporte sur celui du defm, le suffixe _e64 tombe.
TD_VOP3_EXCERPT = """
defm V_DIV_SCALE_F64       : VOP3be_Real_gfx11_gfx12_not_gfx1250_gfx13<0x2fd, "V_DIV_SCALE_F64", "v_div_scale_f64">;
defm V_ALIGNBIT_B32_opsel  : VOP3OpSel_Real_gfx10_with_name<0x14e, "V_ALIGNBIT_B32_opsel", "v_alignbit_b32">;
  defm V_MUL_LO_I32      : VOP3_Real_gfx10<0x16b>;
defm V_INTERP_P1_F32_e64  : VOP3Interp_Real_gfx10<0x200>;
defm V_DIV_SCALE_F64   : VOP3be_Real_gfx6_gfx7_gfx10<0x16e>;
defm V_MIN_LEGACY_F32  : VOP3_Real_gfx6_gfx7<0x12d>;
"""

# Extrait de gfx10_asm_dpp16.s : l'encodage suit la ligne source, sans « encoding: ».
BARE_EXCERPT = """// RUN: not llvm-mc -triple=amdgpu10.10 -mattr=+wavefrontsize32 -show-encoding %s
v_sat_pk_u8_i16_dpp v5, v1 quad_perm:[3,2,1,0] row_mask:0x0 bank_mask:0x0
// GFX10: [0xfa,0xc4,0x0a,0x7e,0x01,0x1b,0x00,0x00]

v_mov_b32_e64 v5, v1 quad_perm:[3,2,1,0]
// W32-ERR: :[[@LINE-1]]:22: error: not a valid operand. [0x01]
"""
SAT_PK_DPP = bytes((0xFA, 0xC4, 0x0A, 0x7E, 0x01, 0x1B, 0x00, 0x00))


def test_td_lines_give_gfx10_vop3p_opcodes(tmp_path):
    (tmp_path / "VOP3PInstructions.td").write_text(TD_EXCERPT, encoding="utf-8")
    assert rdna2_mnemonics.td_mnemonics(tmp_path) == {
        ("VOP3P", 0x00): "v_pk_mad_i16", ("VOP3P", 0x0F): "v_pk_add_f16",
        ("VOP3P", 0x14): "v_dot2_i32_i16", ("VOP3P", 0x20): "v_fma_mix_f32"}


def test_td_lines_give_gfx10_vop3_opcodes(tmp_path):
    (tmp_path / "VOP3Instructions.td").write_text(TD_VOP3_EXCERPT, encoding="utf-8")
    assert rdna2_mnemonics.td_mnemonics(tmp_path) == {
        ("VOP3", 0x14E): "v_alignbit_b32", ("VOP3", 0x16B): "v_mul_lo_i32",
        ("VOP3", 0x200): "v_interp_p1_f32", ("VOP3", 0x16E): "v_div_scale_f64"}


def test_bare_encodings_name_their_source_line(tmp_path):
    (tmp_path / "gfx10_asm_dpp16.s").write_text(BARE_EXCERPT, encoding="utf-8")
    assert list(rdna2_mnemonics.read_encodings(tmp_path)) == [("gfx10_asm_dpp16.s", "v_sat_pk_u8_i16", SAT_PK_DPP)]
    votes, preferred, wrong = rdna2_mnemonics.collect(tmp_path)
    assert dict(votes) == {("VOP1", 0x62): {"v_sat_pk_u8_i16": 1}} and not preferred and not wrong


def test_bare_encodings_stay_out_of_operand_text_comparisons(tmp_path):
    """Leur texte est celui de la source, pas celui de LLVM : rdna2_shapes ne doit pas le lire."""
    (tmp_path / "gfx10_asm_dpp16.s").write_text(BARE_EXCERPT, encoding="utf-8")
    assert list(rdna2_mnemonics.llvm_lines(tmp_path)) == []


def test_encodings_win_over_td_and_disagreements_are_reported():
    table = {("VOP3P", 0x0F): "v_pk_add_f16"}
    td = {("VOP3P", 0x0F): "v_pk_add_f16", ("VOP3P", 0x10): "v_pk_mul_f16", ("VOP3P", 0x00): "autre"}
    table[("VOP3P", 0x00)] = "v_pk_mad_i16"
    disagreements = rdna2_mnemonics.merge_td(table, td)
    assert table[("VOP3P", 0x10)] == "v_pk_mul_f16" and table[("VOP3P", 0x00)] == "v_pk_mad_i16"
    assert disagreements == [(("VOP3P", 0x00), "v_pk_mad_i16", "autre")]
