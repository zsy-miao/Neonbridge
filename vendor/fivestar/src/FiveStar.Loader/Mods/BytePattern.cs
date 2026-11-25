// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Loader.Mods;

/// <summary>A byte sequence in which <c>??</c> matches any byte, written as hex pairs separated by spaces.</summary>
internal sealed class BytePattern
{
    private const string Wildcard = "??";
    private const string ShortWildcard = "?";
    private const int HexDigitsPerByte = 2;

    private readonly byte[] _values;
    private readonly bool[] _known;
    private readonly int _anchor;

    public BytePattern(byte[] values, bool[] known)
    {
        if (values.Length == 0 || values.Length != known.Length)
        {
            throw new ArgumentException("a pattern needs one known flag per byte and at least one byte");
        }

        _anchor = Array.IndexOf(known, true);

        if (_anchor < 0)
        {
            throw new ArgumentException("a pattern needs at least one byte that is not a wildcard");
        }

        _values = values;
        _known = known;
    }

    public int Length => _values.Length;

    /// <summary>Parses "48 8B ?? 05"; throws <see cref="FormatException"/> on anything else.</summary>
    public static BytePattern Parse(string text)
    {
        string[] tokens = text.Split(' ', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries);
        var values = new byte[tokens.Length];
        var known = new bool[tokens.Length];

        for (int i = 0; i < tokens.Length; i++)
        {
            if (tokens[i] is Wildcard or ShortWildcard)
            {
                continue;
            }

            if (!byte.TryParse(tokens[i], NumberStyles.AllowHexSpecifier, CultureInfo.InvariantCulture, out values[i]) || tokens[i].Length != HexDigitsPerByte)
            {
                throw new FormatException($"'{tokens[i]}' is not a hex byte or {Wildcard}");
            }

            known[i] = true;
        }

        try
        {
            return new BytePattern(values, known);
        }
        catch (ArgumentException e)
        {
            throw new FormatException(e.Message, e);
        }
    }

    /// <summary>Offsets of the matches in <paramref name="memory"/>, in order; stops after <paramref name="limit"/> matches.</summary>
    public List<int> FindAll(ReadOnlySpan<byte> memory, int limit)
    {
        var matches = new List<int>();
        int start = 0;

        while (matches.Count < limit && start + _values.Length <= memory.Length)
        {
            int found = memory[(start + _anchor)..].IndexOf(_values[_anchor]);

            if (found < 0)
            {
                break;
            }

            int candidate = start + found;

            if (candidate + _values.Length > memory.Length)
            {
                break;
            }

            if (MatchesAt(memory, candidate))
            {
                matches.Add(candidate);
            }

            start = candidate + 1;
        }

        return matches;
    }

    public override string ToString()
    {
        var parts = new string[_values.Length];

        for (int i = 0; i < parts.Length; i++)
        {
            parts[i] = Wildcard;

            if (_known[i])
            {
                parts[i] = _values[i].ToString("X2", CultureInfo.InvariantCulture);
            }
        }

        return string.Join(' ', parts);
    }

    private bool MatchesAt(ReadOnlySpan<byte> memory, int position)
    {
        for (int i = 0; i < _values.Length; i++)
        {
            if (_known[i] && memory[position + i] != _values[i])
            {
                return false;
            }
        }

        return true;
    }
}
