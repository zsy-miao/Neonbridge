"""16-bit integer compares v_cmp[x]_{lt,eq,le,gt,ne,ge}_{i16,u16} (VOPC, VOP3, SDWA): interpreter and GPU parity.

Semantics: AMD "RDNA 2" ISA (November 2020), table 61 (p. 156-159): S0[15:0] against S1[15:0], signed or
unsigned; section 3.9: the mask is fully written (inactive lanes cleared); section 6.2.5: no OPSEL on VOPC
in VOP3. Inline floats give their float32 pattern, of which the compare reads bits 15:0 (LLVM and gfx1036
measurements of other 16-bit integer sources, emu/rdna2_exec_cmp16.py; the compares: PortBridge #83). Expected values are written by
hand below (RELATIONS); the GPU tests check the HLSL against the interpreter byte for byte.
"""
import struct

import pytest

import test_rdna2_hlsl_wave64 as wave64
from emu import rdna2_exec, rdna2_exec_cmp16, rdna2_exec_ext, rdna2_gpu, rdna2_hlsl
from test_rdna2_exec import V, const, program, sop1, vop1, vop2, vop3, vopc
from test_rdna2_exec_memory import INPUT, OUTPUT, global_
from test_rdna2_hlsl import compare, store_lane, with_lane_offset
from test_rdna2_sdwa_dpp_exec import SDWA, WORD_0, WORD_1, BYTE_0, sdwa_word

LITERAL, MINUS_ONE, HALF, ONE_OVER_TWO_PI = 255, 193, 240, 248
VCC_LO, EXEC_LO = 106, 126
ALL = 0xFFFF_FFFF
SLOTS_PER_BASE = 16
HIGH_HALF = 0xFFFF_0000
gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc or anyxboxx_gpu_run.exe missing")

# (S0, S1, signed relation of the low halves, unsigned relation); the high halves are junk that a 32-bit
# compare would see.
RELATIONS = [
    (0x1234_7FFF, 0xABCD_8000, "gt", "lt"),     # 32767 > -32768 ; 0x7FFF < 0x8000
    (0xFFFF_8000, 0x0000_7FFF, "lt", "gt"),     # -32768 < 32767 ; 0x8000 > 0x7FFF
    (0x0001_FFFF, 0xFFFF_0000, "lt", "gt"),     # -1 < 0 ; 0xFFFF > 0
    (0xDEAD_0005, 0xBEEF_0005, "eq", "eq"),     # same low half, different high halves
    (0x0000_0000, 0x0001_0000, "eq", "eq"),
    (0x8000_FFFF, 0x7FFF_FFFE, "gt", "gt"),     # -1 > -2 ; 0xFFFF > 0xFFFE
    (0x5555_8000, 0xAAAA_8001, "lt", "lt"),     # -32768 < -32767
    (0x0000_7FFE, 0xFFFF_7FFF, "lt", "lt"),
]
HOLDS = {"lt": {"lt"}, "le": {"lt", "eq"}, "eq": {"eq"}, "ne": {"lt", "gt"}, "ge": {"gt", "eq"}, "gt": {"gt"}}
OPERATIONS = tuple(HOLDS)
NAMES = [f"v_cmp_{operation}_{kind}" for kind in ("i16", "u16") for operation in OPERATIONS]


def with_sdst(word, sgpr):
    """VOPC SDWA word writing its mask to `sgpr` (SD bit 15, SDST bits 14:8) instead of VCC."""
    return word & ~0xFF00 | (0x80 | sgpr) << 8


def lanes(values, count=32):
    return [values[lane % len(values)] for lane in range(count)]


def expected_mask(name, count=32, exec_mask=None):
    operation, kind = name.split("_")[-2:]
    column = 2 if kind == "i16" else 3
    mask = sum(1 << lane for lane in range(count)
               if RELATIONS[lane % len(RELATIONS)][column] in HOLDS[operation])
    return mask if exec_mask is None else mask & exec_mask


def run(words, exec_mask=ALL, preload=ALL):
    wave = rdna2_exec.Wave()
    wave.v[1] = lanes([a for a, _, _, _ in RELATIONS])
    wave.v[2] = lanes([b for _, b, _, _ in RELATIONS])
    wave.s[VCC_LO] = preload
    for number in range(40, 60):
        wave.s[number] = preload
    wave.s[EXEC_LO] = exec_mask
    return rdna2_exec.run(program(*words), wave)


@pytest.mark.parametrize("name", NAMES)
def test_vopc_compares_the_low_halves_into_vcc(name):
    assert run([vopc(name, V + 1, 2)]).vcc == expected_mask(name)


