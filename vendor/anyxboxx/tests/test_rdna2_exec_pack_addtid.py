"""s_pack_{ll,lh,hh}_b32_b16 (SOP2) and global_{load,store}_dword_addtid: interpreter and GPU parity.

Semantics: AMD "RDNA 2" ISA (November 2020). Section 12.1, SOP2 opcodes 50-52 (p. 106):
D = {S1[15:0], S0[15:0]}, {S1[31:16], S0[15:0]}, {S1[31:16], S0[31:16]}, SCC unchanged. Section 12.18.3,
GLOBAL opcodes 22-23 (p. 230): "No VGPR address is supplied ... memory_Addr = sgpr_addr(64) +
inst_offset(12) + tid*4". Expected values are written by hand; the GPU tests check the HLSL against the
interpreter byte for byte.
"""
import struct

import pytest

import test_rdna2_hlsl_wave64 as wave64
from emu import rdna2_exec, rdna2_gpu, rdna2_hlsl
from test_rdna2_exec import const, program, sop1, sop2, sopc, vop1
from test_rdna2_exec_memory import INPUT, OUTPUT, global_, set_pair
from test_rdna2_hlsl import compare, store_lane, with_lane_offset

LITERAL, MINUS_ONE, EXEC_LO = 255, 193, 126
S0, S1 = 0x1234_5678, 0x9ABC_DEF0
OLD = 0xDEAD_BEEF
gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc or anyxboxx_gpu_run.exe missing")


def pack_program():
    """s10-s15: packs (s16: SCC after the packs under SCC = 1, s17: under SCC = 0); s18-s20: inline floats
    on 16-bit sources, read as their float32 patterns."""
    return [sop1("s_mov_b32", 4, LITERAL), S0, sop1("s_mov_b32", 5, LITERAL), S1,
            sopc("s_cmp_eq_u32", 4, 4),                                  # SCC = 1
            sop2("s_pack_ll_b32_b16", 10, 4, 5),
            sop2("s_pack_lh_b32_b16", 11, 4, 5),
            sop2("s_pack_hh_b32_b16", 12, 4, 5),
            sop2("s_cselect_b32", 16, const(1), const(2)),
            sopc("s_cmp_lg_u32", 4, 4),                                  # SCC = 0
            sop2("s_pack_hh_b32_b16", 13, 5, 4),
            sop2("s_pack_ll_b32_b16", 14, MINUS_ONE, const(5)),
            sop2("s_pack_lh_b32_b16", 15, LITERAL, 5), 0xAAAA_BBBB,
            sop2("s_cselect_b32", 17, const(1), const(2)),
            sop2("s_pack_ll_b32_b16", 18, 240, 4),                       # 0.5 = 0x3F000000: low half 0
            sop2("s_pack_ll_b32_b16", 19, 4, 248),                       # 1/(2*PI) = 0x3E22F983
            sop2("s_pack_lh_b32_b16", 20, 242, 5)]                       # 1.0 = 0x3F800000


def test_packs_select_the_halves_and_keep_scc():
    wave = rdna2_exec.run(program(*pack_program()))
    assert wave.s[10:16] == [0xDEF0_5678, 0x9ABC_5678, 0x9ABC_1234, 0x1234_9ABC, 0x0005_FFFF, 0x9ABC_BBBB]
    assert wave.s[16:18] == [1, 2]
    assert wave.s[18:21] == [0x5678_0000, 0xF983_5678, 0x9ABC_0000]


def test_pack_hlsl_reads_both_halves():
    hlsl = rdna2_hlsl.translate(program(*pack_program()))
    assert "s[10] = (s[5] << 16) | (s[4] & 0xFFFFu);" in hlsl
    assert "s[12] = (s[5] & 0xFFFF0000u) | (s[4] >> 16);" in hlsl


@gpu
def test_packs_match_interpreter_on_gpu():
    stores = [word for slot, sgpr in enumerate(range(10, 21)) for word in (vop1("v_mov_b32", 3, sgpr),
                                                                            *store_lane(3, slot))]
    expected, actual = compare(with_lane_offset(*pack_program(), *stores))
    assert actual == expected
    assert struct.unpack_from("<I", expected, OUTPUT)[0] == 0xDEF0_5678


# --- global_load/store_dword_addtid ---------------------------------------------------------------------

def input_words(count=64):
    return [0x0101_0101 * (index % 251) ^ index << 20 for index in range(count + 4)]


def addtid_wave(words, exec_mask=0xFFFF_FFFF, lanes=32):
    wave = rdna2_exec.Wave(lanes=lanes)
    wave.memory.write_words(INPUT, input_words())
    set_pair(wave, 0, INPUT)
    set_pair(wave, 2, OUTPUT)
    wave.v[1] = [OLD] * lanes
    wave.write_mask(EXEC_LO, exec_mask)
    return rdna2_exec.run(program(*words), wave)


def test_load_adds_lane_times_four_to_the_scalar_base():
    exec_mask = 0xF0F0_FF0F
    wave = addtid_wave(global_("global_load_dword_addtid", addr=0, vdst=1, saddr=0, offset=8), exec_mask)
    words = input_words()
    assert wave.v[1] == [words[lane + 2] if exec_mask >> lane & 1 else OLD for lane in range(32)]


