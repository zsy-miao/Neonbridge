"""v_sat_pk_u8_i16 (VOP1 0x062, VOP3 0x1E2) : interpréteur, HLSL et parité GPU.

Sémantique : spécification XML RDNA 2 d'AMD (« Given 2 signed 16-bit integer inputs, saturate each input
over an unsigned 8-bit integer range, pack the resulting values into a packed 16-bit value »,
destination de 16 bits). D[31:16] mis à 0 : mesuré sur RDNA 2 (gfx1036, VIctory #499,
https://github.com/Marvisak/VIctory/pull/499#issuecomment-7123377192), en VOP1 comme en VOP3.
Valeurs attendues calculées à la main : octet 0 = sat(S0[15:0]), octet 1 = sat(S0[31:16]).
"""
import struct

import pytest

from emu import rdna2_exec, rdna2_gpu, rdna2_hlsl
from test_rdna2_exec import V, const, program, sop1, vop1, vop2, vop3
from test_rdna2_exec_memory import INPUT, global_
from test_rdna2_hlsl import compare, store_lane, with_lane_offset
from test_rdna2_sdwa_dpp_exec import DPP8, DPP16, SDWA, dpp8_word, dpp16_word, run_wave, sdwa_word

LITERAL, MINUS_ONE = 255, 193
OLD = 0xABCD_1234                     # destination avant l'instruction : D[31:16] = 0xABCD doit disparaître
gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc ou anyxboxx_gpu_run.exe absent")

CASES = [
    (0x0000_0000, 0x0000_0000),       # 0, 0
    (0x00FF_00FF, 0x0000_FFFF),       # 255, 255 : bornes incluses
    (0x0100_0100, 0x0000_FFFF),       # 256 → 255 dans les deux moitiés
    (0xFFFF_8000, 0x0000_0000),       # -1 → 0 (haut), -32768 → 0 (bas)
    (0x7FFF_0080, 0x0000_FF80),       # 32767 → 255 (haut), 128 (bas)
    (0x00FE_0001, 0x0000_FE01),       # 254 (haut), 1 (bas)
    (0x8000_7FFF, 0x0000_00FF),       # -32768 → 0 (haut), 32767 → 255 (bas)
    (0x0001_FFFF, 0x0000_0100),       # 1 (haut), -1 → 0 (bas)
]


def lanes(values):
    return [values[lane % len(values)] for lane in range(32)]


def test_vop1_saturates_both_halves_and_clears_the_high_word():
    inputs = [source for source, _ in CASES]
    wave = run_wave([vop1("v_sat_pk_u8_i16", 2, V + 1)], init_v={1: lanes(inputs), 2: [OLD] * 32})
    assert wave.v[2] == lanes([expected for _, expected in CASES])


def test_vop3_reads_scalar_and_inline_sources():
    wave = rdna2_exec.run(program(
        sop1("s_mov_b32", 4, LITERAL), 0x0123_FF85,
        vop1("v_mov_b32", 2, LITERAL), OLD, vop1("v_mov_b32", 3, LITERAL), OLD, vop1("v_mov_b32", 5, LITERAL), OLD,
        *vop3("v_sat_pk_u8_i16", 2, 4),                 # haut 0x0123 = 291 → 255, bas 0xFF85 = -123 → 0
        *vop3("v_sat_pk_u8_i16", 3, MINUS_ONE),         # -1 = 0xFFFFFFFF : -1 et -1 → 0, 0
        *vop3("v_sat_pk_u8_i16", 5, const(64))))        # 0x00000040 : haut 0, bas 64
    assert (wave.v[2][0], wave.v[3][0], wave.v[5][0]) == (0x0000_FF00, 0x0000_0000, 0x0000_0040)


def test_dpp16_reads_the_permuted_lane():
    # quad_perm:[3,2,1,0] : la ligne 0 lit la ligne 3 (0xFFFF_8000 → 0), la ligne 1 la ligne 2 (0x0100_0100 → 0xFFFF)
    inputs = [source for source, _ in CASES]
    wave = run_wave([vop1("v_sat_pk_u8_i16", 2, DPP16), dpp16_word(1, 0x1B)],
                    init_v={1: lanes(inputs), 2: [OLD] * 32})
    assert wave.v[2][:4] == [0x0000_0000, 0x0000_FFFF, 0x0000_FFFF, 0x0000_0000]


