// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation.Core;

/// <summary>
/// SGPR numbers of the special scalar registers in the gfx10 operand encoding (RDNA ISA, "Scalar ALU operands"):
/// 104.. are not preloaded user data, VCC is 106/107, M0 124, EXEC 126/127.
/// </summary>
public static class SpecialRegisters
{
    /// <summary>The first SGPR number that is not an ordinary register.</summary>
    public const int FirstSpecial = 104;

    public const int VccLo = 106;

    public const int VccHi = 107;

    public const int M0 = 124;

    public const int ExecLo = 126;

    public const int ExecHi = 127;

    /// <summary>The EXEC mask register (low half).</summary>
    public static RegisterKey Exec => RegisterKey.Sgpr(ExecLo);

    /// <summary>The VCC lane mask register (low half).</summary>
    public static RegisterKey Vcc => RegisterKey.Sgpr(VccLo);

    /// <summary>The HLSL variable of a special SGPR, or null for an ordinary one.</summary>
    public static string? VariableName(int sgpr)
    {
        switch (sgpr)
        {
            case VccLo:
                return "s_vcc";
            case VccHi:
                return "s_vcch";
            case M0:
                return "s_m0";
            case ExecLo:
                return "s_exec";
            case ExecHi:
                return "s_exech";
            default:
                return null;
        }
    }

    /// <summary>The register named by an assembler alias (vcc, vcc_lo, m0, exec, ...) and whether it is a pair.</summary>
    public static (int Index, bool Pair)? FromAlias(string name)
    {
        switch (name)
        {
            case "vcc_lo":
                return (VccLo, false);
            case "vcc_hi":
                return (VccHi, false);
            case "vcc":
                return (VccLo, true);
            case "m0":
                return (M0, false);
            case "exec_lo":
                return (ExecLo, false);
            case "exec_hi":
                return (ExecHi, false);
            case "exec":
                return (ExecLo, true);
            default:
                return null;
        }
    }
}
