"""Pixel shaders multi-lignes (issue #68) : ballot (`v_cmp_*`) et `v_readlane_b32` /
`v_readfirstlane_b32` sur plusieurs pixels d'un même wave32, dans l'interpréteur
(`rdna2_exec_graphics.run_pixel_wave`) et dans le recompilateur HLSL SM 6.0
(`rdna2_hlsl_graphics.translate_pixel`).
"""
import struct

import pytest

from emu import rdna2_exec_graphics, rdna2_gpu, rdna2_graphics as graphics, rdna2_hlsl_graphics
from emu.rdna2_exec_ops import float_bits
from test_rdna2_exec import ONE, V, const, program, sop1, vop1, vop2, vop3, vopc
from test_rdna2_exec_memory import INPUT
from test_rdna2_graphics import HEIGHT, PIXEL_SHADER, SGPRS, VERTEX_SHADER, WIDTH, exp, vintrp, words

VCC_LO, EXEC_LO = 106, 126
TARGET_MRT1 = graphics.TARGET_MRT0 + 1
MINUS_ONE = 193
gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc ou anyxboxx_gpu_run.exe absent")


def _ps_ballot_and_readlane():
    """Pixel shader de test : interpole attr0.x en v2, compare v2 < 0.5 (ballot → vcc),
    lit la valeur de la ligne 2 (`v_readlane_b32`) et la première ligne active (`v_readfirstlane_b32`),
    et exporte (v2, ballot, lane2, first_active) sur mrt0."""
    return program(
        *vintrp("v_interp_p1_f32", 2, 0, 0, 0),
        *vintrp("v_interp_p2_f32", 2, 1, 0, 0),
        vopc("v_cmp_gt_f32", 0xF0, 2),                      # vcc_lo = (0.5 > v2) → bits des pixels < 0.5
        vop1("v_mov_b32", 3, VCC_LO),                       # v3 = masque ballot
        *vop3("v_readlane_b32", 4, V + 2, const(2)),        # s4 = v2[lane 2]
        vop1("v_mov_b32", 4, 4),                            # v4 = s4
        sop1("s_and_saveexec_b32", 6, VCC_LO),              # restreint EXEC aux pixels où v2 < 0.5
        vop1("v_readfirstlane_b32", 5, V + 2),              # s5 = v2 de la 1re ligne active
        sop1("s_mov_b32", EXEC_LO, 6),                      # restaure EXEC
        vop1("v_mov_b32", 5, 5),                            # v5 = s5
        *exp(graphics.TARGET_MRT0, (2, 3, 4, 5), done=True),
    )


def _ps_lane_dependent_wave():
    """Lane-dependent multi-lane pixel shader for GPU-vs-interpreter verification:
    - interpolates per-pixel attr0.x into v2;
    - records the hardware lane index (`v_mbcnt_lo_u32_b32`), initial EXEC (`s[126]`),
      and wave-identifying first-active-lane barycentrics (`v_readfirstlane_b32`) on mrt1;
    - computes ballot `0.5 > v2` into v3, narrows EXEC via `s_and_saveexec_b32`,
      adds 1.0 to v2 and records the narrowed EXEC into v5 on active lanes only;
    - restores EXEC, reads lane 2's updated v2 via `v_readlane_b32(..., 2)` into v4;
    - exports `(v2, ballot, lane2_v2, narrowed_exec)` on mrt0."""
    return program(
        *vintrp("v_interp_p1_f32", 2, 0, 0, 0),
        *vintrp("v_interp_p2_f32", 2, 1, 0, 0),
        *vop3("v_mbcnt_lo_u32_b32", 7, MINUS_ONE, const(0)),   # v7 = lane index (0..31)
        vop1("v_mov_b32", 8, EXEC_LO),                         # v8 = initial EXEC mask
        vop1("v_readfirstlane_b32", 8, V + 0),                 # s8 = first active lane bary.x
        vop1("v_readfirstlane_b32", 9, V + 1),                 # s9 = first active lane bary.y
        vop1("v_mov_b32", 9, 8),                               # v9 = s8
        vop1("v_mov_b32", 10, 9),                              # v10 = s9
        vop1("v_mov_b32", 5, const(0)),                        # v5 = 0 for inactive lanes
        vopc("v_cmp_gt_f32", 0xF0, 2),                         # vcc_lo = (0.5 > v2) & EXEC
        vop1("v_mov_b32", 3, VCC_LO),                          # v3 = ballot result
        sop1("s_and_saveexec_b32", 6, VCC_LO),                 # EXEC = init_exec & ballot
        vop2("v_add_f32", 2, ONE, 2),                          # active lanes: v2 += 1.0
        vop1("v_mov_b32", 5, EXEC_LO),                         # active lanes: v5 = narrowed EXEC
        sop1("s_mov_b32", EXEC_LO, 6),                         # restore EXEC
        *vop3("v_readlane_b32", 4, V + 2, const(2)),           # s4 = v2[lane 2]
        vop1("v_mov_b32", 4, 4),                               # v4 = s4
        *exp(graphics.TARGET_MRT0, (2, 3, 4, 5)),
        *exp(TARGET_MRT1, (7, 8, 9, 10), done=True),
    )


