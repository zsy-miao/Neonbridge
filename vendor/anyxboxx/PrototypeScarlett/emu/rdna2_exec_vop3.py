"""Exécution des instructions VOP3 (encodage 64 bits) par l'interpréteur de référence RDNA 2.

VOP3 porte trois sources, des modificateurs d'entrée (abs, neg) et de sortie (clamp, omod),
et réencode les instructions VOPC (opcodes 0x000-0x0FF), VOP2 (0x100-0x13F) et VOP1
(0x180-0x1FF). Les comparaisons y écrivent un masque dans un registre scalaire au lieu de VCC.
Sémantique d'après la documentation publique d'AMD (« RDNA 2 Instruction Set Architecture »).
"""
import math

from emu import (rdna2_exec_cmp16, rdna2_exec_cmp64, rdna2_exec_cmpf16, rdna2_exec_div, rdna2_exec_f16,
                 rdna2_exec_movrel, rdna2_exec_sad, rdna2_exec_wide)
from emu.rdna2_exec_ops import (MASK32, SIXTEEN_BIT_RESULTS, VECTOR, VECTOR_COMPARE, VECTOR_NO_OPERATION,
                                _float_op, _fma, _legacy_fma, _mul_add, bits_float, float_bits, ldexp, signed)

SIGN = 0x8000_0000
EXEC_LO = 126
OUTPUT_SCALE = {1: 2.0, 2: 4.0, 3: 0.5}
COMPARE_END, VOP2_START, VOP2_END, VOP1_START, VOP1_END = 0x100, 0x100, 0x140, 0x180, 0x200
FLOAT_TO_INT = frozenset({"v_cvt_u32_f32", "v_cvt_i32_f32", "v_cvt_flr_i32_f32", "v_cvt_rpi_i32_f32",
                          "v_frexp_exp_i32_f32"})


def float_result(name):
    """Résultat flottant 32 bits : clamp et omod s'appliquent."""
    return "_f32" in name and name not in FLOAT_TO_INT and not name.startswith(("v_cvt_u32", "v_cvt_i32"))


def float_inputs(name):
    """Sources flottantes : abs et neg s'appliquent."""
    return float_result(name) or name.startswith("v_cvt_") or name in FLOAT_TO_INT


def _median(a, b, c):
    return sorted((a, b, c))[1]


def _float3(function):
    operation = _float_op(function)
    return lambda a, b, c: operation(bits_float(a), bits_float(b), bits_float(c))


def _bitfield(value, offset, width, is_signed=False):
    offset, width = offset & 31, width & 31
    field = (value >> offset) & ((1 << width) - 1) if width else 0
    return signed(field, width) if is_signed and width else field


def _perm_b32(src0, src1, src2):
    out = 0
    for shift in (0, 8, 16, 24):
        sel = (src2 >> shift) & 0xFF
        if sel < 4:
            byte = (src1 >> (sel * 8)) & 0xFF
        elif sel < 8:
            byte = (src0 >> ((sel - 4) * 8)) & 0xFF
        elif sel == 8:
            byte = 0xFF if src1 & 0x0000_8000 else 0x00
        elif sel == 9:
            byte = 0xFF if src1 & 0x8000_0000 else 0x00
        elif sel == 10:
            byte = 0xFF if src0 & 0x0000_8000 else 0x00
        elif sel == 11:
            byte = 0xFF if src0 & 0x8000_0000 else 0x00
        elif sel == 12:
            byte = 0x00
        else:
            byte = 0xFF
        out |= byte << shift
    return out


def _bytes(value):
    return [(value >> shift) & 0xFF for shift in (0, 8, 16, 24)]


def _sad_u8(a, b):
    """Somme des écarts absolus des quatre octets."""
    return sum(abs(x - y) for x, y in zip(_bytes(a), _bytes(b)))


def _lerp_u8(a, b, c):
    """Moyenne arrondie octet par octet : (a + b + bit d'arrondi de c) >> 1, bit 0 de chaque octet de c."""
    return sum(((x + y + (z & 1)) >> 1) << (8 * i) for i, (x, y, z) in enumerate(zip(_bytes(a), _bytes(b), _bytes(c))))


def _cube(x, y, z):
    """(face, sc, tc, 2 × axe majeur) d'une direction, comme les v_cube* (repli de LLVM, AMDGPU)."""
    if abs(z) >= abs(x) and abs(z) >= abs(y):
        return (5.0 if z < 0 else 4.0), (-x if z < 0 else x), -y, 2.0 * z
    if abs(y) >= abs(x):
        return (3.0 if y < 0 else 2.0), x, (-z if y < 0 else z), 2.0 * y
    return (1.0 if x < 0 else 0.0), (z if x < 0 else -z), -y, 2.0 * x


