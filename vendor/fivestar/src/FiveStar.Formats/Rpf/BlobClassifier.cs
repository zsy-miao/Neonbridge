// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Formats.Rpf;

/// <summary>Names the kind of a carved entry from its leading bytes.</summary>
public static class BlobClassifier
{
    // Paged resource root: u64 vtable slot (0 on disk) at +0x00, page-map pointer at +0x08, 'PRD0' at +0x10.
    private const int VtableSize = 8;
    private const int PageMapPointerOffset = 8;
    private const int Prd0MagicOffset = 0x10;
    private const int Prd0MagicEnd = 0x14;
    private const int SegmentShift = 28;
    private const ulong VirtualSegment = 5;

    // Text: the first 64 bytes are printable ASCII, tab, line feed or carriage return.
    private const int TextProbeLength = 64;
    private const int TextAsciiProbe = 4;
    private const byte FirstPrintable = 32;
    private const byte LastPrintable = 126;
    private const byte Tab = 9;
    private const byte LineFeed = 10;
    private const byte CarriageReturn = 13;

    /// <summary>The <c>PRD0</c> magic as stored (the bytes <c>30 44 52 50</c>, "0DRP").</summary>
    public static ReadOnlySpan<byte> Prd0Magic => "0DRP"u8;

    public static BlobKind Classify(ReadOnlySpan<byte> data)
    {
        if (IsPrd0(data))
        {
            return BlobKind.Prd0;
        }

        if (IsVirtualSegment(data))
        {
            return BlobKind.Rsc;
        }

        var magic = ClassifyMagic(data);

        if (magic is not null)
        {
            return magic.Value;
        }

        if (LooksLikeText(data))
        {
            return BlobKind.Txt;
        }

        return BlobKind.Bin;
    }

    /// <summary>The lower-case tag used in carved file names (<c>prd0</c>, <c>psin</c>, <c>phys</c>...).</summary>
    public static string Tag(BlobKind kind)
    {
        return kind.ToString().ToLowerInvariant();
    }

    /// <summary>True for a virtual segment: u64 0 then a pointer whose top nibble is 5.</summary>
    public static bool IsVirtualSegment(ReadOnlySpan<byte> data)
    {
        if (data.Length < Prd0MagicOffset || data[..VtableSize].IndexOfAnyExcept((byte)0) >= 0)
        {
            return false;
        }

        return BinaryPrimitives.ReadUInt64LittleEndian(data[PageMapPointerOffset..]) >> SegmentShift == VirtualSegment;
    }

    /// <summary>True when <c>PRD0</c> sits at +0x10 (the vtable slot is not checked).</summary>
    public static bool HasPrd0Magic(ReadOnlySpan<byte> data)
    {
        return data.Length >= Prd0MagicEnd && data[Prd0MagicOffset..Prd0MagicEnd].SequenceEqual(Prd0Magic);
    }

    private static bool IsPrd0(ReadOnlySpan<byte> data)
    {
        return HasPrd0Magic(data) && data[..VtableSize].IndexOfAnyExcept((byte)0) < 0;
    }

    private static BlobKind? ClassifyMagic(ReadOnlySpan<byte> data)
    {
        if (data.StartsWith("PSIN"u8))
        {
            return BlobKind.Psin;
        }

        if (data.StartsWith("<?xml"u8) || data.StartsWith("﻿<?xml"u8))
        {
            return BlobKind.Xml;
        }

        if (data.StartsWith("RSC"u8))
        {
            return BlobKind.Rsc;
        }

        if (data.StartsWith("KB2"u8))
        {
            return BlobKind.Bk2;
        }

        return ClassifyContainerMagic(data);
    }

    private static BlobKind? ClassifyContainerMagic(ReadOnlySpan<byte> data)
    {
        if (data.StartsWith("RIFF"u8))
        {
            return BlobKind.Riff;
        }

        if (data.StartsWith("DXBC"u8))
        {
            return BlobKind.Dxbc;
        }

        if (data.StartsWith("SGD5"u8))
        {
            return BlobKind.Sgd5;
        }

        if (data.StartsWith("ETPF"u8))
        {
            return BlobKind.Etpf;
        }

        if (data.StartsWith("RDS5"u8))
        {
            return BlobKind.Rds5;
        }

        return null;
    }

    private static bool LooksLikeText(ReadOnlySpan<byte> data)
    {
        if (data.IsEmpty)
        {
            return true;
        }

        var first = data[0];

        if (first == (byte)'<' || first == (byte)'{' || first == (byte)'[')
        {
            return true;
        }

        if (!System.Text.Ascii.IsValid(data[..Math.Min(TextAsciiProbe, data.Length)]))
        {
            return false;
        }

        foreach (var value in data[..Math.Min(TextProbeLength, data.Length)])
        {
            var printable = value >= FirstPrintable && value <= LastPrintable;

            if (!printable && value != Tab && value != LineFeed && value != CarriageReturn)
            {
                return false;
            }
        }

        return true;
    }
}
