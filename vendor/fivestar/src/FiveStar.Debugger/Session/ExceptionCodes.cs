// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Debugger.Session;

/// <summary>Names and parses exception codes.</summary>
internal static class ExceptionCodes
{
    private const string HexPrefix = "0x";

    /// <summary>The code in hex with its name when it has one: <c>c0000005 AccessViolation</c>.</summary>
    public static string Describe(uint code)
    {
        string? name = Enum.GetName((WellKnownException)code);

        if (name is null)
        {
            return $"{code:x8}";
        }

        return $"{code:x8} {name}";
    }

    public static bool IsBreakpoint(uint code)
    {
        return code == (uint)WellKnownException.Breakpoint || code == (uint)WellKnownException.Wx86Breakpoint;
    }

    /// <summary>A code given as hex (with or without 0x) or as a <see cref="WellKnownException"/> name.</summary>
    public static bool TryParse(string text, out uint code)
    {
        if (Enum.TryParse(text, true, out WellKnownException named) && Enum.IsDefined(named) && !char.IsAsciiDigit(text[0]))
        {
            code = (uint)named;

            return true;
        }

        string digits = text;

        if (digits.StartsWith(HexPrefix, StringComparison.OrdinalIgnoreCase))
        {
            digits = digits[HexPrefix.Length..];
        }

        return uint.TryParse(digits, NumberStyles.HexNumber, CultureInfo.InvariantCulture, out code);
    }
}
