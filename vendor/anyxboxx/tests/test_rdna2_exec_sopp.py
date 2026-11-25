"""SOPP sans effet architectural dans le modèle : s_icache_inv, s_incperflevel, s_decperflevel,
s_ttracedata_imm, s_wait_idle, s_waitcnt_depctr. Les SOPP qui changent l'état ou l'exécution
(s_code_end, s_round_mode, s_denorm_mode, s_sendmsg, s_trap) restent nommées.
"""
import pytest

from emu import rdna2_exec, rdna2_gpu, rdna2_hlsl
from test_rdna2_exec import const, program, sopp, vop2
from test_rdna2_hlsl import compare, store_lane, with_lane_offset

gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc ou anyxboxx_gpu_run.exe absent")
NO_EFFECT = [("s_icache_inv", 0), ("s_incperflevel", 3), ("s_decperflevel", 3), ("s_ttracedata_imm", 0x42),
             ("s_wait_idle", 0), ("s_waitcnt_depctr", 0xFFE3)]


def sopp_program():
    words = []
    for index, (name, simm16) in enumerate(NO_EFFECT):
        words += [vop2("v_add_nc_u32", 1, const(index + 1), 1), sopp(name, simm16)]
    return with_lane_offset(*words, *store_lane(1, 0))


def test_no_effect_sopp_only_advance():
    wave = rdna2_exec.run(sopp_program())
    assert wave.v[1] == [21] * 32                                  # 1 + 2 + … + 6
    assert wave.steps == 1 + 2 * len(NO_EFFECT) + 1                # lshlrev, paires add + SOPP, store


@pytest.mark.parametrize("name", ["s_code_end", "s_round_mode", "s_denorm_mode", "s_sendmsg", "s_trap"])
def test_state_changing_sopp_stay_named(name):
    with pytest.raises(rdna2_exec.Untranslated, match=name):
        rdna2_exec.run(program(sopp(name)))
    with pytest.raises(rdna2_hlsl.Untranslatable, match=name):
        rdna2_hlsl.translate(program(sopp(name)))


@gpu
def test_no_effect_sopp_match_interpreter_on_gpu():
    expected, actual = compare(sopp_program())
    assert actual == expected
