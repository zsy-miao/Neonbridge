"""Half-precision compares v_cmp[x]_*_f16 and v_cmp[x]_class_f16 (VOPC, VOP3) in the reference interpreter.

Semantics: AMD "RDNA 2" ISA (November 2020), section 12.9 (p. 156-161), section 3.9 (the mask is fully
written) and section 6.2.5 (no OPSEL on VOPC in VOP3). Expected masks come from the relation of each pair,
written by hand below, not from the interpreter.
"""
import pytest

from emu import rdna2_exec, rdna2_hlsl
from test_rdna2_exec import V, const, program, sop1, vop3, vopc

LITERAL, ONE, VCC_LO, EXEC_LO = 255, 242, 106, 126
SDWA = 0xF9
PARTIAL_EXEC = 0x0F0F_F0F3

# (S0, S1, relation of the binary16 low halves); the high halves are junk that a 32-bit compare would see.
PAIRS = [
    (0xABCD_3C00, 0x1234_4000, "lt"),       # 1.0 < 2.0
    (0x0000_8000, 0xFFFF_0000, "eq"),       # -0 == +0
    (0x7FFF_7E00, 0x0000_3C00, "un"),       # quiet NaN
    (0x0000_3C00, 0x3F80_7D00, "un"),       # signalling NaN in S1
    (0x8000_0001, 0x0000_0000, "gt"),       # smallest denormal > +0
    (0x0000_8001, 0x8000_0000, "lt"),       # -denormal < +0
    (0x0000_7C00, 0x0000_7BFF, "gt"),       # +inf > 65504
    (0x7F80_FC00, 0x0000_FBFF, "lt"),       # -inf < -65504
    (0x1111_C000, 0x2222_C000, "eq"),       # -2.0 == -2.0
    (0x0000_3555, 0x0000_3556, "lt"),       # neighbours
    (0x0000_7E00, 0x0000_7E00, "un"),       # NaN != NaN
    (0x0000_4000, 0x0000_3C00, "gt"),
]
HOLDS = {"f": set(), "lt": {"lt"}, "eq": {"eq"}, "le": {"lt", "eq"}, "gt": {"gt"}, "lg": {"lt", "gt"},
         "ge": {"gt", "eq"}, "o": {"lt", "eq", "gt"}, "u": {"un"}, "nge": {"lt", "un"}, "nlg": {"eq", "un"},
         "ngt": {"lt", "eq", "un"}, "nle": {"gt", "un"}, "neq": {"lt", "gt", "un"}, "nlt": {"gt", "eq", "un"},
         "tru": {"lt", "eq", "gt", "un"}}
# One value per class bit of S1 (0 sNaN … 9 +inf), in binary16.
CLASSES = [0x7C01, 0x7E00, 0xFC00, 0xBC00, 0x8001, 0x8000, 0x0000, 0x03FF, 0x3C00, 0x7C00]


def lanes(values, count=32):
    return [values[lane % len(values)] for lane in range(count)]


def run(*words, s0=(), s1=(), exec_mask=None):
    wave = rdna2_exec.Wave()
    wave.v[1], wave.v[2] = lanes(list(s0) or [0]), lanes(list(s1) or [0])
    if exec_mask is not None:
        wave.write_mask(EXEC_LO, exec_mask)
    rdna2_exec.run(program(*words), wave)
    return wave


def expected(operation, exec_mask=0xFFFF_FFFF):
    return sum(1 << lane for lane in range(32)
               if PAIRS[lane % len(PAIRS)][2] in HOLDS[operation]) & exec_mask


def pair_wave(*words, exec_mask=None):
    return run(*words, s0=[a for a, _, _ in PAIRS], s1=[b for _, b, _ in PAIRS], exec_mask=exec_mask)


def test_every_name_is_executable_in_vopc_and_vop3():
    names = [f"{prefix}_{operation}_f16" for prefix in ("v_cmp", "v_cmpx") for operation in (*HOLDS, "class")]
    assert len(names) == 34
    assert all(rdna2_exec.executable(family, name) for family in ("VOPC", "VOP3") for name in names)


@pytest.mark.parametrize("operation", HOLDS)
def test_vopc_writes_vcc_and_clears_inactive_lanes(operation):
    wave = pair_wave(vopc(f"v_cmp_{operation}_f16", V + 1, 2), exec_mask=PARTIAL_EXEC)
    assert wave.vcc == expected(operation, PARTIAL_EXEC)
    assert wave.exec == PARTIAL_EXEC


@pytest.mark.parametrize("operation", HOLDS)
def test_vop3_writes_its_sgpr(operation):
    wave = pair_wave(*vop3(f"v_cmp_{operation}_f16", 40, V + 1, V + 2))
    assert wave.s[40] == expected(operation)
    assert wave.vcc == 0


