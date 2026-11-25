"""16-bit integer compares of the RDNA 2 reference interpreter: v_cmp[x]_{lt,eq,le,gt,ne,ge}_{i16,u16}.

They compare S0[15:0] with S1[15:0], signed (i16) or unsigned (u16); the high halves are ignored. Like the
32-bit compares, the mask is written whole: bits of inactive lanes are cleared. VOPC writes VCC (v_cmp) or
EXEC (v_cmpx), VOP3 an SGPR (v_cmp) or EXEC (v_cmpx). There are no f/t forms for these types on RDNA 2.

Sources:
- AMD, "RDNA 2" Instruction Set Architecture, November 2020, table 61 "VOPC Compare Opcodes"
  (p. 156-159: 137 V_CMP_LT_I16 to 190 V_CMPX_GE_U16, "D[threadId] = (S0 < S1)"), section 3.9 "Vector
  Compares: VCC and VCCZ" (p. 19-20, "VCC is fully written"). Table 60's hex ranges (0xA0-0xBF) do not
  match table 61 or LLVM; the opcodes come from table 61 and LLVM (rdna2_ops.py, 0x89-0xBE).
- Inline float constants (codes 240-248) give their float32 bit pattern, of which a 16-bit compare reads
  bits 15:0 (0.5 = 0x3F000000 compares as 0x0000, 1/(2*PI) = 0x3E22F983 as 0xF983), not the
  half-precision value. LLVM 20.1.8, SIInstrInfo::isInlineConstant (OPERAND_REG_*_INT16): "in the case
  of 16-bit integer operations, the 'floating point' values appear to not work. It seems read the low
  16-bits of 32-bit immediates" (bugzilla 46302), although its disassembler prints the f16 value.
  Measured on gfx1036 for other 16-bit integer sources: v_cvt_f16_u16_sdwa d, 1.0 src0_sel:WORD_1
  converts 0x3F80, and an inline float in S2 of v_mad_i32_i16 adds its float32 pattern (1.0 adds
  0x3F80): ubiquitarianism/leonida docs/design.md, commit 9f9a7c5f30f8559c52675780cf84004571b8d6e5,
  rows "SDWA select on an f16 inline constant" (line 331) and "v_mad_u32_u16 / v_mad_i32_i16 op_sel on
  S2 and D" (line 359). The compares themselves are not measured: requested as PortBridge issue #83
  (https://github.com/akimfomin/PortBridge/issues/83). In SDWA, the float32 pattern then goes through
  src_sel like a register, as measured for v_cvt_f16_u16_sdwa.
- Same document, section 6.2.5 "OPSEL Field Restrictions" (p. 44): OPSEL is "not for VOP1, VOP2 or VOPC
  instructions using the VOP3 encoding (these should use SDWA instead)": a VOP3 compare with op_sel set
  is refused by name instead of guessing which half the hardware reads.
"""
HALF_MASK, HALF_SIGN = 0xFFFF, 0x8000
TESTS = {"lt": lambda x, y: x < y, "eq": lambda x, y: x == y, "le": lambda x, y: x <= y,
         "gt": lambda x, y: x > y, "ne": lambda x, y: x != y, "ge": lambda x, y: x >= y}


def unsigned16(value):
    return value & HALF_MASK


def signed16(value):
    value &= HALF_MASK
    return value - (HALF_MASK + 1) if value & HALF_SIGN else value


# {mnemonic: (test on the two 32-bit source values, writes EXEC instead of VCC/SDST)}
COMPARE16 = {}
for _prefix, _writes_exec in (("v_cmp", False), ("v_cmpx", True)):
    for _name, _test in TESTS.items():
        COMPARE16[f"{_prefix}_{_name}_i16"] = (lambda a, b, t=_test: t(signed16(a), signed16(b)), _writes_exec)
        COMPARE16[f"{_prefix}_{_name}_u16"] = (lambda a, b, t=_test: t(unsigned16(a), unsigned16(b)), _writes_exec)


def check_vop3(name, op_sel, error):
    """Refuse a VOP3 16-bit compare with op_sel set (ISA section 6.2.5); `error`: exception class to raise."""
    if name in COMPARE16 and op_sel:
        raise error(f"{name} with op_sel (VOP3 compare, 16-bit sources): not usable per the RDNA 2 ISA")
