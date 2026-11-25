"""Half-precision compares of the RDNA 2 reference interpreter: v_cmp[x]_{f,lt,…,tru}_f16 and v_cmp[x]_class_f16.

S0[15:0] and S1[15:0] are binary16 values; the high halves are ignored. The 16 predicates are those of the
f32 compares (rdna2_exec_ops.FLOAT_TESTS): ordered ones are false on NaN, their n* negations true, -0 == +0,
denormals compared by value. v_cmp_class_f16 tests the IEEE class of S0 against the mask S1 (bit 0 sNaN,
1 qNaN, 2 -inf, 3 -normal, 4 -denormal, 5 -0, 6 +0, 7 +denormal, 8 +normal, 9 +inf). As for the other
compares, the mask is written whole (inactive lanes cleared): VOPC writes VCC (v_cmp) or EXEC (v_cmpx),
VOP3 its SGPR destination (v_cmp) or EXEC (v_cmpx).

Sources are read like those of the other f16 instructions (rdna2_exec_f16._source): a register gives its
low half, an inline float constant its binary16 encoding (1.0 = 0x3C00), an inline integer or a literal
its low 16 bits. VOP3 abs/neg act on bit 15 of a float source.

Sources:
- AMD, "RDNA 2" Instruction Set Architecture, November 2020, section 12.9 "VOPC Instructions" (p. 156-161):
  200 V_CMP_F_F16 to 239 V_CMP_TRU_F16 ("D[threadId] = (S0 < S1)" …), 216-255 the V_CMPX forms writing
  EXEC, 143/159 V_CMP(X)_CLASS_F16 "performed on S0.f16. Note that the S1 has a format of f16 since floating
  point literal constants are interpreted as 16 bit value for this opcode"; section 3.9 (p. 19-20, "VCC is
  fully written").
- Same document, section 6.2.5 "OPSEL Field Restrictions" (p. 44): OPSEL is not for VOPC instructions in
  the VOP3 encoding, so op_sel is refused by name, as for the 16-bit integer compares.

Refused by name (rule 3), not measured: VOP3 op_sel, clamp and omod; abs/neg on the class mask S1; the
SDWA and DPP forms (refused for every f16 instruction, see rdna2_exec_f16).
"""
from emu.rdna2_exec_cmp64 import float_class
from emu.rdna2_exec_f16 import EXTENSIONS, VGPR_FIRST, _modified, _source, bits_half
from emu.rdna2_exec_ops import FLOAT_TESTS

EXEC_LO, VCC_LO = 126, 106
HALF, MASK = "h", "i"                             # source kinds of rdna2_exec_f16 (mask: no abs/neg)


def _class(a, b):
    return b >> float_class(a, 16) & 1


def _tests():
    """{mnemonic: (test on the two binary16 patterns, writes EXEC, source kinds)}."""
    table = {}
    for prefix, writes_exec in (("v_cmp", False), ("v_cmpx", True)):
        for name, test in FLOAT_TESTS.items():
            table[f"{prefix}_{name}_f16"] = (lambda a, b, t=test: t(bits_half(a), bits_half(b)), writes_exec,
                                             (HALF, HALF))
        table[f"{prefix}_class_f16"] = (_class, writes_exec, (HALF, MASK))
    return table


COMPAREF16 = _tests()


def _refuse(message):
    from emu.rdna2_exec import Untranslated

    raise Untranslated(message)


def _write(wave, writes_exec, destination, mask):
    wave.write_mask(EXEC_LO if writes_exec else destination, mask)


def execute(wave, name, fields, literal):
    """VOPC: S0 is src0, S1 the VGPR vsrc1; the mask goes to VCC or EXEC."""
    if fields["src0"] in EXTENSIONS:
        _refuse(f"{name}: SDWA/DPP form (src0 = 0x{fields['src0']:X}) not supported for f16")
    test, writes_exec, _ = COMPAREF16[name]
    codes = (fields["src0"], VGPR_FIRST + fields["vsrc1"])
    mask = 0
    for lane in wave.active():
        a, b = (_source(wave, name, HALF, code, lane, literal) for code in codes)
        mask |= bool(test(a, b)) << lane
    _write(wave, writes_exec, VCC_LO, mask)


def _check_vop3(name, kinds, fields):
    if fields.op_sel:
        _refuse(f"{name} with op_sel (VOP3 compare, 16-bit sources): not usable per the RDNA 2 ISA")
    if fields.clamp or fields.omod:
        _refuse(f"{name} with clamp or omod (VOP3 f16 compare): not measured on RDNA 2")
    for index, kind in enumerate(kinds):
        if kind == MASK and (fields.abs | fields.neg) >> index & 1:
            _refuse(f"{name} with abs/neg on the class mask S1: not measured on RDNA 2")


def execute_vop3(wave, name, fields, literal):
    """VOP3: S0, S1 from the first two sources, abs/neg on bit 15; the mask goes to the SGPR vdst or EXEC."""
    test, writes_exec, kinds = COMPAREF16[name]
    _check_vop3(name, kinds, fields)
    mask = 0
    for lane in wave.active():
        values = [_modified(_source(wave, name, HALF, code, lane, literal), kind, fields, index)
                  for index, (kind, code) in enumerate(zip(kinds, fields.sources[:2]))]
        mask |= bool(test(*values)) << lane
    _write(wave, writes_exec, fields.vdst, mask)
