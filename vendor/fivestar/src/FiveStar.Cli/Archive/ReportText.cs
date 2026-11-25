// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text;
using FiveStar.Formats.Common;

namespace FiveStar.Cli.Archive;

/// <summary>Text forms shared by the reports: quoted strings, count maps and padded columns.</summary>
internal static class ReportText
{
    private const char FirstPrintable = ' ';
    private const char LastAscii = '~';
    private const int MinHexEscape = 0x100;

    /// <summary>A string in single quotes (double quotes when it holds a single quote but no double quote), escaped.</summary>
    public static string Quote(string value)
    {
        var quote = '\'';

        if (value.Contains('\'', StringComparison.Ordinal) && !value.Contains('"', StringComparison.Ordinal))
        {
            quote = '"';
        }

        var text = new StringBuilder();
        text.Append(quote);

        foreach (var character in value)
        {
            AppendEscaped(text, character, quote);
        }

        text.Append(quote);

        return text.ToString();
    }

    /// <summary>A count map as <c>{'key': n, ...}</c> in the given order.</summary>
    public static string CountMap(IReadOnlyList<KeyValuePair<string, long>> entries)
    {
        var parts = new List<string>(entries.Count);

        foreach (var entry in entries)
        {
            parts.Add(Quote(entry.Key) + ": " + Number(entry.Value));
        }

        return "{" + string.Join(", ", parts) + "}";
    }

    /// <summary>A tally as <c>{'key': n, ...}</c> in first-seen order.</summary>
    public static string CountMap(Tally<string> tally)
    {
        var entries = new List<KeyValuePair<string, long>>(tally.Count);

        foreach (var key in tally.Keys)
        {
            entries.Add(new KeyValuePair<string, long>(key, tally[key]));
        }

        return CountMap(entries);
    }

    public static string Number(long value)
    {
        return value.ToString(CultureInfo.InvariantCulture);
    }

    /// <summary>Lower-case hex with at least <paramref name="digits"/> digits.</summary>
    public static string Hex(long value, int digits)
    {
        return value.ToString("x" + digits.ToString(CultureInfo.InvariantCulture), CultureInfo.InvariantCulture);
    }

    private static void AppendEscaped(StringBuilder text, char character, char quote)
    {
        if (character == quote || character == '\\')
        {
            text.Append('\\').Append(character);

            return;
        }

        switch (character)
        {
            case '\n':
                text.Append("\\n");

                return;
            case '\r':
                text.Append("\\r");

                return;
            case '\t':
                text.Append("\\t");

                return;
            default:
                break;
        }

        if (character >= FirstPrintable && (character <= LastAscii || !char.IsControl(character)))
        {
            text.Append(character);

            return;
        }

        var width = character < MinHexEscape ? "x2" : "x4";
        var prefix = character < MinHexEscape ? "\\x" : "\\u";
        text.Append(prefix).Append(((int)character).ToString(width, CultureInfo.InvariantCulture));
    }
}