@pytest.mark.parametrize("name", NAMES)
def test_vop3_writes_the_sgpr_and_clears_inactive_lanes(name):
    exec_mask = 0x0F0F_F0F3
    wave = run([*vop3(name, 40, V + 1, V + 2)], exec_mask=exec_mask)
    assert wave.s[40] == expected_mask(name, exec_mask=exec_mask)


@pytest.mark.parametrize("name", NAMES)
def test_cmpx_writes_exec_only(name):
    exec_mask = 0xFFFF_00FF
    cmpx = name.replace("v_cmp_", "v_cmpx_")
    vopc_wave = run([vopc(cmpx, V + 1, 2)], exec_mask=exec_mask, preload=0x1234_5678)
    vop3_wave = run([*vop3(cmpx, EXEC_LO, V + 1, V + 2)], exec_mask=exec_mask, preload=0x1234_5678)
    expected = expected_mask(name, exec_mask=exec_mask)
    assert vopc_wave.exec == vop3_wave.exec == expected
    assert vopc_wave.vcc == vop3_wave.vcc == 0x1234_5678


def test_inactive_vcc_bits_are_cleared():
    wave = run([vopc("v_cmp_eq_u16", V + 1, 1)], exec_mask=0x0000_FFFF)
    assert wave.vcc == 0x0000_FFFF


def test_inline_floats_are_the_low_half_of_their_float32_pattern():
    wave = rdna2_exec.run(program(
        vop1("v_mov_b32", 1, LITERAL), 0x1234_3800,                 # f16 0.5 in the low half
        vop1("v_mov_b32", 2, LITERAL), 0x3F00_0000,                 # f32 0.5: low half 0
        vop1("v_mov_b32", 3, LITERAL), 0xFFFF_F983,                 # low half of f32 1/(2*PI) = 0x3E22F983
        vopc("v_cmp_eq_u16", HALF, 1),                              # 0x0000 == 0x3800 : false
        *vop3("v_cmp_eq_u16", 40, V + 2, HALF),
        *vop3("v_cmp_eq_i16", 41, ONE_OVER_TWO_PI, V + 3),
        *vop3("v_cmp_gt_i16", 42, V + 3, 244),                      # -1661 > 2.0 (0x40000000 → 0) : false
        *vop3("v_cmp_lt_u16", 43, 247, V + 3)))                     # -4.0 (0xC0800000 → 0) < 0xF983
    assert wave.vcc == 0
    assert wave.s[40:44] == [ALL, ALL, 0, ALL]


def test_sdwa_inline_float_is_selected_from_its_float32_pattern():
    # As measured on gfx1036 for v_cvt_f16_u16_sdwa: the float32 pattern goes through src0_sel.
    wave = rdna2_exec.run(program(
        vop1("v_mov_b32", 2, LITERAL), 0x3F00_0000,
        vopc("v_cmp_eq_u16", SDWA, 2), sdwa_word(HALF, src0_scalar=1, src0_sel=WORD_1, src1_sel=WORD_1),
        vopc("v_cmp_eq_u16", SDWA, 2), with_sdst(sdwa_word(HALF, src0_scalar=1, src1_sel=WORD_1), 40)))
    assert wave.vcc == ALL                                           # 0x3F00 == 0x3F00
    assert wave.s[40] == 0                                           # 0x0000 != 0x3F00


def test_integer_constants_and_literals_use_their_low_half():
    wave = rdna2_exec.run(program(
        vop1("v_mov_b32", 1, LITERAL), 0x0000_FFFF,
        vop1("v_mov_b32", 2, LITERAL), 0x7FFF_8000,
        vopc("v_cmp_eq_i16", MINUS_ONE, 1),                          # -1 = 0xFFFFFFFF, low half 0xFFFF
        *vop3("v_cmp_lt_i16", 40, V + 2, const(64)),                 # -32768 < 64
        *vop3("v_cmp_gt_u16", 41, V + 2, const(64)),                 # 0x8000 > 64
        vopc("v_cmp_eq_u16", LITERAL, 2), 0x1234_8000))
    assert wave.vcc == ALL
    assert wave.s[40:42] == [ALL, ALL]


def test_sdwa_selects_the_compared_halves():
    # v1 = 0x8000_0001: WORD_1 = -32768 (i16), WORD_0 = 1 ; v2 = 0x0002_FFFF.
    wave = rdna2_exec.run(program(
        vop1("v_mov_b32", 1, LITERAL), 0x8000_0001, vop1("v_mov_b32", 2, LITERAL), 0x0002_FFFF,
        vopc("v_cmp_lt_i16", SDWA, 2), sdwa_word(1, src0_sel=WORD_1, src1_sel=WORD_0),     # -32768 < -1
        *vop3("v_cmp_lt_i16", 40, V + 1, V + 2),                                         # 1 < -1 : false
        vopc("v_cmp_gt_u16", SDWA, 2), with_sdst(sdwa_word(1, src0_sel=WORD_1, src1_sel=WORD_1), 41)))
    assert wave.vcc == ALL and wave.s[40] == 0 and wave.s[41] == ALL                     # 0x8000 > 2


