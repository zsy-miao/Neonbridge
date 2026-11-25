// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Frozen;
using System.Text.RegularExpressions;

namespace FiveStar.Shaders.Translation.Core;

/// <summary>
/// Whether an HLSL value can be computed on every lane, active or not: it only calls arithmetic and conversion
/// functions, so it reads no memory and no other lane. Such a VGPR write becomes <c>v = select(active, value, v)</c>,
/// straight-line code, instead of a branch per instruction; dxc's optimizer (jump threading in particular) slows down
/// sharply with tens of thousands of those branches.
/// </summary>
internal static partial class PureExpression
{
    // HLSL intrinsics and prelude helpers without memory access or cross-lane reads.
    private static readonly FrozenSet<string> PureFunctions = new[]
    {
        "abs", "alignbit", "and", "asfloat", "asint", "asuint", "bfe_i", "bfe_u", "ceil", "clamp", "cmp_class", "cos",
        "countbits", "cubeid", "cubema", "cubesc", "cubetc", "exp2", "f16tof32", "f32tof16", "f32tof16_rne",
        "f32tof16_rtz", "firstbithigh", "firstbithigh_amd", "firstbitlow", "flbit_signed", "floor", "fma32", "frac",
        "ldexp_exact", "log2", "mad", "mad_legacy", "max", "med3", "med3i", "med3u", "min", "mul_hi_u", "mul_legacy",
        "or", "perm_b32", "rcp", "reversebits", "round", "round_ne", "rsqrt", "saturate", "select", "sext24", "sin",
        "sqrt", "trunc",
    }.ToFrozenSet(StringComparer.Ordinal);

    /// <summary>True when every call in <paramref name="value"/> is a pure function and nothing is indexed.</summary>
    public static bool IsPure(string value)
    {
        if (value.Contains('[', StringComparison.Ordinal) || MethodCallPattern().IsMatch(value))
        {
            return false;
        }

        foreach (Match call in CallPattern().Matches(value))
        {
            if (!PureFunctions.Contains(call.Groups[1].Value))
            {
                return false;
            }
        }

        return true;
    }

    [GeneratedRegex(@"\b([A-Za-z_]\w*)\s*\(")]
    private static partial Regex CallPattern();

    [GeneratedRegex(@"\.\w+\s*\(")]
    private static partial Regex MethodCallPattern();
}
