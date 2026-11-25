// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text;

namespace FiveStar.Shaders.Translation.Core;

/// <summary>Splits an operand list at top-level commas (commas inside brackets or parentheses stay).</summary>
internal static class ArgumentList
{
    public static List<string> Split(string text)
    {
        var parts = new List<string>();
        var current = new StringBuilder();
        var depth = 0;

        foreach (var c in text)
        {
            if (c is '[' or '(')
            {
                depth++;
            }

            if (c is ']' or ')')
            {
                depth--;
            }

            if (c == ',' && depth == 0)
            {
                parts.Add(current.ToString().Trim());
                current.Clear();
            }
            else
            {
                current.Append(c);
            }
        }

        var last = current.ToString().Trim();

        if (last.Length > 0)
        {
            parts.Add(last);
        }

        return parts;
    }

    /// <summary>The numbers of a bracketed list such as <c>[0,1,1]</c>; empty for null or empty text.</summary>
    public static List<int> Bits(string? text)
    {
        if (string.IsNullOrEmpty(text))
        {
            return [];
        }

        return text.Trim('[', ']').Split(',').Select(part => int.Parse(part.Trim(), System.Globalization.CultureInfo.InvariantCulture)).ToList();
    }
}
