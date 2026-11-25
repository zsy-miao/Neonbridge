"""FMA f32 arrondies une seule fois : v_fma_f32, v_fmac_f32 (VOP2 et VOP3), v_fmamk_f32, v_fmaak_f32 et
v_fma_legacy_f32, dans l'interpréteur et dans le HLSL (FmaF32, emu/rdna2_hlsl_fma.py).

Le double arrondi (somme double, puis simple) a été signalé sur #133 par akimfomin avec PortBridge ; les
six premiers triplets viennent de son rapport. Les attentes viennent d'un calcul exact en rationnels
(`exact`), pas de l'interpréteur. Les NaN suivent la règle de l'interpréteur (premier opérande NaN rendu
silencieux, sinon 0x7FC00000), supposée et pas encore mesurée sur RDNA 2.
"""
import random
import struct
from fractions import Fraction

import pytest

from emu import rdna2_exec, rdna2_gpu
from emu.rdna2_exec_ops import bits_float, float_bits
from test_rdna2_exec import LITERAL, V, const, program, sop1, sop2, sopc, sopp, vop1, vop2, vop3
from test_rdna2_exec_memory import INPUT, OUTPUT, global_
from test_rdna2_hlsl import compare, store_lane, with_lane_offset

gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc ou anyxboxx_gpu_run.exe absent")

QUIET, DEFAULT_NAN, INFINITY, SIGN = 0x0040_0000, 0x7FC0_0000, 0x7F80_0000, 0x8000_0000

# (a, b, c, a × b + c arrondi une fois, ce qui est vérifié). 1,5 × (1 + 2^-23) = 1,5 + 3 × 2^-24 est au
# milieu de deux f32 ; ±2^-76 place la somme exacte d'un côté ou de l'autre, mais disparaît dans la
# somme double. Les autres lignes séparent le calcul fusionné du calcul non fusionné (produit arrondi
# d'abord, ce que donne `mad` sur WARP) ou fixent les cas particuliers d'IEEE 754.
DIRECTED = [
    (0x3FC0_0000, 0x3F80_0001, 0x9980_0000, 0x3FC0_0001, "milieu, somme exacte juste en dessous"),
    (0x3FC0_0000, 0x3F80_0003, 0x1980_0000, 0x3FC0_0005, "milieu, somme exacte juste au-dessus"),
    (0xBFC0_0000, 0x3F80_0001, 0x1980_0000, 0xBFC0_0001, "milieu négatif"),
    (0x3FC0_0000, 0x3F80_0001, 0x0000_0000, 0x3FC0_0002, "égalité exacte : pair"),
    (0x0080_0000, 0x3F7F_FFFF, 0x0000_0000, 0x0080_0000, "milieu entre normal et sous-normal"),
    (0x5B78_0000, 0x6384_2108, 0x8D80_0000, 0x7F7F_FFFF, "milieu de débordement : reste fini"),
    (0x3F80_0001, 0x3F80_0001, 0xBF80_0002, 0x2880_0000, "annulation : 2^-46 survit au produit"),
    (0x3F80_0000, 0x3F80_0000, 0xBF80_0000, 0x0000_0000, "annulation exacte : +0"),
    (0x7F7F_FFFF, 0x4000_0000, 0xFF7F_FFFF, 0x7F7F_FFFF, "produit hors format, somme finie"),
    (0x7F7F_FFFF, 0x4000_0000, 0x0000_0000, 0x7F80_0000, "débordement vers +inf"),
    (0xFF7F_FFFF, 0x3F80_0001, 0x0000_0000, 0xFF80_0000, "débordement vers -inf"),
    (0x0080_0000, 0x3F00_0000, 0x0000_0000, 0x0040_0000, "résultat sous-normal"),
    (0x0080_0003, 0x3F00_0000, 0x8000_0001, 0x0040_0000, "sous-normal, égalité vers le pair"),
    (0x0000_0003, 0x4B00_0000, 0x0000_0001, 0x0140_0000, "entrées sous-normales gardées"),
    (0x0000_0001, 0x0000_0001, 0x8000_0000, 0x0000_0000, "sous-dépassement total : +0"),
    (0x8000_0001, 0x0000_0001, 0x0000_0000, 0x8000_0000, "sous-dépassement total : -0"),
    (0x8000_0000, 0x3F80_0000, 0x8000_0000, 0x8000_0000, "-0 + -0 = -0"),
    (0x0000_0000, 0xBF80_0000, 0x0000_0000, 0x0000_0000, "-0 + +0 = +0"),
    (0x8000_0000, 0x8000_0000, 0x8000_0000, 0x0000_0000, "+0 + -0 = +0"),
    (0x0000_0000, 0x3F80_0000, 0x8000_0001, 0x8000_0001, "0 × 1 + sous-normal"),
    (0x7F80_0000, 0x3F80_0000, 0x3F80_0000, 0x7F80_0000, "inf × 1 + 1"),
    (0xFF80_0000, 0xBF80_0000, 0x7F80_0000, 0x7F80_0000, "-inf × -1 + inf"),
    (0x3F80_0000, 0x3F80_0000, 0xFF80_0000, 0xFF80_0000, "1 + -inf"),
    (0x7F80_0000, 0xBF80_0000, 0x7F80_0000, DEFAULT_NAN, "-inf + inf"),
    (0x7F80_0000, 0x0000_0000, 0x3F80_0000, DEFAULT_NAN, "inf × 0"),
    (0x8000_0000, 0xFF80_0000, 0x3F80_0000, DEFAULT_NAN, "-0 × -inf"),
    (0x7FC1_2345, 0x3F80_0000, 0x3F80_0000, 0x7FC1_2345, "NaN en a"),
    (0x7F80_0001, 0x3F80_0000, 0x0000_0000, 0x7FC0_0001, "NaN signalant en a, rendu silencieux"),
    (0x3F80_0000, 0xFFA0_0000, 0x7FC0_0002, 0xFFE0_0000, "NaN en b avant NaN en c"),
    (0x3F80_0000, 0x3F80_0000, 0x7F81_2345, 0x7FC1_2345, "NaN signalant en c"),
    (0x7F80_0000, 0x0000_0000, 0x7FC0_0005, 0x7FC0_0005, "inf × 0 + NaN : le NaN de c"),
]


