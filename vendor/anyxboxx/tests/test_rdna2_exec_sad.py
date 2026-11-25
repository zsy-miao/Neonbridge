"""v_msad_u8, v_qsad_pk_u16_u8, v_mqsad_pk_u16_u8 et v_mqsad_u32_u8 : interpréteur de référence, traduction
HLSL et parité GPU (wave32, wave64).

Valeurs attendues calculées à la main (arithmétique en commentaire) d'après la spécification XML d'AMD
(RDNA 2) et le pseudo-code RDNA 3 d'AMD : fenêtres S0[31:0], S0[39:8], S0[47:16], S0[55:24].
"""
import pytest

from emu import rdna2_exec, rdna2_gpu, rdna2_hlsl
from test_rdna2_exec import V, program, vop1, vop2, vop3
from test_rdna2_exec_memory import OUTPUT
from test_rdna2_hlsl import compare, store_lane, with_lane_offset
import test_rdna2_hlsl_wave64 as wave64

LITERAL, MINUS_ONE = 255, 193
S0_LOW, S0_HIGH = 0x0403_0201, 0x0807_0605                  # octets 1, 2, …, 8 de bas en haut
gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc ou anyxboxx_gpu_run.exe absent")


def mov(vdst, value):
    return vop1("v_mov_b32", vdst, LITERAL), value & 0xFFFF_FFFF


def run(*words):
    return rdna2_exec.run(program(*words))


def test_msad_skips_bytes_whose_reference_is_zero():
    wave = run(*mov(1, 0x1020_3040), *mov(2, 0x0025_003A),
               *vop3("v_msad_u8", 3, V + 1, V + 2, LITERAL), 100,
               *vop3("v_sad_u8", 4, V + 1, V + 2, LITERAL), 100)
    assert wave.v[3][0] == 111            # |0x40-0x3A| + |0x20-0x25| = 6 + 5, octets 1 et 3 exclus, + 100
    assert wave.v[4][0] == 175            # v_sad_u8 compte tout : 6 + 48 + 5 + 16 + 100


def quad_program(name, reference, accumulators, words):
    """S0 = v[1:2], S1 = v3, S2 = v[4:...] ; résultat dans v[10:...]."""
    ops = [*mov(1, S0_LOW), *mov(2, S0_HIGH), *mov(3, reference)]
    for i, value in enumerate(accumulators[:words]):
        ops += mov(4 + i, value)
    return program(*ops, *vop3(name, 10, V + 1, V + 3, V + 4))


def test_qsad_packs_four_16_bit_sums():
    # fenêtres (octets) 1-4, 2-5, 3-6, 4-7 contre 2, 3, 4, 5 : SAD 4, 0, 4, 8 ; accumulateurs 1, 2, 3, 4
    wave = rdna2_exec.run(quad_program("v_qsad_pk_u16_u8", 0x0504_0302, (0x0002_0001, 0x0004_0003), 2))
    assert (wave.v[10][0], wave.v[11][0]) == (0x0002_0005, 0x000C_0007)


def test_qsad_truncates_each_sum_to_16_bits():
    # 4 + 0xFFFF = 0x10003 → 0x0003 ; la retenue ne passe pas dans le résultat voisin (0 + 2)
    wave = rdna2_exec.run(quad_program("v_qsad_pk_u16_u8", 0x0504_0302, (0x0002_FFFF, 0x0004_0003), 2))
    assert (wave.v[10][0], wave.v[11][0]) == (0x0002_0003, 0x000C_0007)


def test_mqsad_pk_masks_the_zero_reference_byte():
    # référence 2, 3, 0, 5 : l'octet 2 est exclu → SAD 1+1+1 = 3, 0, 1+1+1 = 3, 2+2+2 = 6 ; + 1, 2, 3, 4
    wave = rdna2_exec.run(quad_program("v_mqsad_pk_u16_u8", 0x0500_0302, (0x0002_0001, 0x0004_0003), 2))
    assert (wave.v[10][0], wave.v[11][0]) == (0x0002_0004, 0x000A_0006)


def test_mqsad_u32_writes_four_32_bit_sums():
    # SAD masqués 3, 0, 3, 6 ; accumulateurs 0xFFFFFFFF, 100, 0x10000, 7 (3 - 1 = 2 modulo 2^32)
    wave = rdna2_exec.run(quad_program("v_mqsad_u32_u8", 0x0500_0302, (0xFFFF_FFFF, 100, 0x10000, 7), 4))
    assert [wave.v[10 + i][0] for i in range(4)] == [2, 100, 0x10003, 13]


