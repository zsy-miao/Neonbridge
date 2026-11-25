// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text.RegularExpressions;

namespace FiveStar.Shaders.Translation.Core;

/// <summary>The branch instructions the translator follows and the resolved target in their text (<c>@T0x...</c>).</summary>
internal static partial class BranchTargets
{
    private static readonly HashSet<string> Branches =
    [
        "s_branch", "s_cbranch_scc0", "s_cbranch_scc1", "s_cbranch_vccz", "s_cbranch_vccnz", "s_cbranch_execz", "s_cbranch_execnz",
    ];

    /// <summary>True for a direct (static target) branch.</summary>
    public static bool IsBranch(string mnemonic)
    {
        return Branches.Contains(mnemonic);
    }

    /// <summary>The target address resolved by the disassembler.</summary>
    public static long Target(string text)
    {
        var match = TargetPattern().Match(text);

        if (!match.Success)
        {
            throw new UnsupportedProgramException($"branch without target: {text}");
        }

        // A target before the program start is a 64-bit wrapped value; it keeps its bits.
        return unchecked((long)ulong.Parse(match.Groups[1].Value, NumberStyles.HexNumber, CultureInfo.InvariantCulture));
    }

    /// <summary>The text that marks a branch to a target address.</summary>
    public static string Marker(long target)
    {
        return "@T" + Hlsl.Hex(target);
    }

    [GeneratedRegex(@"@T0x([0-9a-f]+)")]
    private static partial Regex TargetPattern();
}
