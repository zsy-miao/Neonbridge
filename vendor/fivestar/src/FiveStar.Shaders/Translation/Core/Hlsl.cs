// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Shaders.Translation.Core;

/// <summary>Formatting of HLSL literals.</summary>
internal static class Hlsl
{
    /// <summary>A lower-case hex literal: 0x0, 0x1f, ...</summary>
    public static string Hex(long value)
    {
        return "0x" + value.ToString("x", CultureInfo.InvariantCulture);
    }

    /// <summary>A lower-case hex uint literal: 0x0u, 0x1fu, ...</summary>
    public static string HexUint(long value)
    {
        return Hex(value) + "u";
    }

    /// <summary>A decimal literal.</summary>
    public static string Number(long value)
    {
        return value.ToString(CultureInfo.InvariantCulture);
    }

    /// <summary>A float literal as the generator writes scale factors: 2.0, 4.0, 0.5.</summary>
    public static string Float(double value)
    {
        var text = value.ToString("R", CultureInfo.InvariantCulture);

        if (!text.Contains('.', StringComparison.Ordinal) && !text.Contains('E', StringComparison.Ordinal))
        {
            text += ".0";
        }

        return text;
    }
}
