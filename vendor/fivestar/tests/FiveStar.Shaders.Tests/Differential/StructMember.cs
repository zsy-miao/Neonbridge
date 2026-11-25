// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Tests.Differential;

/// <summary>
/// One member of a struct of translated HLSL: <c>[modifiers] &lt;Scalar&gt;&lt;Width&gt; Name : Semantic;</c>, as written
/// (<see cref="Line"/>).
/// </summary>
public sealed record StructMember(string Line, string Scalar, int Width, string Name, string Semantic)
{
    /// <summary>An HLSL expression of the member's bits as uint (or uintN), half types widened to float first.</summary>
    public string Bits(string value)
    {
        var vector = Vector("float");

        switch (Scalar)
        {
            case "float":
            case "uint":
            case "int":
                return $"asuint({value})";
            case "half":
            case "min16float":
                return $"asuint(({vector}){value})";
            default:
                return $"({Vector("uint")}){value}";
        }
    }

    /// <summary>The bits as a uint4, zero-padded.</summary>
    public string Bits4(string value)
    {
        if (Width == 4)
        {
            return Bits(value);
        }

        return $"uint4({Bits(value)}{string.Concat(Enumerable.Repeat(", 0u", 4 - Width))})";
    }

    /// <summary>The member's type with another scalar, e.g. float3 -> uint3.</summary>
    public string Vector(string scalar)
    {
        if (Width == 1)
        {
            return scalar;
        }

        return scalar + Width.ToString(System.Globalization.CultureInfo.InvariantCulture);
    }
}