# Lignes de la mesure (gfx1036, destination préchargée à 0xDEADBEEF) : source → résultat, VOP1 et VOP3.
MEASURED = [
    (0x00FF_0001, 0x0000_FF01), (0x0000_0100, 0x0000_00FF), (0x0100_0100, 0x0000_FFFF),
    (0x0000_7FFF, 0x0000_00FF), (0x0000_8000, 0x0000_0000), (0x0080_8000, 0x0000_8000),
    (0x0000_FFFF, 0x0000_0000), (0x0000_FF80, 0x0000_0000), (0x0100_0123, 0x0000_FFFF),
]


def test_measured_rows_clear_the_high_word_in_both_encodings():
    inputs = [source for source, _ in MEASURED]
    wave = run_wave([vop1("v_sat_pk_u8_i16", 2, V + 1), *vop3("v_sat_pk_u8_i16", 3, V + 1)],
                    init_v={1: lanes(inputs), 2: [0xDEAD_BEEF] * 32, 3: [0xDEAD_BEEF] * 32})
    expected = lanes([result for _, result in MEASURED])
    assert wave.v[2] == expected and wave.v[3] == expected


@pytest.mark.parametrize("words, message", [
    (vop3("v_sat_pk_u8_i16", 2, V + 1, sdst=0x40), "op_sel"),       # op_sel[3] : moitié haute de D
    (vop3("v_sat_pk_u8_i16", 2, V + 1, clamp=1), "clamp"),
    ((vop1("v_sat_pk_u8_i16", 2, SDWA), sdwa_word(1)), "SDWA"),
])
def test_unverified_forms_are_refused_by_name(words, message):
    code = program(*words)
    with pytest.raises(rdna2_exec.Untranslated, match=f"(?=.*v_sat_pk_u8_i16)(?=.*{message})"):
        rdna2_exec.run(code)
    with pytest.raises(rdna2_hlsl.Untranslatable, match=f"(?=.*v_sat_pk_u8_i16)(?=.*{message})"):
        rdna2_hlsl.translate(code)


def parity_memory():
    """Une valeur par ligne : les cas de CASES, puis des bits pseudo-aléatoires."""
    words = [CASES[lane][0] if lane < len(CASES) else (lane * 2654435761) & 0xFFFF_FFFF for lane in range(32)]
    image = bytearray(INPUT + 128)
    image[INPUT:] = struct.pack("<32I", *words)
    return bytes(image)


def parity_program():
    ops = [
        *global_("global_load_dword", addr=31, vdst=1, saddr=0),
        vop2("v_add_nc_u32", 2, LITERAL, 31), OLD,                      # D[31:16] différent par ligne
        vop2("v_add_nc_u32", 3, LITERAL, 31), 0x5A5A_0000,
        vop2("v_add_nc_u32", 4, LITERAL, 31), 0x0F0F_0000,
        vop2("v_add_nc_u32", 5, LITERAL, 31), 0x7777_0000,
        vop1("v_sat_pk_u8_i16", 2, V + 1),
        *vop3("v_sat_pk_u8_i16", 3, V + 1),
        vop1("v_sat_pk_u8_i16", 4, DPP16), dpp16_word(1, 0x1B, bound_ctrl=1),
        vop1("v_sat_pk_u8_i16", 5, DPP8), dpp8_word(1, [7, 6, 5, 4, 3, 2, 1, 0]),
    ]
    stores = [word for slot, register in enumerate((2, 3, 4, 5)) for word in store_lane(register, slot)]
    return with_lane_offset(*ops, *stores)


def test_hlsl_translates_every_form():
    hlsl = rdna2_hlsl.translate(parity_program())
    assert "0xFFFF0000u" not in hlsl and hlsl.count("clamp((int)") == 8


@gpu
def test_sat_pk_matches_interpreter_on_gpu():
    expected, actual = compare(parity_program(), memory=parity_memory())
    assert actual == expected
