// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Audio.Xma;

/// <summary>Collects the bits of one frame, which may start in one packet and end in the next, into one buffer.</summary>
internal sealed class XmaFrameAssembler
{
    private const int BitsPerByte = 8;
    private const int ByteShift = 3;
    private const int BitInByte = 7;
    private const int HighBit = 0x80;

    /// <summary>The longest frame the 15-bit length allows, in bytes, plus room for the bit reader's look-ahead.</summary>
    private const int Capacity = ((1 << XmaLayout.FrameLengthBits) / BitsPerByte) + BitsPerByte;

    private readonly byte[] _buffer = new byte[Capacity];

    /// <summary>Gets the bits collected.</summary>
    public int Bits { get; private set; }

    /// <summary>Gets the frame's length in bits once its length prefix is in, else 0.</summary>
    public int DeclaredLength
    {
        get
        {
            if (Bits < XmaLayout.FrameLengthBits)
            {
                return 0;
            }

            return (int)Reader().Peek(XmaLayout.FrameLengthBits);
        }
    }

    /// <summary>Gets a value indicating whether the declared length is in and collected.</summary>
    public bool Complete => DeclaredLength > 0 && Bits >= DeclaredLength;

    public ReadOnlySpan<byte> Data => _buffer;

    public void Clear() => Bits = 0;

    /// <summary>Appends the next <paramref name="count"/> bits of <paramref name="source"/>.</summary>
    [HotPath]
    public void Collect(ref XmaBitReader source, int count)
    {
        var room = (_buffer.Length * BitsPerByte) - Bits;
        var taken = count < room ? count : room;

        for (var bit = 0; bit < taken; bit++)
        {
            var mask = HighBit >> (Bits & BitInByte);
            var index = Bits >> ByteShift;

            if (source.ReadBit())
            {
                _buffer[index] = (byte)(_buffer[index] | mask);
            }
            else
            {
                _buffer[index] = (byte)(_buffer[index] & ~mask);
            }

            Bits++;
        }

        source.Advance(count - taken);
    }

    /// <summary>Whether the frame says another frame follows it in the packet: its last bit.</summary>
    [HotPath]
    public bool MoreFrames()
    {
        var reader = Reader();
        reader.Position = DeclaredLength - 1;

        return reader.ReadBit();
    }

    private XmaBitReader Reader() => new(_buffer, 0, Bits);
}