# Instructions propres à VOP3 : fonction(a, b, c) des trois sources.
VECTOR3 = {
    "v_fma_f32": _float3(_fma),
    "v_mad_f32": _float3(_mul_add),
    "v_fma_legacy_f32": _float3(_legacy_fma),
    "v_mad_u32_u24": lambda a, b, c: (a & 0xFFFFFF) * (b & 0xFFFFFF) + c,
    "v_mad_i32_i24": lambda a, b, c: signed(a, 24) * signed(b, 24) + signed(c),
    "v_bfe_u32": lambda a, b, c: _bitfield(a, b, c),
    "v_bfe_i32": lambda a, b, c: _bitfield(a, b, c, is_signed=True),
    "v_bfi_b32": lambda a, b, c: (a & b) | (~a & c),
    "v_alignbit_b32": lambda a, b, c: ((a << 32 | b) >> (c & 31)) & MASK32,
    "v_alignbyte_b32": lambda a, b, c: ((a << 32 | b) >> ((c & 3) * 8)) & MASK32,
    "v_min3_f32": _float3(lambda x, y, z: min(x, y, z)),
    "v_max3_f32": _float3(lambda x, y, z: max(x, y, z)),
    "v_med3_f32": _float3(_median),
    "v_min3_u32": min, "v_max3_u32": max, "v_med3_u32": _median,
    "v_min3_i32": lambda a, b, c: min(signed(a), signed(b), signed(c)),
    "v_max3_i32": lambda a, b, c: max(signed(a), signed(b), signed(c)),
    "v_med3_i32": lambda a, b, c: _median(signed(a), signed(b), signed(c)),
    "v_lshl_add_u32": lambda a, b, c: (a << (b & 31)) + c,
    "v_add_lshl_u32": lambda a, b, c: (a + b) << (c & 31),
    "v_add3_u32": lambda a, b, c: a + b + c,
    "v_lshl_or_b32": lambda a, b, c: (a << (b & 31)) | c,
    "v_and_or_b32": lambda a, b, c: (a & b) | c,
    "v_or3_b32": lambda a, b, c: a | b | c,
    "v_xor3_b32": lambda a, b, c: a ^ b ^ c,
    "v_xad_u32": lambda a, b, c: (a ^ b) + c,
    "v_sad_u32": lambda a, b, c: abs(a - b) + c,
    "v_mul_lo_u32": lambda a, b, c: a * b,
    "v_mul_hi_u32": lambda a, b, c: (a * b) >> 32,
    "v_mul_hi_i32": lambda a, b, c: (signed(a) * signed(b)) >> 32,
    "v_bcnt_u32_b32": lambda a, b, c: bin(a).count("1") + b,
    "v_bfm_b32": lambda a, b, c: ((1 << (a & 31)) - 1) << (b & 31),
    "v_ldexp_f32": lambda a, b, c: _float_op(lambda x: ldexp(x, signed(b)))(bits_float(a)),
    "v_perm_b32": _perm_b32,
    "v_mul_lo_i32": lambda a, b, c: a * b,
    "v_sad_u8": lambda a, b, c: _sad_u8(a, b) + c,
    "v_sad_hi_u8": lambda a, b, c: (_sad_u8(a, b) << 16) + c,
    "v_lerp_u8": _lerp_u8,
    "v_cubeid_f32": _float3(lambda x, y, z: _cube(x, y, z)[0]),
    "v_cubesc_f32": _float3(lambda x, y, z: _cube(x, y, z)[1]),
    "v_cubetc_f32": _float3(lambda x, y, z: _cube(x, y, z)[2]),
    "v_cubema_f32": _float3(lambda x, y, z: _cube(x, y, z)[3]),
}
# S0 × S1 (32 bits) + S2 (64 bits) → D sur deux VGPR ; retenue (bit 64) dans SDST pour u64.
MAD64 = frozenset({"v_mad_u64_u32", "v_mad_i64_i32"})
NULL_SGPR = 125
# Retenues (VOP3b) : fonction(a, b, retenue entrante) → (résultat, retenue sortante).
CARRY = {
    "v_add_co_u32": lambda a, b, k: (a + b, (a + b) >> 32 != 0),
    "v_sub_co_u32": lambda a, b, k: (a - b, a < b),
    "v_subrev_co_u32": lambda a, b, k: (b - a, b < a),
    "v_add_co_ci_u32": lambda a, b, k: (a + b + k, (a + b + k) >> 32 != 0),
    "v_sub_co_ci_u32": lambda a, b, k: (a - b - k, a < b + k),
    "v_subrev_co_ci_u32": lambda a, b, k: (b - a - k, b < a + k),
}
LANE_OPERATIONS = frozenset({"v_readlane_b32", "v_writelane_b32", "v_mbcnt_lo_u32_b32", "v_mbcnt_hi_u32_b32",
                             "v_readfirstlane_b32"})
