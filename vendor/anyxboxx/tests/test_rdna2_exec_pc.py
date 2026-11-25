"""Compteur de programme : s_getpc_b64 (interpréteur, HLSL, parité GPU), s_setpc_b64, s_swappc_b64 et
s_call_b64 (interpréteur seulement ; sauts indirects refusés en HLSL en les nommant).

Adresse d'une instruction = PROGRAM_BASE (0) + son décalage en octets. Valeurs attendues calculées à la main.
"""
import struct

import pytest

from emu import rdna2_exec, rdna2_gpu, rdna2_hlsl
from test_rdna2_alu_parity import sopk
from test_rdna2_exec import V, const, program, sop1, sop2, sopp, vop2
from test_rdna2_hlsl import compare, store_lane, with_lane_offset
import test_rdna2_hlsl_wave64 as wave64

NULL = 125
gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc ou anyxboxx_gpu_run.exe absent")


def raw(*words):
    """Programme sans s_endpgm ajouté (les tests le placent eux-mêmes)."""
    return struct.pack(f"<{len(words)}I", *words)


def test_getpc_returns_the_address_of_the_next_instruction():
    wave = rdna2_exec.run(program(sop1("s_getpc_b64", 10, 0), sop1("s_getpc_b64", 12, 0),
                                  sop1("s_getpc_b64", NULL, 0)))
    assert wave.s[10:14] == [4, 0, 8, 0]                       # décalages 0 et 4, + 4


def test_setpc_jumps_to_an_address_from_getpc():
    wave = rdna2_exec.run(program(
        sop1("s_getpc_b64", 10, 0),                            # 0 : s10 = 4
        sop2("s_add_u32", 10, 10, const(12)),                  # 4 : s10 = 16
        sop1("s_setpc_b64", 0, 10),                            # 8 : saut en 16
        sop1("s_mov_b32", 20, const(1)),                       # 12 : sauté
        sop1("s_mov_b32", 21, const(2))))                      # 16
    assert (wave.s[20], wave.s[21], wave.steps) == (0, 2, 4)


def test_swappc_calls_and_setpc_returns():
    wave = rdna2_exec.run(raw(
        sop1("s_getpc_b64", 10, 0),                            # 0 : s10 = 4
        sop2("s_add_u32", 10, 10, const(16)),                  # 4 : s10 = 20, le sous-programme
        sop1("s_swappc_b64", 30, 10),                          # 8 : s[30:31] = 12, saut en 20
        sop1("s_mov_b32", 21, const(2)),                       # 12 : après le retour
        sopp("s_endpgm"),                                      # 16
        sop1("s_mov_b32", 20, const(1)),                       # 20 : sous-programme
        sop1("s_setpc_b64", 0, 30)))                           # 24 : retour en 12
    assert (wave.s[20], wave.s[21], wave.s[30], wave.s[31]) == (1, 2, 12, 0)


def test_swappc_reads_its_target_before_writing_the_same_pair():
    wave = rdna2_exec.run(raw(
        sop1("s_mov_b32", 10, const(12)),                      # 0
        sop1("s_swappc_b64", 10, 10),                          # 4 : cible 12, puis s[10:11] = 8
        sopp("s_endpgm"),                                      # 8
        sopp("s_endpgm")))                                     # 12
    assert (wave.s[10], wave.pc) == (8, 12)


def test_call_jumps_relative_and_returns():
    wave = rdna2_exec.run(raw(
        sopk("s_call_b64", 30, 3),                             # 0 : s[30:31] = 4, saut en 0 + 4 + 3 × 4 = 16
        sop1("s_mov_b32", 21, const(2)),                       # 4 : après le retour
        sopp("s_endpgm"),                                      # 8
        sop1("s_mov_b32", 22, const(3)),                       # 12 : jamais exécutée
        sop1("s_mov_b32", 20, const(1)),                       # 16
        sop1("s_setpc_b64", 0, 30)))                           # 20 : retour en 4
    assert (wave.s[20], wave.s[21], wave.s[22], wave.s[30]) == (1, 2, 0, 4)


