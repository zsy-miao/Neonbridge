"""Operand types of SDWA and DPP16 forms: one definition for the interpreter and the HLSL (#205).

SDWA applies neg/abs to f32 sources and sext to integer ones, and omod/clamp to f32 results; DPP16 applies
neg/abs to f32 sources. The types come from the mnemonic (destination type before source type when both are
named, as in LLVM's VOP profiles): v_cvt_flr_i32_f32 and v_cvt_rpi_i32_f32 read f32 and write i32,
v_cvt_off_f32_i4 reads an integer and writes f32, v_frexp_exp_i32_f32 reads f32 and writes i32.
Expected interpreter values are worked out by hand from the RDNA 2 ISA descriptions of each instruction.
"""
import struct

import pytest

from emu import rdna2_exec, rdna2_exec_ext, rdna2_gpu, rdna2_hlsl, rdna2_hlsl_ext
from emu.rdna2_exec_ops import float_bits
from test_rdna2_exec import program, vop1
from test_rdna2_exec_memory import INPUT, global_
from test_rdna2_hlsl import compare, store_lane, with_lane_offset
from test_rdna2_sdwa_dpp_exec import BYTE_0, BYTE_1, DPP16, SDWA, dpp16_word, run_wave, sdwa_word

gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc ou anyxboxx_gpu_run.exe absent")
SWAP_PAIRS = 0xB1                     # quad_perm:[1,0,3,2]
FLOATS = [2.5, -2.5, 0.5, -0.75, 1.75, -7.25, 3.0, 1.0e10, -0.0, 6.5, -1.5, 0.25, 100.5, -3.5, 1.0e-3, 8.0]
NIBBLES = [0x0000_0700, 0x0000_0F00, 0x0000_0800, 0x0000_0100, 0x0000_0009, 0x1234_5678, 0xFFFF_FFFF, 0]


@pytest.mark.parametrize("name, flags", [
    ("v_cvt_flr_i32_f32", (True, False)),
    ("v_cvt_rpi_i32_f32", (True, False)),
    ("v_cvt_off_f32_i4", (False, True)),
    ("v_frexp_exp_i32_f32", (True, False)),
    ("v_frexp_mant_f32", (True, True)),
    ("v_cvt_f32_i32", (False, True)),
    ("v_cvt_u32_f32", (True, False)),
    ("v_cvt_f32_ubyte2", (False, True)),
    ("v_cvt_pkrtz_f16_f32", (True, False)),
    ("v_mul_hi_i32_i24", (False, False)),
    ("v_add_f32", (True, True)),
    ("v_add_nc_u32", (False, False)),
])
def test_operand_types_follow_the_mnemonic(name, flags):
    assert rdna2_exec_ext.operand_float_flags(name) == flags


def test_interpreter_and_hlsl_share_one_definition():
    assert rdna2_hlsl_ext.operand_float_flags is rdna2_exec_ext.operand_float_flags


def test_sdwa_neg_abs_reach_the_f32_source_of_flr_and_rpi():
    wave = run_wave([
        vop1("v_cvt_flr_i32_f32", 2, SDWA), sdwa_word(1, src0_neg=1),               # floor(-2.5) = -3
        vop1("v_cvt_rpi_i32_f32", 3, SDWA), sdwa_word(1, src0_neg=1, src0_abs=1),   # floor(-2.5 + 0.5) = -2
        vop1("v_cvt_flr_i32_f32", 4, DPP16), dpp16_word(1, 0, src0_abs=1),          # lane 0 reads lane 0
    ], init_v={1: [float_bits(2.5)] * 32})
    assert (wave.v[2][0], wave.v[3][0], wave.v[4][0]) == (0xFFFF_FFFD, 0xFFFF_FFFE, 2)


def test_sdwa_omod_and_clamp_reach_the_f32_result_of_cvt_off():
    wave = run_wave([
        vop1("v_cvt_off_f32_i4", 2, SDWA), sdwa_word(1, omod=1, src0_sel=BYTE_1, src0_sext=1),  # 7/16 × 2
        vop1("v_cvt_off_f32_i4", 3, SDWA), sdwa_word(1, clamp=1, src0_sel=BYTE_0),             # -1/16 → 0
        vop1("v_cvt_off_f32_i4", 4, SDWA), sdwa_word(1, omod=3, src0_sel=BYTE_0),              # -1/16 × 0.5
    ], init_v={1: [0x0000_F70F] * 32})
    assert (wave.v[2][0], wave.v[3][0], wave.v[4][0]) == (float_bits(0.875), 0, float_bits(-0.03125))