@pytest.mark.parametrize("words, reason", [
    (vop3("v_mqsad_pk_u16_u8", 10, V + 1, V + 3, V + 4, clamp=1), "clamp"),
    (vop3("v_msad_u8", 10, V + 1, V + 3, V + 4, abs_=1), "abs"),
    (vop3("v_qsad_pk_u16_u8", 10, V + 1, V + 3, V + 4, neg=1), "neg"),
    ((*vop3("v_qsad_pk_u16_u8", 10, LITERAL, V + 3, V + 4), 7), "littéral"),
    (vop3("v_mqsad_pk_u16_u8", 10, V + 1, V + 3, 242), "flottante"),     # 1.0 en ligne sur S2 (64 bits)
    (vop3("v_mqsad_u32_u8", 10, V + 1, V + 3, 20), "VGPR"),
])
def test_undocumented_forms_are_refused_by_name(words, reason):
    code = program(*words)
    name = rdna2_exec.rdna2.mnemonic("VOP3", (words[0] >> 16) & 0x3FF)
    with pytest.raises(rdna2_exec.Untranslated, match=f"{name}.*{reason}"):
        rdna2_exec.run(code)
    with pytest.raises(rdna2_hlsl.Untranslatable, match=f"{name}.*{reason}"):
        rdna2_hlsl.translate(code)


def sad_program(lane_offset, store):
    """S0, référence (octets nuls selon la ligne) et accumulateurs propres à chaque ligne."""
    ops = [
        vop2("v_mul_u32_u24", 1, LITERAL, 0), 0x0103_0507,
        vop2("v_xor_b32", 1, LITERAL, V + 1), 0x80F0_7F11,
        vop2("v_mul_u32_u24", 2, LITERAL, 0), 0x00F1_E3D5,
        vop2("v_mul_u32_u24", 3, LITERAL, 0), 0x0093_7A15,
        vop2("v_and_b32", 3, LITERAL, V + 3), 0xF0F0_F0F0,          # octets nuls sur certaines lignes
        vop2("v_mul_u32_u24", 4, LITERAL, 0), 0x0001_FFF3,
        vop2("v_xor_b32", 5, LITERAL, V + 2), 0xFFF0_0FFF,
        vop2("v_mul_u32_u24", 6, LITERAL, 0), 0x00FE_DCBA,
        vop2("v_sub_nc_u32", 7, LITERAL, V + 1), 0x1234_5678,
        *vop3("v_msad_u8", 10, V + 1, V + 3, V + 4),
        *vop3("v_msad_u8", 11, LITERAL, V + 3, V + 0), 0x00FF_8001,
        *vop3("v_qsad_pk_u16_u8", 12, V + 1, V + 3, V + 4),
        *vop3("v_mqsad_pk_u16_u8", 14, V + 1, V + 3, V + 4),
        *vop3("v_mqsad_pk_u16_u8", 16, 20, V + 2, MINUS_ONE),        # S0 = s[20:21], S2 = -1 sur 64 bits
        *vop3("v_mqsad_u32_u8", 18, V + 1, V + 3, V + 4),
    ]
    stores = [word for slot, register in enumerate(range(10, 22)) for word in store(register, slot)]
    return lane_offset(*ops, *stores)


def test_hlsl_uses_the_sad_helpers():
    hlsl = rdna2_hlsl.translate(sad_program(with_lane_offset, store_lane))
    for token in ("uint MsadU8(", "QsadPk(v[1], v[2], v[3], uint2(v[4], v[5]), false)",
                  "QsadPk(s[20], s[21], v[2], uint2(0xFFFFFFFFu, 0xFFFFFFFFu), true)",
                  "MqsadU32(v[1], v[2], v[3], uint4(v[4], v[5], v[6], v[7]))"):
        assert token in hlsl


def test_parity_program_writes_lane_varying_results():
    """Garde-fou des tests GPU : résultats différents d'une ligne à l'autre, octets de référence nuls inclus."""
    wave = rdna2_exec.Wave()
    wave.s[2], wave.s[20] = OUTPUT, 0x0102_0304
    rdna2_exec.run(sad_program(with_lane_offset, store_lane), wave)
    assert all(len(set(wave.v[register])) > 16 for register in range(10, 22))
    assert any(value & 0xFF00 == 0 for value in wave.v[3]) and any(value & 0xFF00 for value in wave.v[3])


@gpu
def test_sad_operations_match_interpreter_on_gpu():
    expected, actual = compare(sad_program(with_lane_offset, store_lane), sgprs={20: 0x0102_0304, 21: 0x8070_6050})
    assert actual == expected


@gpu
def test_sad_operations_match_interpreter_on_gpu_in_wave64():
    expected, actual = wave64.compare(sad_program(wave64.with_lane_offset, wave64.store_lane))
    assert actual == expected