def value(bits):
    """Valeur exacte d'un f32 fini."""
    return Fraction(struct.unpack("<f", struct.pack("<I", bits))[0])


def exact(a, b, c):
    """a × b + c exact, arrondi au plus proche pair en f32 (sous-normaux et débordement compris)."""
    total = value(a) * value(b) + value(c)
    if total == 0:      # zéro exact : -0 seulement si le produit et c sont tous deux -0 (IEEE 754, 6.3)
        return SIGN if value(a) * value(b) == 0 and (a ^ b) & SIGN and c == SIGN else 0
    sign, magnitude = (SIGN if total < 0 else 0), abs(total)
    exponent = magnitude.numerator.bit_length() - magnitude.denominator.bit_length()
    if Fraction(2) ** exponent > magnitude:
        exponent -= 1
    ulp = Fraction(2) ** (max(exponent, -126) - 23)
    whole, rest = divmod(magnitude, ulp)
    if rest > ulp / 2 or (rest == ulp / 2 and whole % 2):
        whole += 1
    if whole * ulp >= 2 ** 128:
        return sign | INFINITY
    return sign | struct.unpack("<I", struct.pack("<f", float(whole * ulp)))[0]


def is_nan(bits):
    return bits & 0x7F80_0000 == INFINITY and bits & 0x7F_FFFF


def is_inf(bits):
    return bits & 0x7FFF_FFFF == INFINITY


def reference(a, b, c):
    """Résultat attendu de V_FMA_F32 pour tous les bits d'entrée."""
    for operand in (a, b, c):
        if is_nan(operand):
            return operand | QUIET
    zero_product = a & 0x7FFF_FFFF == 0 or b & 0x7FFF_FFFF == 0
    if is_inf(a) or is_inf(b):
        product_sign = (a ^ b) & SIGN
        if zero_product or (is_inf(c) and c & SIGN != product_sign):
            return DEFAULT_NAN
        return product_sign | INFINITY
    return c if is_inf(c) else exact(a, b, c)


def legacy(a, b, c):
    """V_FMA_LEGACY_F32 : « DX9 rules, 0.0 * x = 0.0 », donc c (rendu silencieux s'il est NaN)."""
    if a & 0x7FFF_FFFF == 0 or b & 0x7FFF_FFFF == 0:
        return c | QUIET if is_nan(c) else c
    return reference(a, b, c)


def mov(register, bits):
    return vop1("v_mov_b32", register, LITERAL), bits


def fused_words(a, b, c):
    """Six encodages sur (a, b, c) en littéraux, résultats dans v4 à v9 (v9 : v_fma_legacy_f32)."""
    return [*mov(1, a), *mov(2, b), *mov(3, c),
            *mov(4, c), vop2("v_fmac_f32", 4, V + 1, 2),
            *mov(5, c), *vop3("v_fmac_f32", 5, V + 1, V + 2),
            *vop3("v_fma_f32", 6, V + 1, V + 2, V + 3),
            vop2("v_fmamk_f32", 7, V + 1, 3), b,
            vop2("v_fmaak_f32", 8, V + 1, 2), c,
            *vop3("v_fma_legacy_f32", 9, V + 1, V + 2, V + 3)]