@pytest.mark.parametrize("name, ext", [
    ("v_frexp_exp_i32_f32", sdwa_word(1, clamp=1)),
    ("v_frexp_exp_i32_f32", sdwa_word(1, omod=1)),
    ("v_cvt_flr_i32_f32", sdwa_word(1, omod=2, src0_neg=1)),
    ("v_cvt_rpi_i32_f32", sdwa_word(1, clamp=1)),
])
def test_sdwa_output_modifiers_on_an_integer_result_are_refused_by_name(name, ext):
    code = program(vop1(name, 2, SDWA), ext)
    with pytest.raises(rdna2_exec.Untranslated, match=name):
        rdna2_exec.run(code)
    with pytest.raises(rdna2_hlsl.Untranslatable, match=name):
        rdna2_hlsl.translate(code)


def parity_memory():
    """FLOATS from INPUT, NIBBLES from INPUT + 128, one value per lane."""
    floats = [float_bits(FLOATS[lane % len(FLOATS)]) for lane in range(32)]
    nibbles = [NIBBLES[lane % len(NIBBLES)] ^ (lane << 12) for lane in range(32)]
    image = bytearray(INPUT + 256)
    image[INPUT:] = struct.pack("<64I", *floats, *nibbles)
    return bytes(image)


FORMS = {
    "flr_rpi_neg_abs": [
        ("v_cvt_flr_i32_f32", SDWA, sdwa_word(1, src0_neg=1)),
        ("v_cvt_flr_i32_f32", SDWA, sdwa_word(1, src0_abs=1)),
        ("v_cvt_rpi_i32_f32", SDWA, sdwa_word(1, src0_neg=1)),
        ("v_cvt_rpi_i32_f32", SDWA, sdwa_word(1, src0_neg=1, src0_abs=1)),
        ("v_cvt_flr_i32_f32", DPP16, dpp16_word(1, SWAP_PAIRS, src0_neg=1)),
        ("v_cvt_rpi_i32_f32", DPP16, dpp16_word(1, SWAP_PAIRS, src0_abs=1)),
    ],
    "frexp_exp_neg_abs": [
        ("v_frexp_exp_i32_f32", SDWA, sdwa_word(1, src0_neg=1, src0_abs=1)),
        ("v_frexp_exp_i32_f32", DPP16, dpp16_word(1, SWAP_PAIRS, src0_neg=1)),
    ],
    "cvt_off_omod_sext": [
        ("v_cvt_off_f32_i4", SDWA, sdwa_word(2, omod=1, src0_sel=BYTE_1, src0_sext=1)),
        ("v_cvt_off_f32_i4", SDWA, sdwa_word(2, omod=3, src0_sel=BYTE_0)),
    ],
    "cvt_off_clamp": [
        ("v_cvt_off_f32_i4", SDWA, sdwa_word(2, clamp=1, src0_sel=BYTE_0)),
        ("v_cvt_off_f32_i4", SDWA, sdwa_word(2, clamp=1, omod=1, src0_sel=BYTE_1)),
    ],
}


def parity_program(forms):
    ops = [*global_("global_load_dword", addr=31, vdst=1, saddr=0),
           *global_("global_load_dword", addr=31, vdst=2, saddr=0, offset=128)]
    for slot, (name, kind, ext) in enumerate(forms):
        ops += [vop1(name, 3 + slot, kind), ext]
    stores = [word for slot in range(len(forms)) for word in store_lane(3 + slot, slot)]
    return with_lane_offset(*ops, *stores)


@gpu
@pytest.mark.parametrize("group", FORMS)
def test_sdwa_and_dpp16_type_modifiers_match_interpreter_on_gpu(group):
    expected, actual = compare(parity_program(FORMS[group]), memory=parity_memory())
    assert actual == expected