@pytest.mark.parametrize("words, message", [
    ((sop1("s_mov_b32", 10, const(2)), sop1("s_setpc_b64", 0, 10)), "s_setpc_b64 : cible 0x2 hors du programme"),
    ((sop1("s_mov_b32", 10, const(64)), sop1("s_swappc_b64", 12, 10)), "s_swappc_b64 : cible 0x40 hors"),
    ((sopk("s_call_b64", 30, -2),), "s_call_b64 : cible 0xFFFFFFFFFFFFFFFC hors"),
    ((sop1("s_setpc_b64", 0, const(4)),), "s_setpc_b64 : S0 doit être une paire de SGPR"),
    ((sop1("s_getpc_b64", 11, 0),), "s_getpc_b64 : destination s11 impaire"),
])
def test_bad_targets_and_registers_are_named(words, message):
    with pytest.raises(rdna2_exec.Untranslated, match=message):
        rdna2_exec.run(program(*words))


@pytest.mark.parametrize("word, reason", [
    (sop1("s_setpc_b64", 0, 10), "saut indirect"),
    (sop1("s_swappc_b64", 30, 10), "saut indirect"),
    (sopk("s_call_b64", 30, 1), "appel de sous-programme"),
    (sop1("s_getpc_b64", 11, 0), "impaire"),
])
def test_indirect_jumps_stay_untranslatable_in_hlsl(word, reason):
    with pytest.raises(rdna2_hlsl.Untranslatable, match=reason):
        rdna2_hlsl.translate(program(word))
    with pytest.raises(rdna2_hlsl.Untranslatable, match=reason):
        rdna2_hlsl.translate(program(word), wave_size=64)


def getpc_program(lane_offset, store):
    """s_getpc_b64 à plusieurs décalages ; chaque ligne y ajoute son numéro pour des résultats différents."""
    ops = [
        sop1("s_getpc_b64", 10, 0),
        sop2("s_add_u32", 10, 10, const(40)),
        sop1("s_getpc_b64", 12, 0),
        sop1("s_getpc_b64", NULL, 0),
        sop1("s_getpc_b64", 14, 0),
        vop2("v_add_nc_u32", 1, 10, V + 0),
        vop2("v_add_nc_u32", 2, 11, V + 0),
        vop2("v_add_nc_u32", 3, 12, V + 0),
        vop2("v_add_nc_u32", 4, 14, V + 0),
        vop2("v_add_nc_u32", 5, 15, V + 0),
    ]
    stores = [word for slot, register in enumerate(range(1, 6)) for word in store(register, slot)]
    return lane_offset(*ops, *stores)


def test_hlsl_writes_each_getpc_address_as_a_constant():
    hlsl = rdna2_hlsl.translate(getpc_program(with_lane_offset, store_lane))
    assert "s[10] = 0x8u; s[11] = 0x0u;" in hlsl              # v_lshlrev_b32 en 0, s_getpc en 4
    assert "s[12] = 0x10u;" in hlsl and "s[14] = 0x18u;" in hlsl and "0x14u" not in hlsl   # NULL : rien


def test_getpc_program_reads_the_expected_addresses():
    wave = rdna2_exec.run(getpc_program(with_lane_offset, store_lane))
    assert (wave.s[10], wave.s[12], wave.s[14]) == (48, 16, 24)        # 8 + 40 ; 12 + 4 ; 20 + 4


@gpu
def test_getpc_matches_interpreter_on_gpu():
    expected, actual = compare(getpc_program(with_lane_offset, store_lane))
    assert actual == expected


@gpu
def test_getpc_matches_interpreter_on_gpu_in_wave64():
    expected, actual = wave64.compare(getpc_program(wave64.with_lane_offset, wave64.store_lane))
    assert actual == expected