def test_store_writes_active_lanes_only_and_ignores_the_vgpr_address():
    exec_mask = 0x0FF0_F00F
    wave = addtid_wave([vop1("v_mov_b32", 1, 256 + 0),                  # v1 = lane
                        vop1("v_mov_b32", 0, LITERAL), 0x7FFF_0000,     # v0: an address field that is not used
                        *global_("global_store_dword_addtid", addr=0, data=1, saddr=2, offset=0xFFC)],  # -4
                       exec_mask)
    stored = wave.memory.read_words(OUTPUT - 4, 32)
    assert stored == [lane if exec_mask >> lane & 1 else 0 for lane in range(32)]


def test_wave64_lanes_reach_byte_252():
    wave = addtid_wave(global_("global_load_dword_addtid", addr=0, vdst=1, saddr=0), lanes=64,
                       exec_mask=(1 << 64) - 1)
    assert wave.v[1] == input_words()[:64]


def test_scalar_base_off_is_refused_by_name():
    code = program(*global_("global_load_dword_addtid", addr=0, vdst=1))
    with pytest.raises(rdna2_exec.Untranslated, match="(?=.*global_load_dword_addtid)(?=.*SADDR off)"):
        rdna2_exec.run(code)
    with pytest.raises(rdna2_hlsl.Untranslatable, match="(?=.*global_load_dword_addtid)(?=.*SADDR off)"):
        rdna2_hlsl.translate(code)


def addtid_program(lanes):
    exec_lo, exec_hi = [sop1("s_mov_b32", EXEC_LO, LITERAL), 0xF0F0_FF0F], [sop1("s_mov_b32", EXEC_LO + 1, LITERAL), 0x80FF_FF01]
    full = [sop1("s_mov_b32", EXEC_LO, MINUS_ONE), sop1("s_mov_b32", EXEC_LO + 1, MINUS_ONE)]
    partial = exec_lo + (exec_hi if lanes == 64 else [])
    return [vop1("v_mov_b32", 1, LITERAL), OLD, vop1("v_mov_b32", 4, 256 + 0),
            *partial,
            *global_("global_load_dword_addtid", addr=0, vdst=1, saddr=0, offset=4),
            *global_("global_store_dword_addtid", addr=0, data=4, saddr=2, offset=0x7F0),
            *(full if lanes == 64 else full[:1])]


@gpu
def test_addtid_matches_interpreter_on_gpu():
    image = bytearray(INPUT + 4 * 68)
    struct.pack_into("<68I", image, INPUT, *input_words())
    expected, actual = compare(with_lane_offset(*addtid_program(32), *store_lane(1, 0)), memory=bytes(image))
    assert actual == expected
    assert struct.unpack_from("<I", expected, OUTPUT)[0] == input_words()[1]
    assert struct.unpack_from("<2I", expected, OUTPUT + 0x7F0) == (0, 1)           # lanes 0 and 1 active


@gpu
def test_addtid_matches_interpreter_in_wave64_on_gpu():
    image = bytearray(INPUT + 4 * 68)
    struct.pack_into("<68I", image, INPUT, *input_words())
    data = wave64.with_lane_offset(*addtid_program(64), *wave64.store_lane(1, 0))
    expected, actual = wave64.compare(data, memory=bytes(image))
    assert actual == expected
    assert struct.unpack_from("<I", expected, OUTPUT + 63 * 4)[0] == input_words()[64]   # lane 63 active


def test_graphics_shaders_with_addtid_use_real_lane_ids():
    from emu import rdna2_hlsl_graphics

    load = program(*global_("global_load_dword_addtid", addr=0, vdst=1, saddr=0))
    assert rdna2_hlsl_graphics._uses_wave_ops(load)


@pytest.mark.parametrize("name, ssrc0, ssrc1, expected", [
    # Sources AMD's RDNA 2 XML sizes at 16 bits: the float32 pattern's low half, as LLVM (SOP2_32) and the
    # gfx1036 measurements of 16-bit integer VALU sources say (emu/rdna2_exec_bits.py; PortBridge #83).
    ("s_pack_ll_b32_b16", 240, const(4), 0x0004_0000),     # 0.5 = 0x3F000000
    ("s_pack_ll_b32_b16", const(4), 248, 0xF983_0004),     # 1/(2*PI) = 0x3E22F983
    ("s_pack_lh_b32_b16", 247, const(4), 0x0000_0000),     # -4.0 = 0xC0800000
])
def test_inline_float_on_a_16_bit_source_reads_the_float32_pattern(name, ssrc0, ssrc1, expected):
    wave = rdna2_exec.run(program(sop2(name, 10, ssrc0, ssrc1)))
    assert wave.s[10] == expected


def test_inline_float_on_a_32_bit_source_reads_single_precision():
    wave = rdna2_exec.run(program(sop2("s_pack_hh_b32_b16", 10, 242, 244),      # 1.0 = 0x3F800000, 2.0 = 0x40000000
                                  sop2("s_pack_lh_b32_b16", 11, const(7), 246)))  # 4.0 = 0x40800000
    assert wave.s[10:12] == [0x4000_3F80, 0x4080_0007]