def test_sdwa_operand_types_are_integers():
    # The shared operand_float_flags (#215) reads i16/u16 sources as integers: SDWA applies sext, not neg/abs.
    for name in rdna2_exec_cmp16.COMPARE16:
        assert rdna2_exec_ext.operand_float_flags(name) == (False, False), name


@pytest.mark.parametrize("words, name, message", [
    (vop3("v_cmp_lt_i16", 40, V + 1, V + 2, sdst=0x08), "v_cmp_lt_i16", "op_sel"),          # op_sel[0]
    (vop3("v_cmpx_ge_u16", EXEC_LO, V + 1, V + 2, sdst=0x10), "v_cmpx_ge_u16", "op_sel"),   # op_sel[1]
])
def test_unverified_forms_are_refused_by_name(words, name, message):
    code = program(*words)
    with pytest.raises(rdna2_exec.Untranslated, match=f"(?=.*{name})(?=.*{message})"):
        rdna2_exec.run(code)
    with pytest.raises(rdna2_hlsl.Untranslatable, match=f"(?=.*{name})(?=.*{message})"):
        rdna2_hlsl.translate(code)


# --- GPU parity -----------------------------------------------------------------------------------------

def parity_memory(count=32):
    """v1 and v2 inputs: RELATIONS first, then boundary low halves under pseudo-random high halves
    (one lane in four compares equal low halves)."""
    edges = (0x0000, 0x0001, 0x7FFE, 0x7FFF, 0x8000, 0x8001, 0xFFFE, 0xFFFF)
    first, second = [a for a, _, _, _ in RELATIONS], [b for _, b, _, _ in RELATIONS]
    for lane in range(len(RELATIONS), count):
        low = edges[lane % len(edges)]
        other = low if lane % 4 == 0 else edges[(lane * 3 + 1) % len(edges)]
        first.append((lane * 2654435761) & HIGH_HALF | low)
        second.append((lane * 40503 << 13) & HIGH_HALF | other)
    image = bytearray(INPUT + 8 * count)
    struct.pack_into(f"<{2 * count}I", image, INPUT, *first, *second)
    return bytes(image)


def compares():
    """Every 16-bit compare in VOPC and VOP3 (masks in s40-s87), under a partial EXEC, plus SDWA forms."""
    words, sgpr = [], 40
    for name in NAMES:
        for prefix in ("v_cmp_", "v_cmpx_"):
            opcode = name.replace("v_cmp_", prefix)
            for encoding in ("VOPC", "VOP3"):
                words += [sop1("s_mov_b32", EXEC_LO, LITERAL), 0x0F0F_F0F3]
                target = EXEC_LO if prefix == "v_cmpx_" else sgpr
                words += [vopc(opcode, V + 1, 2)] if encoding == "VOPC" else [*vop3(opcode, target, V + 1, V + 2)]
                source = EXEC_LO if prefix == "v_cmpx_" else VCC_LO if encoding == "VOPC" else None
                if source is not None:
                    words.append(sop1("s_mov_b32", sgpr, source))
                sgpr += 1
    return words, sgpr


def sdwa_compares(first_sgpr):
    words = [sop1("s_mov_b32", EXEC_LO, LITERAL), ALL,
             vopc("v_cmp_lt_i16", SDWA, 2), sdwa_word(1, src0_sel=WORD_1, src1_sel=WORD_0),
             sop1("s_mov_b32", first_sgpr, VCC_LO),
             vopc("v_cmp_ge_u16", SDWA, 2), with_sdst(sdwa_word(1, src0_sel=WORD_0, src1_sel=WORD_1), first_sgpr + 1),
             vopc("v_cmp_ne_i16", SDWA, 2), sdwa_word(1, src0_sel=BYTE_0, src0_sext=1, src1_sel=BYTE_0, src1_sext=1),
             sop1("s_mov_b32", first_sgpr + 2, VCC_LO)]
    return words, first_sgpr + 3


def parity_program():
    loads = [*global_("global_load_dword", addr=31, vdst=1, saddr=0),
             *global_("global_load_dword", addr=31, vdst=2, saddr=0, offset=128)]
    words, end = compares()
    sdwa, end = sdwa_compares(end)
    stores = [sop1("s_mov_b32", EXEC_LO, LITERAL), ALL]
    return with_lane_offset(*loads, *words, *sdwa, *stores, *store_masks(range(40, end)))


