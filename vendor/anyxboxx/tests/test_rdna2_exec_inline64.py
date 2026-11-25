"""Inline float constants (codes 240-248) in 64-bit scalar operands: the f64 value, not the f32 bits.

AMD "RDNA 2" ISA (November 2020), section 5.2 "Scalar ALU Operands" (p. 32): codes 240-248 are "single or
double floats", with 1/(2π) "single: 0x3e22f983, double: 0x3fc45f306dc9c882". LLVM agrees: llvm-mc 20
(gfx1030) assembles `s_mov_b64 s[0:1], 0x3ff0000000000000` to the inline code 242 (1.0), so compiled code
relies on it. 32-bit sources keep the f32 pattern. Expected values are written by hand.
"""
import pytest

from emu import rdna2_exec, rdna2_hlsl
from test_rdna2_exec import const, program, sop1, sop2, sopc

DOUBLES = {240: 0x3FE0_0000_0000_0000, 241: 0xBFE0_0000_0000_0000, 242: 0x3FF0_0000_0000_0000,
           243: 0xBFF0_0000_0000_0000, 244: 0x4000_0000_0000_0000, 245: 0xC000_0000_0000_0000,
           246: 0x4010_0000_0000_0000, 247: 0xC010_0000_0000_0000, 248: 0x3FC4_5F30_6DC9_C882}
LITERAL = 255


def run(*words):
    return rdna2_exec.run(program(*words))


def pair(wave, register):
    return wave.s[register] | wave.s[register + 1] << 32


@pytest.mark.parametrize("code", sorted(DOUBLES))
def test_s_mov_b64_reads_the_double(code):
    assert pair(run(sop1("s_mov_b64", 40, code)), 40) == DOUBLES[code]


def test_32_bit_sources_keep_the_float32_pattern():
    wave = run(sop1("s_mov_b32", 40, 242), sop1("s_mov_b32", 41, 248))
    assert (wave.s[40], wave.s[41]) == (0x3F80_0000, 0x3E22_F983)


def test_logic_compare_and_select_read_the_double():
    wave = run(
        sop2("s_or_b64", 40, const(0), 245),                           # -2.0
        sop1("s_mov_b32", 42, const(0)), sop1("s_mov_b32", 43, LITERAL), 0x3FF0_0000,
        sopc("s_cmp_eq_u64", 42, 242), sop2("s_cselect_b32", 44, const(1), const(0)),
        sop2("s_cselect_b64", 46, 246, const(0)))                      # SCC = 1 : 4.0
    assert pair(wave, 40) == DOUBLES[245]
    assert wave.s[44] == 1
    assert pair(wave, 46) == DOUBLES[246]


def test_hlsl_still_refuses_an_inline_float_in_a_64_bit_scalar_operand():
    """The HLSL recompiler does not translate these yet: it must refuse, not take the f32 bits."""
    with pytest.raises(rdna2_hlsl.Untranslatable):
        rdna2_hlsl.translate(program(sop1("s_mov_b64", 40, 242)))
