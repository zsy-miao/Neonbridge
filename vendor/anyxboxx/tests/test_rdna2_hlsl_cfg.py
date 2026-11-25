"""Contrôle de flux structuré dans emu/rdna2_hlsl.py (issue #68) :
- programmes réductibles (ligne droite, diamants if/else, boucles simples et imbriquées)
  traduits sans `switch (pc)` via third_party/leonida/cfg_structure.py ;
- graphes irréductibles (boucle à deux entrées) repliés proprement sur `switch (pc)` ;
- étages graphiques (`translate_vertex`, `translate_pixel`) sans `break;` hors boucle.
"""
import pytest

from emu import rdna2_gpu, rdna2_hlsl, rdna2_hlsl_graphics
from test_rdna2_exec import V, const, program, sop1, sop2, sopc, sopp, vop1, vop2
from test_rdna2_graphics import PIXEL_SHADER, VERTEX_SHADER
from test_rdna2_hlsl import compare, store_lane, with_lane_offset

gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc ou anyxboxx_gpu_run.exe absent")


def test_straight_line_has_no_switch_or_loop():
    hlsl = rdna2_hlsl.translate(program(vop1("v_mov_b32", 1, const(7))))
    assert "switch (pc)" not in hlsl
    assert "case 0u:" not in hlsl
    assert "while (" not in hlsl
    assert "v[1] = 7u;" in hlsl


def test_reducible_loop_and_if_else_have_no_switch():
    # if (s4 == 0) s5 = 10 else s5 = 20; then loop s6 = 3..1
    code = with_lane_offset(
        sop1("s_mov_b32", 4, const(0)),
        sopc("s_cmp_eq_u32", 4, const(0)),
        sopp("s_cbranch_scc0", 2),
        sop1("s_mov_b32", 5, const(10)),
        sopp("s_branch", 1),
        sop1("s_mov_b32", 5, const(20)),
        sop1("s_mov_b32", 6, const(3)),
        sop2("s_add_u32", 5, 5, 6),
        sop2("s_sub_u32", 6, 6, const(1)),
        sopc("s_cmp_lg_u32", 6, const(0)),
        sopp("s_cbranch_scc1", -4),
        vop1("v_mov_b32", 1, 5),
        *store_lane(1, 0),
    )
    hlsl = rdna2_hlsl.translate(code)
    assert "switch (pc)" not in hlsl
    assert "[loop] while (pc ==" in hlsl
    assert "if (pc ==" in hlsl


def test_nested_loops_emit_nested_structured_loops_without_switch():
    # Outer loop s4 = 2..1, inner loop s5 = 3..1, s6 += 1
    code = with_lane_offset(
        sop1("s_mov_b32", 6, const(0)),
        sop1("s_mov_b32", 4, const(2)),
        sop1("s_mov_b32", 5, const(3)),
        sop2("s_add_u32", 6, 6, const(1)),
        sop2("s_sub_u32", 5, 5, const(1)),
        sopc("s_cmp_lg_u32", 5, const(0)),
        sopp("s_cbranch_scc1", -4),
        sop2("s_sub_u32", 4, 4, const(1)),
        sopc("s_cmp_lg_u32", 4, const(0)),
        sopp("s_cbranch_scc1", -8),
        vop1("v_mov_b32", 1, 6),
        *store_lane(1, 0),
    )
    hlsl = rdna2_hlsl.translate(code)
    assert "switch (pc)" not in hlsl
    assert hlsl.count("[loop] while (pc ==") == 2


def test_irreducible_cfg_falls_back_to_switch():
    # Two-entry loop: entry -> B2 (or B1 if scc0), B1 -> B2, B2 -> B1 (cycle {B1, B2} with 2 entries).
    code = program(
        sopc("s_cmp_eq_u32", 4, const(0)),
        sopp("s_cbranch_scc0", 1),
        sopp("s_branch", 2),
        # B1:
        sop2("s_add_u32", 5, 5, const(1)),
        # B2:
        sop2("s_sub_u32", 4, 4, const(1)),
        sopc("s_cmp_lg_u32", 4, const(0)),
        sopp("s_cbranch_scc1", -4),
    )
    hlsl = rdna2_hlsl.translate(code)
    assert "switch (pc)" in hlsl


def test_graphics_stages_structured_without_switch_or_stray_break():
    vs = rdna2_hlsl_graphics.translate_vertex(VERTEX_SHADER)
    ps = rdna2_hlsl_graphics.translate_pixel(PIXEL_SHADER, 64)
    assert "switch (pc)" not in vs and "break;" not in vs
    assert "switch (pc)" not in ps and "break;" not in ps


@gpu
def test_structured_nested_loops_and_branch_match_interpreter():
    code = with_lane_offset(
        sop1("s_mov_b32", 6, const(0)),
        sop1("s_mov_b32", 4, const(2)),
        sop1("s_mov_b32", 5, const(3)),
        sop2("s_add_u32", 6, 6, const(1)),
        sop2("s_sub_u32", 5, 5, const(1)),
        sopc("s_cmp_lg_u32", 5, const(0)),
        sopp("s_cbranch_scc1", -4),
        sop2("s_sub_u32", 4, 4, const(1)),
        sopc("s_cmp_lg_u32", 4, const(0)),
        sopp("s_cbranch_scc1", -8),
        vop2("v_add_nc_u32", 1, 6, V + 0),
        *store_lane(1, 0),
    )
    expected, actual = compare(code)
    assert actual == expected


@gpu
def test_if_else_diamond_matches_interpreter():
    # if (s4 == 0) s5 = 10 else s5 = 20; v1 = s5 + lane
    code = with_lane_offset(
        sopc("s_cmp_eq_u32", 4, const(0)),
        sopp("s_cbranch_scc0", 2),
        sop1("s_mov_b32", 5, const(10)),
        sopp("s_branch", 1),
        sop1("s_mov_b32", 5, const(20)),
        vop2("v_add_nc_u32", 1, 5, V + 0),
        *store_lane(1, 0),
    )
    assert "switch (pc)" not in rdna2_hlsl.translate(code)
    for branch_value in (0, 1):
        expected, actual = compare(code, sgprs={4: branch_value})
        assert actual == expected


@gpu
def test_irreducible_cfg_fallback_matches_interpreter():
    # Two-entry loop (irreducible CFG -> switch (pc) fallback):
    # entry -> B2 when s4 == 0, else B1; B1 adds 1 to s5 then falls into B2;
    # B2 decrements s4 and loops back to B1 while s4 != 0.
    code = with_lane_offset(
        sopc("s_cmp_eq_u32", 4, const(0)),
        sopp("s_cbranch_scc1", 2),
        sopp("s_branch", 0),
        # B1:
        sop2("s_add_u32", 5, 5, const(1)),
        # B2:
        sopc("s_cmp_eq_u32", 4, const(0)),
        sopp("s_cbranch_scc1", 3),
        sop2("s_sub_u32", 4, 4, const(1)),
        sopc("s_cmp_lg_u32", 4, const(0)),
        sopp("s_cbranch_scc1", -6),
        vop2("v_add_nc_u32", 1, 5, V + 0),
        *store_lane(1, 0),
    )
    assert "switch (pc)" in rdna2_hlsl.translate(code)
    for start_s4 in (0, 3):
        expected, actual = compare(code, sgprs={4: start_s4, 5: 10})
        assert actual == expected
