// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>How an operand's register width and constants are read: integer and float constants print differently.</summary>
public enum OperandType
{
    /// <summary>No operand.</summary>
    None,

    /// <summary>32-bit integer or untyped bits.</summary>
    Word,

    /// <summary>32-bit float.</summary>
    Real,

    /// <summary>64-bit integer or untyped bits.</summary>
    WideInteger,

    /// <summary>64-bit float.</summary>
    WideReal,

    /// <summary>16-bit integer.</summary>
    HalfInteger,

    /// <summary>16-bit float.</summary>
    HalfReal,

    /// <summary>Two packed 16-bit integers.</summary>
    PackedInteger,

    /// <summary>Two packed 16-bit floats.</summary>
    PackedReal,

    /// <summary>128 bits (four dwords), as the v_mqsad_u32_u8 result.</summary>
    Quad,

    /// <summary>A lane mask: one SGPR in wave32, an SGPR pair in wave64.</summary>
    LaneMask,
}
