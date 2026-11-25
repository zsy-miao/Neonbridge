// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Formats.Pso;

/// <summary>Bounds-checked little-endian reads from a PSO buffer; a read past the end throws <see cref="PsoException"/>.</summary>
internal static class PsoBytes
{
    /// <summary>Throws unless <paramref name="size"/> bytes are readable at <paramref name="at"/>.</summary>
    public static void Check(byte[] data, long at, int size)
    {
        if (at < 0 || at + size > data.Length)
        {
            throw new PsoException(
                $"unpack_from requires a buffer of at least {at + size} bytes for unpacking {size} bytes at offset {at} (actual buffer size is {data.Length})",
                false);
        }
    }

    public static ushort U16(byte[] data, long at)
    {
        Check(data, at, sizeof(ushort));

        return BinaryPrimitives.ReadUInt16LittleEndian(data.AsSpan((int)at));
    }

    public static uint U32(byte[] data, long at)
    {
        Check(data, at, sizeof(uint));

        return BinaryPrimitives.ReadUInt32LittleEndian(data.AsSpan((int)at));
    }

    public static ulong U64(byte[] data, long at)
    {
        Check(data, at, sizeof(ulong));

        return BinaryPrimitives.ReadUInt64LittleEndian(data.AsSpan((int)at));
    }

    /// <summary>Up to <paramref name="length"/> bytes at <paramref name="at"/>, cut short at the end of the buffer.</summary>
    public static ReadOnlySpan<byte> Slice(byte[] data, long at, long length)
    {
        if (at >= data.Length)
        {
            return ReadOnlySpan<byte>.Empty;
        }

        return data.AsSpan((int)at, (int)Math.Min(length, data.Length - at));
    }
}
