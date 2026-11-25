"""ds_write_addtid_b32 and ds_read_addtid_b32: interpreter, HLSL text and GPU parity.

AMD "RDNA 2" ISA (November 2020), section 12.13, DS opcodes 176-177 (p. 204): "LDS_GS[LDS_BASE +
{OFFSET1,OFFSET0} + M0[15:0] + TID*4].u32 = VGPR[DATA0].u32" and the matching read into VDST. No VGPR
address is read; M0[31:16] is ignored. Expected values are written by hand; the GPU tests check the HLSL
against the interpreter byte for byte.
"""
import struct

import pytest

import test_rdna2_hlsl_wave64 as wave64
from emu import rdna2_exec, rdna2_gpu, rdna2_hlsl
from test_rdna2_exec import program, sop1, vop1, vop2
from test_rdna2_exec_memory import ds
from test_rdna2_hlsl import compare, store_lane, with_lane_offset

LITERAL, MINUS_ONE, M0, EXEC_LO = 255, 193, 124, 126
OLD = 0xDEAD_BEEF
M0_VALUE = 0xABCD_0020          # M0[15:0] = 0x20; the high half must be ignored
gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc or anyxboxx_gpu_run.exe missing")


def prelude():
    return [sop1("s_mov_b32", M0, LITERAL), M0_VALUE,
            vop2("v_mul_u32_u24", 1, LITERAL, 0), 0x0001_0001,           # v1 = lane × 0x10001
            vop1("v_mov_b32", 2, LITERAL), OLD,
            vop1("v_mov_b32", 0, LITERAL), 0x7FFF_0000]                  # an address VGPR that is not read


def run(words, exec_mask, lanes=32):
    wave = rdna2_exec.Wave(lanes=lanes)
    wave.write_mask(EXEC_LO, exec_mask)
    return rdna2_exec.run(program(*words), wave)


def test_write_uses_m0_low_half_offset_and_lane_id():
    exec_mask = 0x0FF0_F00F
    wave = run([*prelude(), *ds("ds_write_addtid_b32", 0, data0=1, offset0=4, offset1=1)], exec_mask)
    base = 0x20 + 0x104
    words = struct.unpack_from("<32I", wave.lds, base)
    assert list(words) == [lane * 0x10001 if exec_mask >> lane & 1 else 0 for lane in range(32)]
    assert wave.lds[:base] == bytes(base)


def test_read_fills_active_lanes_only():
    exec_mask = 0xF00F_0FF0
    words = [*prelude(), *ds("ds_write_addtid_b32", 0, data0=1),
             sop1("s_mov_b32", EXEC_LO, LITERAL), exec_mask,
             *ds("ds_read_addtid_b32", 0, vdst=2, offset0=4)]             # lane i reads lane i + 1's word
    wave = run(words, 0xFFFF_FFFF)
    assert wave.v[2] == [(lane + 1) * 0x10001 if exec_mask >> lane & 1 and lane < 31 else
                         (0 if exec_mask >> lane & 1 else OLD) for lane in range(32)]


def test_wave64_lane_63_reaches_byte_252():
    wave = run([*prelude(), *ds("ds_write_addtid_b32", 0, data0=1)], (1 << 64) - 1, lanes=64)
    assert struct.unpack_from("<I", wave.lds, 0x20 + 252)[0] == 63 * 0x10001


def test_hlsl_reads_m0_low_half_and_lane():
    hlsl = rdna2_hlsl.translate(program(*ds("ds_read_addtid_b32", 0, vdst=2, offset0=8)))
    assert "g_lds[(((s[124] & 0xFFFFu) + 8u + lane * 4u) >> 2)" in hlsl


def test_graphics_shaders_with_ds_addtid_use_real_lane_ids():
    from emu import rdna2_hlsl_graphics

    assert rdna2_hlsl_graphics._uses_wave_ops(program(*ds("ds_read_addtid_b32", 0, vdst=2)))


def addtid_program(lanes):
    partial = [sop1("s_mov_b32", EXEC_LO, LITERAL), 0xF0F0_FF0F]
    partial += [sop1("s_mov_b32", EXEC_LO + 1, LITERAL), 0x80FF_FF01] if lanes == 64 else []
    full = [sop1("s_mov_b32", EXEC_LO, MINUS_ONE)]
    full += [sop1("s_mov_b32", EXEC_LO + 1, MINUS_ONE)] if lanes == 64 else []
    return [*prelude(), *partial,
            *ds("ds_write_addtid_b32", 0, data0=1, offset0=0x10),
            *full,
            *ds("ds_read_addtid_b32", 0, vdst=2, offset0=0x14)]             # neighbour lane, through LDS


@gpu
def test_ds_addtid_matches_interpreter_on_gpu():
    expected, actual = compare(with_lane_offset(*addtid_program(32), *store_lane(2, 0)))
    assert actual == expected


@gpu
def test_ds_addtid_matches_interpreter_in_wave64_on_gpu():
    data = wave64.with_lane_offset(*addtid_program(64), *wave64.store_lane(2, 0))
    expected, actual = wave64.compare(data)
    assert actual == expected
