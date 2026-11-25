// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Formats.Models;

/// <summary>
/// The two segments of a paged resource. Objects live in the virtual segment and point at each other with 64-bit
/// pointers 0x50000000 + offset; buffers live in the physical segment, named by pointers 0x60000000 + offset. Reads
/// outside a segment throw <see cref="ModelFormatException"/>.
/// </summary>
public sealed class ResourceView
{
    /// <summary>Pointer base of the virtual segment.</summary>
    public const ulong VirtualBase = 0x5000_0000;

    /// <summary>Pointer base of the physical segment.</summary>
    public const ulong PhysicalBase = 0x6000_0000;

    private const ulong SegmentSpan = 0x1000_0000;

    private readonly ReadOnlyMemory<byte> _virtual;
    private readonly ReadOnlyMemory<byte> _physical;

    public ResourceView(ReadOnlyMemory<byte> virtualSegment, ReadOnlyMemory<byte> physicalSegment)
    {
        _virtual = virtualSegment;
        _physical = physicalSegment;
    }

    public int VirtualLength => _virtual.Length;

    public int PhysicalLength => _physical.Length;

    /// <summary>The virtual offset named by the pointer stored at <paramref name="at"/>, or null for anything else.</summary>
    public int? Follow(int at)
    {
        if (at < 0 || at + sizeof(ulong) > _virtual.Length)
        {
            return null;
        }

        var pointer = BinaryPrimitives.ReadUInt64LittleEndian(_virtual.Span[at..]);

        if (pointer < VirtualBase || pointer >= VirtualBase + (ulong)_virtual.Length)
        {
            return null;
        }

        return (int)(pointer - VirtualBase);
    }

    /// <summary>The virtual offset named by the pointer at <paramref name="at"/>; throws when there is none.</summary>
    public int FollowRequired(int at, string what)
    {
        var offset = Follow(at);

        if (!offset.HasValue)
        {
            throw new ModelFormatException($"no {what} pointer at +0x{at:X}");
        }

        return offset.Value;
    }

    /// <summary>The physical offset named by the pointer at <paramref name="at"/>; throws when it is not a physical pointer.</summary>
    public int FollowPhysical(int at)
    {
        var pointer = U64(at);

        if (pointer < PhysicalBase || pointer >= PhysicalBase + SegmentSpan)
        {
            throw new ModelFormatException($"no physical pointer at +0x{at:X}");
        }

        return (int)(pointer - PhysicalBase);
    }

    public ushort U16(int at)
    {
        return BinaryPrimitives.ReadUInt16LittleEndian(VirtualBytes(at, sizeof(ushort)));
    }

    public uint U32(int at)
    {
        return BinaryPrimitives.ReadUInt32LittleEndian(VirtualBytes(at, sizeof(uint)));
    }

    public float F32(int at)
    {
        return BinaryPrimitives.ReadSingleLittleEndian(VirtualBytes(at, sizeof(float)));
    }

    public ulong U64(int at)
    {
        return BinaryPrimitives.ReadUInt64LittleEndian(VirtualBytes(at, sizeof(ulong)));
    }

    /// <summary>The NUL-terminated ASCII text at a virtual offset, or null when it is not printable.</summary>
    public string? Text(int at, int maxLength)
    {
        var span = _virtual.Span[at..];
        var end = span[..Math.Min(maxLength, span.Length)].IndexOf((byte)0);

        if (end <= 0)
        {
            return null;
        }

        foreach (var value in span[..end])
        {
            if (value < (byte)' ' || value > (byte)'~')
            {
                return null;
            }
        }

        return System.Text.Encoding.ASCII.GetString(span[..end]);
    }

    public ReadOnlySpan<byte> VirtualBytes(int at, int length)
    {
        if (at < 0 || length < 0 || at + length > _virtual.Length)
        {
            throw new ModelFormatException($"virtual segment ends at 0x{_virtual.Length:X}, read at 0x{at:X}");
        }

        return _virtual.Span.Slice(at, length);
    }

    public ReadOnlySpan<byte> PhysicalBytes(int at, int length)
    {
        if (at < 0 || length < 0 || (long)at + length > _physical.Length)
        {
            throw new ModelFormatException($"physical segment ends at 0x{_physical.Length:X}, needs 0x{(long)at + length:X}");
        }

        return _physical.Span.Slice(at, length);
    }
}
