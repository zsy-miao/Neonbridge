// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Compression;

/// <summary>Reads a deflate bit stream, least significant bit first, from a span.</summary>
internal ref struct BitInput
{
    private const int BitsPerByte = 8;

    // A 64-bit buffer takes one more whole byte while it holds at most 56 bits.
    private const int MaxBufferedBits = 56;

    private readonly ReadOnlySpan<byte> _data;
    private int _position;
    private ulong _bits;
    private int _count;

    public BitInput(ReadOnlySpan<byte> data)
    {
        _data = data;
        _position = 0;
        _bits = 0;
        _count = 0;
    }

    /// <summary>Gets the number of bits buffered and ready to read.</summary>
    public readonly int Count => _count;

    /// <summary>Gets the byte position of the next unread byte; only meaningful after <see cref="ReturnBufferedBytes"/>.</summary>
    public readonly int Position => _position;

    /// <summary>Gets the bytes not yet read; only meaningful after <see cref="ReturnBufferedBytes"/>.</summary>
    public readonly ReadOnlySpan<byte> Remaining => _data[_position..];

    /// <summary>Buffers bits until at least <paramref name="need"/> are ready; false when the input ends first.</summary>
    public bool Fill(int need)
    {
        if (_count >= need)
        {
            return true;
        }

        while (_count <= MaxBufferedBits && _position < _data.Length)
        {
            _bits |= (ulong)_data[_position] << _count;
            _position++;
            _count += BitsPerByte;
        }

        return _count >= need;
    }

    /// <summary>The next <paramref name="count"/> bits without consuming them (zero past the end of the input).</summary>
    public readonly int Peek(int count)
    {
        return (int)(_bits & ((1UL << count) - 1));
    }

    /// <summary>The bit at <paramref name="index"/> ahead of the read position.</summary>
    public readonly int PeekBit(int index)
    {
        return (int)((_bits >> index) & 1);
    }

    public void Drop(int count)
    {
        _bits >>= count;
        _count -= count;
    }

    /// <summary>Reads <paramref name="count"/> bits; false when the input ends first.</summary>
    public bool TryRead(int count, out int value)
    {
        if (!Fill(count))
        {
            value = 0;

            return false;
        }

        value = Peek(count);
        Drop(count);

        return true;
    }

    /// <summary>Drops the bits up to the next byte boundary.</summary>
    public void AlignToByte()
    {
        Drop(_count % BitsPerByte);
    }

    /// <summary>Gives whole buffered bytes back to the input, so byte reads continue at the aligned position.</summary>
    public void ReturnBufferedBytes()
    {
        _position -= _count / BitsPerByte;
        _bits = 0;
        _count = 0;
    }

    public void Skip(int bytes)
    {
        _position += bytes;
    }
}