EXECUTABLE = frozenset(set(VECTOR) - {"v_readfirstlane_b32"} | set(VECTOR_COMPARE) | set(VECTOR3) | set(CARRY)
                       | LANE_OPERATIONS | set(rdna2_exec_cmp64.COMPARE64) | MAD64 | VECTOR_NO_OPERATION
                       | set(rdna2_exec_wide.OPERATIONS) | set(rdna2_exec_div.OPERATIONS)
                       | rdna2_exec_movrel.VECTOR | set(rdna2_exec_sad.OPERATIONS)
                       | set(rdna2_exec_f16.OPERATIONS) | set(rdna2_exec_cmpf16.COMPAREF16))


class Fields:
    """Champs d'une instruction VOP3 lue comme un entier 64 bits."""

    def __init__(self, word):
        self.vdst = word & 0xFF                 # VGPR, ou SGPR pour les comparaisons et readlane
        self.sdst = (word >> 8) & 0x7F          # VOP3b : destination de la retenue
        self.abs = (word >> 8) & 0x7
        self.op_sel = (word >> 11) & 0xF        # moitiés de 16 bits : sources 0-2, puis destination
        self.clamp = (word >> 15) & 1
        self.sources = ((word >> 32) & 0x1FF, (word >> 41) & 0x1FF, (word >> 50) & 0x1FF)
        self.omod = (word >> 59) & 0x3
        self.neg = (word >> 61) & 0x7


def _inputs(wave, fields, lane, literal, modifiers):
    values = []
    for index, code in enumerate(fields.sources):
        value = wave.read_lane(code, lane, literal)
        if modifiers and fields.abs >> index & 1:
            value &= ~SIGN & MASK32
        if modifiers and fields.neg >> index & 1:
            value ^= SIGN
        values.append(value)
    return values


def _output(fields, value, is_float):
    if not is_float or not (fields.clamp or fields.omod):
        return value & MASK32
    number = bits_float(value) * OUTPUT_SCALE.get(fields.omod, 1.0)
    if fields.clamp and not math.isnan(number):
        number = min(max(number, 0.0), 1.0)
    return float_bits(number)


def execute(wave, name, opcode, word, literal):
    """Exécute une instruction VOP3 dont le nom est dans EXECUTABLE."""
    fields = Fields(word)
    if name in VECTOR_NO_OPERATION:
        return None
    if name in rdna2_exec_f16.OPERATIONS:
        return rdna2_exec_f16.execute_vop3(wave, name, fields, literal)
    if name in LANE_OPERATIONS:
        return _lane_operation(wave, name, fields, literal)
    if name in MAD64:
        return _mad64(wave, name, fields, literal)
    if name in rdna2_exec_wide.OPERATIONS:
        return rdna2_exec_wide.execute(wave, name, fields.sources, fields.vdst, literal, fields.abs, fields.neg,
                                       fields.clamp, fields.omod)
    if name in rdna2_exec_cmpf16.COMPAREF16:
        return rdna2_exec_cmpf16.execute_vop3(wave, name, fields, literal)
    if name in rdna2_exec_cmp64.COMPARE64:
        return rdna2_exec_cmp64.execute(wave, name, fields.sources, literal, fields.vdst, fields.abs, fields.neg)
    if name in rdna2_exec_div.OPERATIONS:
        return rdna2_exec_div.execute(wave, name, fields, literal)
    if name in rdna2_exec_sad.OPERATIONS:
        return rdna2_exec_sad.execute(wave, name, fields, literal)
    if name in rdna2_exec_movrel.VECTOR:
        if fields.abs or fields.neg or fields.clamp or fields.omod:
            from emu.rdna2_exec import Untranslated
            raise Untranslated(f"{name} avec modificateurs VOP3")
        return rdna2_exec_movrel.vector(wave, name, fields.vdst, fields.sources[0], literal)
    if opcode < COMPARE_END:
        return _compare(wave, name, fields, literal)
    if name in CARRY:
        return _carry(wave, name, fields, literal)
    if name in SIXTEEN_BIT_RESULTS and (fields.op_sel or fields.clamp):
        from emu.rdna2_exec import Untranslated
        raise Untranslated(f"{name} avec op_sel/clamp (VOP3, résultat 16 bits)")
    is_vop1 = VOP1_START <= opcode < VOP1_END
    is_vop2 = VOP2_START <= opcode < VOP2_END
    is_float = float_result(name)
    destination = wave.v[fields.vdst]
    for lane in wave.active():
        a, b, c = _inputs(wave, fields, lane, literal, float_inputs(name))
        if is_vop1 or is_vop2:
            condition = wave.read_mask(fields.sources[2], literal) >> lane & 1 if name == "v_cndmask_b32" else 0
            result = VECTOR[name](a, b if is_vop2 else 0, destination[lane], literal, condition)
        else:
            result = VECTOR3[name](a, b, c)
        destination[lane] = _output(fields, result, is_float)
    return None