@pytest.mark.parametrize("operation", ["lt", "nge", "u", "tru"])
def test_cmpx_writes_exec_only(operation):
    vopc_wave = pair_wave(vopc(f"v_cmpx_{operation}_f16", V + 1, 2), exec_mask=PARTIAL_EXEC)
    vop3_wave = pair_wave(*vop3(f"v_cmpx_{operation}_f16", EXEC_LO, V + 1, V + 2), exec_mask=PARTIAL_EXEC)
    for wave in (vopc_wave, vop3_wave):
        assert wave.exec == expected(operation, PARTIAL_EXEC)
        assert wave.vcc == 0


def test_constants_are_binary16_or_low_halves():
    """Inline 1.0 is 0x3C00 for an f16 source; a literal and an integer constant give their low 16 bits."""
    wave = run(vopc("v_cmp_eq_f16", ONE, 1), sop1("s_mov_b32", 40, VCC_LO),
               vopc("v_cmp_eq_f16", LITERAL, 1), 0xDEAD_3C00, sop1("s_mov_b32", 41, VCC_LO),
               *vop3("v_cmp_eq_f16", 42, V + 1, const(0)),
               s0=[0x0000_3C00, 0x3F80_0000, 0x3C00_0000])        # 1.0 in f16, 1.0 in f32 (low half 0), +0
    one = sum(1 << lane for lane in range(32) if lane % 3 == 0)
    assert (wave.s[40], wave.s[41]) == (one, one)
    assert wave.s[42] == sum(1 << lane for lane in range(32) if lane % 3)


def test_vop3_abs_and_neg_act_on_bit_15():
    wave = run(*vop3("v_cmp_eq_f16", 40, V + 1, V + 2, abs_=0b001),     # |-1.0| == 1.0
               *vop3("v_cmp_eq_f16", 41, V + 1, V + 2, neg=0b010),      # -1.0 == -(1.0)
               *vop3("v_cmp_lt_f16", 42, V + 1, V + 2),                 # -1.0 < 1.0
               s0=[0x0000_BC00], s1=[0x8000_3C00])
    assert (wave.s[40], wave.s[41], wave.s[42]) == (0xFFFF_FFFF,) * 3


def test_class_tests_each_class_bit():
    """Lane i holds the value of class i % 10; S1 = 1 << (lane % 12) also tries the unused bits 10 and 11."""
    s1 = [1 << (lane % 12) for lane in range(32)]
    wave = run(vopc("v_cmp_class_f16", V + 1, 2), *vop3("v_cmp_class_f16", 40, V + 1, V + 2),
               s0=[0xFFFF_0000 | CLASSES[lane % 10] for lane in range(32)], s1=s1)
    want = sum(1 << lane for lane in range(32) if lane % 10 == lane % 12)
    assert wave.vcc == want and wave.s[40] == want


def test_class_mask_constants():
    """const(3) selects both NaNs; an inline float in S1 is its binary16 pattern (1.0 = 0x3C00: no class bit)."""
    wave = run(*vop3("v_cmp_class_f16", 40, V + 1, const(3)), *vop3("v_cmp_class_f16", 41, V + 1, ONE),
               *vop3("v_cmp_class_f16", 42, V + 1, V + 2, neg=0b001),   # -(+0) is -0: class 5
               s0=CLASSES, s1=[1 << 5])
    assert wave.s[40] == sum(1 << lane for lane in range(32) if lane % 10 in (0, 1))
    assert wave.s[41] == 0
    assert wave.s[42] == sum(1 << lane for lane in range(32) if lane % 10 == 6)


@pytest.mark.parametrize("words, reason", [
    (vop3("v_cmp_lt_f16", 40, V + 1, V + 2, op_sel=0b0001), "op_sel"),
    (vop3("v_cmp_lt_f16", 40, V + 1, V + 2, clamp=1), "clamp"),
    (vop3("v_cmp_lt_f16", 40, V + 1, V + 2, omod=1), "omod"),
    (vop3("v_cmp_class_f16", 40, V + 1, V + 2, abs_=0b010), "class mask"),
    ((vopc("v_cmp_lt_f16", SDWA, 2), 1), "SDWA"),
])
def test_unmeasured_forms_are_refused_by_name(words, reason):
    with pytest.raises(rdna2_exec.Untranslated, match=reason):
        run(*words)


@pytest.mark.parametrize("words", [(vopc("v_cmp_lt_f16", V + 1, 2),), vop3("v_cmp_class_f16", 40, V + 1, V + 2)])
def test_hlsl_still_refuses_f16_compares(words):
    """The HLSL recompiler does not translate them yet: it must refuse, not reuse the f32 compare."""
    with pytest.raises(rdna2_hlsl.Untranslatable):
        rdna2_hlsl.translate(program(*words))