def store_masks(sgprs):
    """One 128-byte slot per mask from OUTPUT on; v31 moves on every 16 slots (12-bit signed offset)."""
    words = []
    for slot, sgpr in enumerate(sgprs):
        if slot and slot % SLOTS_PER_BASE == 0:
            words += [vop2("v_add_nc_u32", 31, LITERAL, 31), SLOTS_PER_BASE * 128]
        words += [vop1("v_mov_b32", 3, sgpr), *store_lane(3, slot % SLOTS_PER_BASE)]
    return words


def test_parity_program_translates_every_compare():
    hlsl = rdna2_hlsl.translate(parity_program())
    assert hlsl.count("asint((") >= 24 * 2 and hlsl.count("& 0xFFFFu)") >= 24 * 2


@gpu
def test_16_bit_compares_match_interpreter_on_gpu():
    expected, actual = compare(parity_program(), memory=parity_memory())
    assert actual == expected


@gpu
def test_inline_floats_and_literals_match_interpreter_on_gpu():
    words = [*global_("global_load_dword", addr=31, vdst=1, saddr=0),
             vopc("v_cmp_eq_u16", HALF, 1), sop1("s_mov_b32", 40, VCC_LO),
             *vop3("v_cmp_le_i16", 41, V + 1, ONE_OVER_TWO_PI),
             *vop3("v_cmpx_gt_u16", EXEC_LO, 245, V + 1), sop1("s_mov_b32", 42, EXEC_LO),
             sop1("s_mov_b32", EXEC_LO, LITERAL), ALL,
             vopc("v_cmp_ne_i16", LITERAL, 1), 0xFFFF_3800, sop1("s_mov_b32", 43, VCC_LO),
             vopc("v_cmp_eq_u16", SDWA, 1), with_sdst(sdwa_word(HALF, src0_scalar=1, src0_sel=WORD_1), 44),
             vopc("v_cmp_lt_i16", SDWA, 1), with_sdst(sdwa_word(ONE_OVER_TWO_PI, src0_scalar=1), 45)]
    stores = store_masks(range(40, 46))
    values = [0x1234_3800, 0x3F00_0000, 0x0000_3118, 0xFFFF_3119, 0x0000_C000, 0x0000_BFFF, 0x0000_F983,
              0xABCD_0000] * 4
    memory = bytearray(INPUT + 128)
    struct.pack_into("<32I", memory, INPUT, *values[:32])
    expected, actual = compare(with_lane_offset(*words, *stores), memory=bytes(memory))
    assert actual == expected


@gpu
def test_wave64_compares_match_interpreter_on_gpu():
    exec_lo, exec_hi = 0xF0F0_FFF3, 0x0FF0_0FF1
    set_exec = [sop1("s_mov_b32", EXEC_LO, LITERAL), exec_lo, sop1("s_mov_b32", EXEC_LO + 1, LITERAL), exec_hi]
    words = [*global_("global_load_dword", addr=31, vdst=1, saddr=0),
             *global_("global_load_dword", addr=31, vdst=2, saddr=0, offset=wave64.SLOT),
             *set_exec,
             vopc("v_cmp_lt_i16", V + 1, 2), sop1("s_mov_b32", 40, VCC_LO), sop1("s_mov_b32", 41, VCC_LO + 1),
             *vop3("v_cmp_ge_u16", 42, V + 1, V + 2),
             *vop3("v_cmpx_ne_i16", EXEC_LO, V + 1, V + 2),
             sop1("s_mov_b32", 44, EXEC_LO), sop1("s_mov_b32", 45, EXEC_LO + 1), *set_exec,
             vopc("v_cmpx_gt_u16", V + 1, 2), sop1("s_mov_b32", 46, EXEC_LO), sop1("s_mov_b32", 47, EXEC_LO + 1),
             sop1("s_mov_b32", EXEC_LO, MINUS_ONE), sop1("s_mov_b32", EXEC_LO + 1, MINUS_ONE)]
    stores = [word for slot, sgpr in enumerate(range(40, 48))
              for word in (vop1("v_mov_b32", 3, sgpr), *wave64.store_lane(3, slot))]
    expected, actual = wave64.compare(wave64.with_lane_offset(*words, *stores), memory=parity_memory(64))
    assert actual == expected
    lows = [struct.unpack_from("<I", expected, OUTPUT + slot * wave64.SLOT)[0] for slot in range(0, 8, 2)]
    assert all(lows) and lows[2] & ~exec_lo == 0            # cmpx: only lanes active before it