def literal_program(cases):
    """Opérandes en littéraux (dxc peut tout replier) : fused_words par triplet, 8 emplacements chacun."""
    words = []
    for a, b, c in cases:
        words += fused_words(a, b, c)
        for register in range(4, 10):
            words += store_lane(register, register - 4)
        words += [sop2("s_add_u32", 2, 2, LITERAL), 8 * 128]
    return with_lane_offset(*words)


def memory_program(blocks):
    """Chaque ligne lit son triplet en mémoire (rien à replier) : a, b, c à INPUT + 384 × bloc + {0, 128,
    256} + 4 × ligne ; v_fmac_f32 (VOP2, VOP3), v_fma_f32, v_fma_legacy_f32 à OUTPUT + 512 × bloc."""
    body = [*global_("global_load_dword", addr=31, vdst=1, saddr=0),
            *global_("global_load_dword", addr=31, vdst=2, saddr=0, offset=128),
            *global_("global_load_dword", addr=31, vdst=3, saddr=0, offset=256),
            vop1("v_mov_b32", 4, V + 3), vop2("v_fmac_f32", 4, V + 1, 2),
            vop1("v_mov_b32", 5, V + 3), *vop3("v_fmac_f32", 5, V + 1, V + 2),
            *vop3("v_fma_f32", 6, V + 1, V + 2, V + 3),
            *vop3("v_fma_legacy_f32", 7, V + 1, V + 2, V + 3),
            *store_lane(4, 0), *store_lane(5, 1), *store_lane(6, 2), *store_lane(7, 3),
            sop2("s_add_u32", 0, 0, LITERAL), 384,
            sop2("s_add_u32", 2, 2, LITERAL), 512,
            sop2("s_sub_u32", 4, 4, const(1)),
            sopc("s_cmp_lg_u32", 4, const(0))]
    return with_lane_offset(sop1("s_mov_b32", 4, LITERAL), blocks, *body, sopp("s_cbranch_scc1", -len(body) - 1))


