// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Formats.Tests.Support;

/// <summary>Builds small binary files for tests, little-endian unless a method says otherwise.</summary>
public sealed class ByteWriter
{
    private readonly List<byte> _bytes = [];

    public int Position => _bytes.Count;

    public ByteWriter Bytes(ReadOnlySpan<byte> bytes)
    {
        foreach (var value in bytes)
        {
            _bytes.Add(value);
        }

        return this;
    }

    public ByteWriter U8(byte value)
    {
        _bytes.Add(value);

        return this;
    }

    public ByteWriter U16(ushort value)
    {
        Span<byte> buffer = stackalloc byte[2];
        BinaryPrimitives.WriteUInt16LittleEndian(buffer, value);

        return Bytes(buffer);
    }

    public ByteWriter U32(uint value)
    {
        Span<byte> buffer = stackalloc byte[4];
        BinaryPrimitives.WriteUInt32LittleEndian(buffer, value);

        return Bytes(buffer);
    }

    public ByteWriter U32BigEndian(uint value)
    {
        Span<byte> buffer = stackalloc byte[4];
        BinaryPrimitives.WriteUInt32BigEndian(buffer, value);

        return Bytes(buffer);
    }

    public ByteWriter U64(ulong value)
    {
        Span<byte> buffer = stackalloc byte[8];
        BinaryPrimitives.WriteUInt64LittleEndian(buffer, value);

        return Bytes(buffer);
    }

    /// <summary>Pads with <paramref name="fill"/> up to the next multiple of <paramref name="alignment"/>.</summary>
    public ByteWriter Align(int alignment, byte fill)
    {
        while (_bytes.Count % alignment != 0)
        {
            _bytes.Add(fill);
        }

        return this;
    }

    public ByteWriter PadTo(int position)
    {
        while (_bytes.Count < position)
        {
            _bytes.Add(0);
        }

        return this;
    }

    public void PatchU32BigEndian(int position, uint value)
    {
        Span<byte> buffer = stackalloc byte[4];
        BinaryPrimitives.WriteUInt32BigEndian(buffer, value);

        for (var index = 0; index < buffer.Length; index++)
        {
            _bytes[position + index] = buffer[index];
        }
    }

    public byte[] ToArray()
    {
        return [.. _bytes];
    }
}