def _compare(wave, name, fields, literal):
    from emu.rdna2_exec import Untranslated

    rdna2_exec_cmp16.check_vop3(name, fields.op_sel, Untranslated)
    test, writes_exec = VECTOR_COMPARE[name]
    mask = 0
    for lane in wave.active():
        a, b, _ = _inputs(wave, fields, lane, literal, name.endswith("_f32"))
        mask |= bool(test(a, b)) << lane
    wave.write_mask(EXEC_LO if writes_exec else fields.vdst, mask)


def _carry(wave, name, fields, literal):
    """VOP3b : retenue dans SDST, bits des lignes inactives à 0.

    La doc RDNA 2 (section 6.2, p. 43) dit seulement que la forme VOP3 écrit la retenue « to an
    arbitrary SGPR-pair » ; la règle « fully written, no partial mask updates » (section 3.9) est
    énoncée pour VCC. Mesuré sur RDNA 2 (gfx1036, Wave32) : v_add_co_ci_u32 en VOP3 met aussi à 0
    les bits des lignes inactives de son SGPR (mesure et portée citées dans rdna2_exec._carry).
    """
    carries = 0
    carry_in = wave.read_mask(fields.sources[2], literal) if name.endswith("_ci_u32") else 0
    for lane in wave.active():
        a, b, _ = _inputs(wave, fields, lane, literal, False)
        result, carry = CARRY[name](a, b, carry_in >> lane & 1)
        wave.v[fields.vdst][lane] = result & MASK32
        carries |= bool(carry) << lane
    wave.write_mask(fields.sdst, carries)


def _mad64(wave, name, fields, literal):
    is_signed = name == "v_mad_i64_i32"
    if is_signed and fields.sdst != NULL_SGPR:
        from emu.rdna2_exec import Untranslated
        raise Untranslated("v_mad_i64_i32 : retenue signée dans SDST non vérifiée")
    carries = 0
    for lane in wave.active():
        a, b = (wave.read_lane(code, lane, literal) for code in fields.sources[:2])
        c = rdna2_exec_cmp64.read64(wave, fields.sources[2], lane, False)
        if is_signed:
            total = signed(a) * signed(b) + signed(c, 64)
        else:
            total = a * b + c
        wave.v[fields.vdst][lane] = total & MASK32
        wave.v[fields.vdst + 1][lane] = total >> 32 & MASK32
        carries |= (not is_signed and total >> 64 != 0) << lane
    wave.write_mask(fields.sdst, carries)


def _lane_operation(wave, name, fields, literal):
    src0, src1, _ = fields.sources
    if name == "v_readfirstlane_b32":
        lanes = wave.active() or [0]
        wave.write_scalar(fields.vdst, wave.read_lane(src0, lanes[0], literal))
    elif name == "v_readlane_b32":                    # ligne : SSRC1[4:0] en wave32, [5:0] en wave64
        lane = wave.read_scalar(src1, literal) & (wave.lanes - 1)
        wave.write_scalar(fields.vdst, wave.read_lane(src0, lane, literal))
    elif name == "v_writelane_b32":
        lane = wave.read_scalar(src1, literal) & (wave.lanes - 1)
        wave.v[fields.vdst][lane] = wave.read_scalar(src0, literal)
    else:
        # ThreadMask = (1 << ligne) - 1 : mbcnt_lo compte S0 & ThreadMask[31:0], mbcnt_hi S0 & ThreadMask[63:32].
        for lane in wave.active():
            below = (1 << lane) - 1
            below = below & MASK32 if name == "v_mbcnt_lo_u32_b32" else below >> 32
            count = bin(wave.read_lane(src0, lane, literal) & below).count("1")
            wave.v[fields.vdst][lane] = (count + wave.read_lane(src1, lane, literal)) & MASK32