def memory_image(triples):
    """Image mémoire de memory_program pour une liste de triplets (complétée par des 1 × 1 + 0)."""
    triples = triples + [(0x3F80_0000, 0x3F80_0000, 0)] * (-len(triples) % 32)
    image = bytearray(INPUT + 12 * len(triples))
    for block in range(len(triples) // 32):
        chunk = triples[32 * block:32 * block + 32]
        for column in range(3):
            struct.pack_into("<32I", image, INPUT + 384 * block + 128 * column, *(t[column] for t in chunk))
    return bytes(image), len(triples) // 32


def results(memory, triples):
    """(résultats des quatre encodages, attentes) lus dans la mémoire finale de memory_program."""
    got, want = [], []
    for index, (a, b, c) in enumerate(triples):
        base = OUTPUT + 512 * (index // 32) + 4 * (index % 32)
        got.append(tuple(struct.unpack_from("<I", memory, base + 128 * slot)[0] for slot in range(4)))
        expected = reference(a, b, c)
        want.append((expected, expected, expected, legacy(a, b, c)))
    return got, want


def finite(rng):
    exponent = 0 if rng.random() < 0.15 else rng.randrange(1, 255)
    return rng.getrandbits(1) << 31 | exponent << 23 | rng.getrandbits(23)


def special(rng):
    return rng.choice((0, SIGN, INFINITY, SIGN | INFINITY, 0x7FC0_0000, 0x7F80_0001, 0xFFC1_2345, 1, SIGN | 1))


def sweep(seed, count):
    """Triplets tirés : produits au milieu de deux f32 plus un petit terme (double arrondi), annulations,
    f32 quelconques (sous-normaux compris) et valeurs particulières."""
    rng, triples = random.Random(seed), []
    while len(triples) < count:
        kind = len(triples) % 4
        if kind == 0:
            a = rng.choice((0x3FC0_0000, 0xBFC0_0000, 0x3F40_0000)) + (rng.randrange(-60, 60) << 23)
            b = 0x3F80_0000 | rng.randrange(0, 1 << 23, 2) | 1
            exponent = max(1, (a >> 23 & 0xFF) + (b >> 23 & 0xFF) - 127 - rng.randrange(20, 110))
            triples.append((a, b, rng.getrandbits(1) << 31 | exponent << 23 | rng.getrandbits(23)))
        elif kind == 1:
            a, b = finite(rng), finite(rng)
            product = -value(a) * value(b)
            if product == 0 or abs(product) >= 2 ** 127:
                continue
            c = struct.unpack("<I", struct.pack("<f", float(product)))[0] ^ rng.getrandbits(2)
            triples.append((a, b, c))
        elif kind == 2:
            triples.append((finite(rng), finite(rng), finite(rng)))
        else:
            triples.append(tuple(special(rng) if rng.random() < 0.4 else finite(rng) for _ in range(3)))
    return triples


DIRECTED_TRIPLES = [case[:3] for case in DIRECTED]
SWEEP = sweep(133, 2048 - len(DIRECTED))


@pytest.mark.parametrize("a, b, c, expected, label", DIRECTED, ids=[case[4] for case in DIRECTED])
def test_reference_gives_the_directed_results(a, b, c, expected, label):
    assert reference(a, b, c) == expected


@pytest.mark.parametrize("a, b, c, expected, label", DIRECTED, ids=[case[4] for case in DIRECTED])
def test_interpreter_rounds_once_in_every_encoding(a, b, c, expected, label):
    wave = rdna2_exec.run(program(*fused_words(a, b, c)))
    assert [wave.v[register][0] for register in range(4, 9)] == [expected] * 5
    assert wave.v[9][0] == legacy(a, b, c)


def test_interpreter_matches_the_exact_rounding_on_a_seeded_sweep():
    triples = DIRECTED_TRIPLES + SWEEP
    image, blocks = memory_image(triples)
    wave = rdna2_exec.Wave()
    wave.memory.write(0, image)
    wave.s[0], wave.s[2] = INPUT, OUTPUT
    rdna2_exec.run(memory_program(blocks), wave)
    got, want = results(wave.memory.read(0, OUTPUT + 512 * blocks), triples)
    wrong = [f"{a:08X} × {b:08X} + {c:08X} : {g} au lieu de {w}"
             for (a, b, c), g, w in zip(triples, got, want) if g != w]
    assert not wrong, f"{len(wrong)} écarts, dont " + "; ".join(wrong[:5])


def test_mad_and_mac_keep_the_double_sum():
    """v_mad_f32 et v_mac_f32 (retirées du RDNA 2, encodages RDNA 1) ne passent pas par la FMA exacte."""
    wave = rdna2_exec.run(program(*mov(1, 0x3FC0_0000), *mov(2, 0x3F80_0001), *mov(3, 0x9980_0000),
                                  *vop3("v_mad_f32", 4, V + 1, V + 2, V + 3),
                                  vop1("v_mov_b32", 5, V + 3), vop2("v_mac_f32", 5, V + 1, 2)))
    assert (wave.v[4][0], wave.v[5][0]) == (0x3FC0_0002, 0x3FC0_0002)


@gpu
def test_gpu_matches_interpreter_and_exact_rounding_on_memory_operands():
    triples = DIRECTED_TRIPLES + SWEEP
    image, blocks = memory_image(triples)
    expected, actual = compare(memory_program(blocks), memory=image)
    got, want = results(actual, triples)
    wrong = [f"{a:08X} × {b:08X} + {c:08X} : {g} au lieu de {w}"
             for (a, b, c), g, w in zip(triples, got, want) if g != w]
    assert not wrong, f"{len(wrong)} écarts sur le GPU, dont " + "; ".join(wrong[:5])
    assert actual == expected


@gpu
def test_gpu_matches_interpreter_when_dxc_folds_literal_operands():
    """Opérandes littéraux : dxc replie FmaF32 à la compilation, en arithmétique entière donc exacte."""
    expected, actual = compare(literal_program(DIRECTED_TRIPLES[:16]))
    assert actual == expected
    for index, (a, b, c, result, _) in enumerate(DIRECTED[:16]):
        slots = struct.unpack_from("<192I", actual, OUTPUT + 8 * 128 * index)[0:192:32]
        assert slots == (result,) * 5 + (legacy(a, b, c),), DIRECTED[index][4]


@pytest.mark.parametrize("bits, quiet", [
    (0x7F80_0001, 0x7FC0_0001), (0xFFA0_0000, 0xFFE0_0000), (0x7FBF_FFFF, 0x7FFF_FFFF),
    (0x7FC1_2345, 0x7FC1_2345), (0x7F80_0000, 0x7F80_0000), (0xFF80_0000, 0xFF80_0000), (0x3F80_0000, 0x3F80_0000),
])
def test_signalling_nan_operands_come_back_quiet_on_every_python(bits, quiet):
    """CPython 3.14 keeps the signalling bit through struct ("<f"); 3.12 and 3.13 cleared it."""
    assert float_bits(bits_float(bits)) == quiet
