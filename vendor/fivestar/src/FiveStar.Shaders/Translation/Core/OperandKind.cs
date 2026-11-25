// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation.Core;

/// <summary>What an instruction operand names.</summary>
public enum OperandKind
{
    /// <summary>A constant (inline constant, literal, null or off).</summary>
    Immediate,

    /// <summary>An SGPR or SGPR range.</summary>
    Scalar,

    /// <summary>A VGPR or VGPR range.</summary>
    Vector,

    /// <summary>A value the translator computed (an HLSL uint expression), for example a DPP or SDWA source.</summary>
    Expression,
}
