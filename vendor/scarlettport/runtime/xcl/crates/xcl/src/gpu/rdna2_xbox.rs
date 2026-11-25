//! Instructions the Xbox Series GPU executes that the retail RDNA2 (gfx10.3) spec does
//! not list.
//!
//! Found in retail console shaders (1.3% of all dwords were undecodable before): the
//! console GPU kept the non-fused multiply-add family of gfx10.1 (RDNA1), which
//! gfx10.3 removed. Opcodes and operand layouts follow gfx10.1 (LLVM `VOP2Instructions.td`
//! / `VOP3Instructions.td`); they occupy opcodes that are unassigned in gfx10.3, so the
//! spec tables always take priority.
//!
//! Note: opcode VOP2 0x06 / VOP3 0x140 are V_MAC_LEGACY_F32 / V_MAD_LEGACY_F32 on gfx10.1
//! but V_FMAC_LEGACY_F32 / V_FMA_LEGACY_F32 (fused) on gfx10.3; the decoder names them
//! after the spec, the recompiler must decide the rounding behaviour.

use super::rdna2::{Inst, InstEnc, Operand};

/// encoding indices in `rdna2_spec::ENCODINGS`
const ENC_VOP2: u16 = 8;
const ENC_VOP3: u16 = 10;

const fn op(field: &'static str, ty: &'static str, output: bool) -> Operand {
    Operand { field: Some(field), ty, size: 32, input: !output, output, implicit: false }
}

const VOP2_VDST_SRC0_VSRC1: &[Operand] = &[op("VDST", "OPR_VGPR", true), op("SRC0", "OPR_SRC", false), op("VSRC1", "OPR_VGPR", false)];
const VOP3_VDST_SRC0_SRC1: &[Operand] = &[op("VDST", "OPR_VGPR", true), op("SRC0", "OPR_SRC", false), op("SRC1", "OPR_SRC_NOLDS", false)];
const VOP3_VDST_SRC012: &[Operand] = &[
    op("VDST", "OPR_VGPR", true),
    op("SRC0", "OPR_SRC", false),
    op("SRC1", "OPR_SRC_NOLDS", false),
    op("SRC2", "OPR_SRC_NOLDS", false),
];
const MADMK: &[Operand] =
    &[op("VDST", "OPR_VGPR", true), op("SRC0", "OPR_SRC", false), op("LITERAL", "OPR_SIMM32", false), op("VSRC1", "OPR_VGPR", false)];
const MADAK: &[Operand] =
    &[op("VDST", "OPR_VGPR", true), op("SRC0", "OPR_SRC", false), op("VSRC1", "OPR_VGPR", false), op("LITERAL", "OPR_SIMM32", false)];

pub static CONSOLE_INSTRUCTIONS: &[Inst] = &[
    // D = S0 * S1 + D (not fused)
    Inst {
        name: "V_MAC_F32",
        flags: 0,
        always_literal: false,
        encs: &[
            InstEnc { encoding: ENC_VOP2, opcode: 0x1f, operands: VOP2_VDST_SRC0_VSRC1 },
            InstEnc { encoding: ENC_VOP3, opcode: 0x11f, operands: VOP3_VDST_SRC0_SRC1 },
        ],
    },
    // D = S0 * K + S1
    Inst { name: "V_MADMK_F32", flags: 0, always_literal: true, encs: &[InstEnc { encoding: ENC_VOP2, opcode: 0x20, operands: MADMK }] },
    // D = S0 * S1 + K
    Inst { name: "V_MADAK_F32", flags: 0, always_literal: true, encs: &[InstEnc { encoding: ENC_VOP2, opcode: 0x21, operands: MADAK }] },
    // D = S0 * S1 + S2 (not fused)
    Inst { name: "V_MAD_F32", flags: 0, always_literal: false, encs: &[InstEnc { encoding: ENC_VOP3, opcode: 0x141, operands: VOP3_VDST_SRC012 }] },
];
