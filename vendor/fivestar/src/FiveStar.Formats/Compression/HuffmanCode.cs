// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Compression;

/// <summary>
/// A canonical deflate Huffman code (RFC 1951 section 3.2.2) built from code lengths, decoded through a lookup
/// table for short codes and a canonical walk for long ones.
/// </summary>
internal sealed class HuffmanCode
{
    /// <summary>The longest code deflate allows (RFC 1951 section 3.2.7).</summary>
    public const int MaxCodeLength = 15;

    /// <summary>Returned by <see cref="Decode"/> when the input ends inside a code.</summary>
    public const int Truncated = -1;

    /// <summary>Returned by <see cref="Decode"/> for a bit pattern that is no code.</summary>
    public const int Invalid = -2;

    private const int FastBits = 10;
    private const int LengthFieldBits = 4;
    private const int LengthFieldMask = (1 << LengthFieldBits) - 1;

    private readonly int[] _counts = new int[MaxCodeLength + 1];
    private readonly int[] _offsets = new int[MaxCodeLength + 1];
    private readonly int[] _nextCode = new int[MaxCodeLength + 1];
    private readonly short[] _symbols;
    private readonly int[] _fast = new int[1 << FastBits];

    public HuffmanCode(int maxSymbols)
    {
        _symbols = new short[maxSymbols];
    }

    /// <summary>Builds a code that must be complete (the code-length code).</summary>
    public bool Build(ReadOnlySpan<byte> lengths)
    {
        return Build(lengths, false);
    }

    /// <summary>
    /// Builds the code. Over-subscribed length sets are refused; incomplete sets are refused unless
    /// <paramref name="allowSingleCode"/> is set and the only code has length 1 (zlib's rule for literal and
    /// distance codes). A set with no codes at all builds, and every decode then fails.
    /// </summary>
    public bool Build(ReadOnlySpan<byte> lengths, bool allowSingleCode)
    {
        Array.Clear(_counts);
        Array.Clear(_fast);

        foreach (var length in lengths)
        {
            _counts[length]++;
        }

        _counts[0] = 0;
        var longest = MaxCodeLength;

        while (longest > 0 && _counts[longest] == 0)
        {
            longest--;
        }

        if (longest == 0)
        {
            return true;
        }

        if (!LengthsFit(longest, allowSingleCode))
        {
            return false;
        }

        SortSymbols(lengths);
        FillFastTable(lengths);

        return true;
    }

    /// <summary>Decodes one symbol; <see cref="Truncated"/> or <see cref="Invalid"/> on failure.</summary>
    public int Decode(ref BitInput input)
    {
        input.Fill(MaxCodeLength);
        var entry = _fast[input.Peek(FastBits)];

        if (entry == 0)
        {
            return DecodeLong(ref input);
        }

        var length = entry & LengthFieldMask;

        if (length > input.Count)
        {
            return Truncated;
        }

        input.Drop(length);

        return entry >> LengthFieldBits;
    }

    private bool LengthsFit(int longest, bool allowSingleCode)
    {
        var left = 1;

        for (var length = 1; length <= MaxCodeLength; length++)
        {
            left <<= 1;
            left -= _counts[length];

            if (left < 0)
            {
                return false;
            }
        }

        return left == 0 || (allowSingleCode && longest == 1);
    }

    private void SortSymbols(ReadOnlySpan<byte> lengths)
    {
        _offsets[1] = 0;

        for (var length = 1; length < MaxCodeLength; length++)
        {
            _offsets[length + 1] = _offsets[length] + _counts[length];
        }

        Span<int> next = stackalloc int[MaxCodeLength + 1];
        _offsets.CopyTo(next);

        for (var symbol = 0; symbol < lengths.Length; symbol++)
        {
            var length = lengths[symbol];

            if (length != 0)
            {
                _symbols[next[length]] = (short)symbol;
                next[length]++;
            }
        }
    }

    private void FillFastTable(ReadOnlySpan<byte> lengths)
    {
        var code = 0;
        _nextCode[0] = 0;

        for (var length = 1; length <= MaxCodeLength; length++)
        {
            code = (code + _counts[length - 1]) << 1;
            _nextCode[length] = code;
        }

        for (var symbol = 0; symbol < lengths.Length; symbol++)
        {
            int length = lengths[symbol];

            if (length == 0)
            {
                continue;
            }

            var reversed = Reverse(_nextCode[length], length);
            _nextCode[length]++;

            if (length > FastBits)
            {
                continue;
            }

            var entry = (symbol << LengthFieldBits) | length;

            for (var index = reversed; index < _fast.Length; index += 1 << length)
            {
                _fast[index] = entry;
            }
        }
    }

    private int DecodeLong(ref BitInput input)
    {
        var code = 0;
        var first = 0;
        var index = 0;

        for (var length = 1; length <= MaxCodeLength; length++)
        {
            if (length > input.Count)
            {
                return Truncated;
            }

            code |= input.PeekBit(length - 1);
            var count = _counts[length];

            if (code - count < first)
            {
                input.Drop(length);

                return _symbols[index + (code - first)];
            }

            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }

        return Invalid;
    }

    private static int Reverse(int code, int length)
    {
        var reversed = 0;

        for (var bit = 0; bit < length; bit++)
        {
            reversed = (reversed << 1) | ((code >> bit) & 1);
        }

        return reversed;
    }
}
