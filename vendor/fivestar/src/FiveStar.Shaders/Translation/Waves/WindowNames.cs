// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Frozen;
using System.Text.RegularExpressions;

namespace FiveStar.Shaders.Translation.Waves;

/// <summary>What the names of translated code say: scalar registers, lane masks, and wave functions by their effect.</summary>
internal static partial class WindowNames
{
    public const string Active = "active";

    /// <summary>Functions whose result depends on which lanes the hardware wave holds.</summary>
    private static readonly FrozenSet<string> WindowFunctions =
        new[] { "vw_any", "vw_count", "vw_ballot", "vw_origin", "readfirstlane" }.ToFrozenSet(StringComparer.Ordinal);

    /// <summary>Functions that give each lane its own value (still the console's).</summary>
    private static readonly FrozenSet<string> LaneFunctions =
        new[] { "vw_lane", "vw_read", "lane_bit", "quad_perm" }.ToFrozenSet(StringComparer.Ordinal);

    /// <summary>Lane masks: EXEC (<c>active</c>), SGPRs holding masks (<c>m_sN</c>) and the saved EXEC of a saveexec.</summary>
    public static bool IsMask(string name)
    {
        return name == Active || name == "_old" || name.StartsWith("m_", StringComparison.Ordinal);
    }

    /// <summary>One value per wave: SGPRs, SCC and the group id (and groupshared scalars).</summary>
    public static bool IsScalar(string name)
    {
        return name == "scc" || name == "in_gid" || ScalarPattern().IsMatch(name);
    }

    /// <summary>A block-local temporary (its kind follows from its value).</summary>
    public static bool IsTemporary(string name)
    {
        return name.StartsWith('_');
    }

    /// <summary>A function whose result depends on the hardware wave (any native wave intrinsic included).</summary>
    public static bool IsWindowFunction(string function)
    {
        return WindowFunctions.Contains(function) || function.StartsWith("Wave", StringComparison.Ordinal);
    }

    /// <summary>A function that gives each lane its own value.</summary>
    public static bool IsLaneFunction(string function)
    {
        return LaneFunctions.Contains(function) || function.StartsWith("Quad", StringComparison.Ordinal);
    }

    /// <summary>A per-lane input that differs between lanes.</summary>
    public static bool IsLaneInput(string name)
    {
        return name is "in_gi" or "g_vw_lane";
    }

    [GeneratedRegex(@"^(?:s\d+|s_\w+|g_ms_\w+)$")]
    private static partial Regex ScalarPattern();
}
