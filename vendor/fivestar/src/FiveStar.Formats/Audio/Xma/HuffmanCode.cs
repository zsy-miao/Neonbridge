// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Audio.Xma;

/// <summary>
/// A prefix code given by its code lengths in code order: the first code is all zeros, and each next code adds one at
/// the previous code's last bit and keeps as many leading bits as its own length. Decoding walks a binary tree one bit
/// at a time.
/// </summary>
internal sealed class HuffmanCode
{
    /// <summary>Returned for a bit pattern that is no code.</summary>
    public const int Invalid = int.MinValue;

    private const int MaxLength = 32;
    private const int Branches = 2;

    private readonly int[] _children;

    /// <param name="lengths">Each code's length in bits, in code order.</param>
    /// <param name="symbols">The value each code decodes to.</param>
    public HuffmanCode(IReadOnlyList<int> lengths, IReadOnlyList<int> symbols)
    {
        _children = new int[Branches * Branches * lengths.Count];
        var nodes = 1;
        ulong code = 0;

        for (var index = 0; index < lengths.Count; index++)
        {
            var length = lengths[index];
            nodes = Insert((uint)(code >> (MaxLength - length)), length, symbols[index], nodes);
            code += 1UL << (MaxLength - length);
        }
    }

    /// <summary>Builds a code from (symbol, length) pairs.</summary>
    public static HuffmanCode FromPairs(ReadOnlySpan<byte> pairs)
    {
        var lengths = new int[pairs.Length / Branches];
        var symbols = new int[lengths.Length];

        for (var index = 0; index < lengths.Length; index++)
        {
            symbols[index] = pairs[Branches * index];
            lengths[index] = pairs[(Branches * index) + 1];
        }

        return new HuffmanCode(lengths, symbols);
    }

    /// <summary>Reads one code and returns its symbol, or <see cref="Invalid"/>.</summary>
    [HotPath]
    public int Decode(ref XmaBitReader reader)
    {
        var node = 0;

        for (var depth = 0; depth < MaxLength; depth++)
        {
            var next = _children[(Branches * node) + (int)reader.Read(1)];

            if (next < 0)
            {
                return -next - 1;
            }

            if (next == 0)
            {
                return Invalid;
            }

            node = next;
        }

        return Invalid;
    }

    /// <summary>Adds one code; leaves hold the (non-negative) symbol as <c>-(symbol + 1)</c>.</summary>
    private int Insert(uint code, int length, int symbol, int nodes)
    {
        var node = 0;

        for (var bit = length - 1; bit > 0; bit--)
        {
            var slot = (Branches * node) + (int)((code >> bit) & 1);

            if (_children[slot] == 0)
            {
                _children[slot] = nodes++;
            }

            node = _children[slot];
        }

        _children[(Branches * node) + (int)(code & 1)] = -symbol - 1;

        return nodes;
    }
}