def test_run_pixel_wave_executes_ballot_and_readlane_across_pixels():
    memory = bytearray(graphics.MEMORY_SIZE)
    # P0 = 0.0, P10 = 1.0, P20 = 0.0 → attr0.x = i
    struct.pack_into("<3f", memory, graphics.parameter_address(0, 0, 0), 0.0, 1.0, 0.0)
    # 4 pixels : i = 0.75, 0.25, 0.875, 0.125 → lanes 1 et 3 ont v2 < 0.5 (ballot = 0b1010 = 10)
    pixels = [(float_bits(x), 0) for x in (0.75, 0.25, 0.875, 0.125)]
    exports = rdna2_exec_graphics.run_pixel_wave(_ps_ballot_and_readlane(), pixels, 0, {}, bytes(memory))
    mrt0 = exports[graphics.TARGET_MRT0]
    assert set(mrt0) == {0, 1, 2, 3}
    for lane, (i_bits, _) in enumerate(pixels):
        assert mrt0[lane] == [i_bits, 0b1010, float_bits(0.875), float_bits(0.25)]


def test_lane_dependent_shader_interpreter_and_hlsl():
    memory = bytearray(graphics.MEMORY_SIZE)
    struct.pack_into("<3f", memory, graphics.parameter_address(0, 0, 0), 0.0, 1.0, 0.0)
    pixels = [(float_bits(x), float_bits(0.1 * idx)) for idx, x in enumerate((0.75, 0.25, 0.375, 0.875))]
    shader = _ps_lane_dependent_wave()
    exports = rdna2_exec_graphics.run_pixel_wave(shader, pixels, 0, {}, bytes(memory))
    mrt0, mrt1 = exports[graphics.TARGET_MRT0], exports[TARGET_MRT1]
    # Lanes 1 and 2 have v2 < 0.5 -> ballot = 0b0110 = 6; lane 2's v2 becomes 1.375.
    for lane in range(4):
        expected_v2 = float_bits((0.75, 1.25, 1.375, 0.875)[lane])
        expected_narrow = 0b0110 if lane in (1, 2) else 0
        assert mrt0[lane] == [expected_v2, 0b0110, float_bits(1.375), expected_narrow]
        assert mrt1[lane] == [lane, 0b1111, pixels[0][0], pixels[0][1]]

    hlsl = rdna2_hlsl_graphics.translate_pixel(shader, WIDTH)
    assert "WaveGetLaneIndex()" in hlsl
    assert "WaveActiveBallot(bit && !IsHelperLane())" in hlsl
    assert "WaveGetLaneIndex() >> 5" in hlsl
    assert "WaveGetLaneIndex() & ~31u" in hlsl
    assert "WaveReadLaneAt(value, base + (other & 31u))" in hlsl


def test_translate_pixel_emits_sm6_wave_intrinsics_for_ballot_and_readlane():
    hlsl = rdna2_hlsl_graphics.translate_pixel(_ps_ballot_and_readlane(), WIDTH)
    assert "WaveGetLaneIndex()" in hlsl
    assert "WaveActiveBallot(bit && !IsHelperLane())" in hlsl
    assert "WaveReadLaneAt(value, base + (other & 31u))" in hlsl
    assert "s[126] = WaveBallot32(true);" in hlsl
    assert "if (!IsHelperLane()) g_memory.Store4(" in hlsl


def test_translate_pixel_keeps_single_lane_for_simple_shader():
    hlsl = rdna2_hlsl_graphics.translate_pixel(PIXEL_SHADER, WIDTH)
    assert "uint lane = 0u;" in hlsl
    assert "WaveActiveBallot" not in hlsl
    assert "IsHelperLane()" not in hlsl


def test_translate_pixel_allows_explicit_wave_lanes_override():
    hlsl = rdna2_hlsl_graphics.translate_pixel(PIXEL_SHADER, WIDTH, multi_lane=True)
    assert "WaveGetLaneIndex()" in hlsl
    assert "WaveActiveBallot(bit && !IsHelperLane())" in hlsl


