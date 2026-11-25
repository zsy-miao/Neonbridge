// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Audio.Xma;

/// <summary>
/// Reads an XMA bitstream: most significant bit first, up to 32 bits at a time. Reading past the end yields zero bits
/// and sets <see cref="Overrun"/>, so a decoder checks once per frame instead of on every read.
/// </summary>
internal ref struct XmaBitReader
{
    private const int MaxBits = 32;
    private const int WindowBytes = 5;
    private const int WindowBits = 64;
    private const int UnusedWindowBits = WindowBits - (WindowBytes * BitsPerByte);
    private const int BitsPerByte = 8;
    private const int ByteShift = 3;
    private const int BitInByte = 7;

    private readonly ReadOnlySpan<byte> _data;
    private readonly int _length;
    private int _position;

    /// <summary>Reads <paramref name="lengthBits"/> bits of <paramref name="data"/>, starting at <paramref name="startBit"/>.</summary>
    public XmaBitReader(ReadOnlySpan<byte> data, int startBit, int lengthBits)
    {
        _data = data;
        _position = startBit;
        _length = startBit + lengthBits;
    }

    /// <summary>Gets or sets the bit position from the start of the data.</summary>
    public int Position
    {
        readonly get => _position;
        set => _position = value;
    }

    /// <summary>Gets the bits left before the end.</summary>
    public readonly int Remaining => _length - _position;

    /// <summary>Gets a value indicating whether a read went past the end.</summary>
    public readonly bool Overrun => _position > _length;

    [HotPath]
    public readonly uint Peek(int count)
    {
        if (count == 0)
        {
            return 0;
        }

        var first = _position >> ByteShift;
        ulong window = 0;

        for (var index = 0; index < WindowBytes; index++)
        {
            window = (window << BitsPerByte) | ByteAt(first + index);
        }

        var aligned = window << (UnusedWindowBits + (_position & BitInByte));

        return (uint)(aligned >> (WindowBits - count));
    }

    [HotPath]
    public uint Read(int count)
    {
        var value = Peek(count);
        _position += count;

        return value;
    }

    [HotPath]
    public bool ReadBit() => Read(1) != 0;

    /// <summary>Reads a two's complement number of <paramref name="count"/> bits.</summary>
    [HotPath]
    public int ReadSigned(int count)
    {
        var value = (int)Read(count);
        var shift = MaxBits - count;

        return (value << shift) >> shift;
    }

    [HotPath]
    public void Advance(int count) => _position += count;

    private readonly ulong ByteAt(int index)
    {
        if (index < 0 || index >= _data.Length || index >= ((_length + BitInByte) >> ByteShift))
        {
            return 0;
        }

        return _data[index];
    }
}