def test_run_pixel_wave_supports_per_lane_primitives():
    """NVIDIA drivers can pack pixels from multiple primitives into a single warp;
    `run_pixel_wave` interpolates each lane using its own primitive's parameters."""
    memory = bytearray(graphics.MEMORY_SIZE)
    # prim 0: attr0.x = 0.75 (always >= 0.5); prim 1: attr0.x = 0.25 (always < 0.5)
    struct.pack_into("<3f", memory, graphics.parameter_address(0, 0, 0), 0.75, 0.0, 0.0)
    struct.pack_into("<3f", memory, graphics.parameter_address(1, 0, 0), 0.25, 0.0, 0.0)
    pixels = {lane: (float_bits(0.1 * lane), 0) for lane in range(8)}
    primitives = {lane: 0 if lane < 4 else 1 for lane in range(8)}
    exports = rdna2_exec_graphics.run_pixel_wave(
        _ps_lane_dependent_wave(), pixels, primitives, {}, bytes(memory), exec_mask=0xFF
    )
    mrt0 = exports[graphics.TARGET_MRT0]
    for lane in range(8):
        expected_v2 = float_bits(0.75 if lane < 4 else 1.25)
        expected_narrow = 0xF0 if lane >= 4 else 0
        assert mrt0[lane] == [expected_v2, 0xF0, float_bits(0.75), expected_narrow]


@gpu
@pytest.mark.parametrize("geometry, verts", [
    ("fullscreen", [(-1.0, -1.0, 0.0), (3.0, -1.0, 2.5), (-1.0, 3.0, 1.0)]),
    ("diagonal_split", [
        (-1.0, -1.0, 0.0), (1.0, -1.0, 1.25), (1.0, 1.0, 0.8),
        (-1.0, -1.0, 0.0), (1.0, 1.0, 0.8), (-1.0, 1.0, 0.2),
    ]),
])
def test_multi_lane_pixel_shader_lane_dependent_matches_interpreter(geometry, verts):
    """Covers both a single oversized triangle (full quads) and two triangles sharing an internal
    diagonal (partial 2x2 quads exercising D3D helper lanes excluded via SM 6.6 `IsHelperLane()`).
    Reconstructs the exact GPU waves from mrt1 and compares mrt0 byte-for-byte against
    `run_pixel_wave`."""
    memory = bytearray(graphics.MEMORY_SIZE)
    vdata = b"".join(struct.pack("<8f", x, y, 0.5, 1.0, r, 0.0, 0.0, 1.0) for x, y, r in verts)
    memory[INPUT:INPUT + len(vdata)] = vdata
    vertex_count = len(verts)
    primitive_count = vertex_count // 3

    vertex_hlsl = rdna2_hlsl_graphics.translate_vertex(VERTEX_SHADER, SGPRS)
    after_vs = rdna2_gpu.draw(vertex_hlsl, None, bytes(memory), vertex_count, WIDTH, HEIGHT)
    memory = bytearray(after_vs)
    records = memory[graphics.VERTEX_RECORDS:graphics.VERTEX_RECORDS + vertex_count * graphics.VERTEX_STRIDE]
    area = graphics.parameters(bytes(records), primitive_count)
    memory[graphics.PARAMETERS:graphics.PARAMETERS + len(area)] = area

    shader = _ps_lane_dependent_wave()
    pixel_hlsl = rdna2_hlsl_graphics.translate_pixel(shader, WIDTH)
    after = rdna2_gpu.draw(vertex_hlsl, pixel_hlsl, bytes(memory), vertex_count, WIDTH, HEIGHT)

    # Reconstruct exact GPU waves keyed by (first_active_i, first_active_j, init_exec).
    # A single GPU warp (e.g. on NVIDIA) may contain pixels from multiple primitives, so
    # primitive is tracked per lane rather than splitting the wave key.
    waves = {}
    covered = 0
    for pixel in range(WIDTH * HEIGHT):
        record = graphics.PIXEL_RECORDS + pixel * graphics.PIXEL_STRIDE
        flag, i_bits, j_bits, primitive = words(after, record, 4)
        assert flag == 1 and 0 <= primitive < primitive_count
        covered += 1
        mrt0 = words(after, record + 16, 4)
        gpu_lane, init_exec, first_i, first_j = words(after, record + 32, 4)
        key = (first_i, first_j, init_exec)
        waves.setdefault(key, {})[gpu_lane] = (pixel, i_bits, j_bits, primitive, mrt0)
    assert covered == WIDTH * HEIGHT

    saw_divergence = False
    lane2_values = set()
    for (_, _, init_exec), lane_map in waves.items():
        wave_pixels = {lane: (i_bits, j_bits) for lane, (_, i_bits, j_bits, _, _) in lane_map.items()}
        wave_primitives = {lane: prim for lane, (_, _, _, prim, _) in lane_map.items()}
        expected = rdna2_exec_graphics.run_pixel_wave(
            shader, wave_pixels, wave_primitives, {}, bytes(memory), exec_mask=init_exec
        )[graphics.TARGET_MRT0]
        for lane, (pixel, _, _, prim, got_mrt0) in lane_map.items():
            assert got_mrt0 == expected[lane], f"{geometry} prim {prim} pixel {pixel} lane {lane}"
            _, ballot, lane2_val, _ = got_mrt0
            if 0 < ballot < init_exec:
                saw_divergence = True
            lane2_values.add(lane2_val)
    assert saw_divergence, f"{geometry}: expected at least one wave with divergent EXEC ballot"
    assert len(lane2_values) > 1 or len(waves) == 1
