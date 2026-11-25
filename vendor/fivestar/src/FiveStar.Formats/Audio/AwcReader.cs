// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using FiveStar.Formats.Common;

namespace FiveStar.Formats.Audio;

/// <summary>
/// Reads the header of an AWC container: <c>ADAT</c>, u16 version 1, u16 flags (high byte 0xFF), u32 stream count and
/// u32 end of the chunk table, then optional tables chosen by the flags, one u32 per stream (id in bits 0..28, chunk
/// count in bits 29..31) and one u64 per chunk. A header counts only when every table adds up and every chunk fits.
/// </summary>
public static class AwcReader
{
    /// <summary>Flag: a u32 of unknown meaning follows the fixed header.</summary>
    public const ushort LeadWordFlag = 0x40;

    /// <summary>Flag: a u16 first-chunk index per stream.</summary>
    public const ushort ChunkIndexFlag = 0x01;

    /// <summary>Flag: one u32 more than there are streams, of unknown meaning.</summary>
    public const ushort ExtraWordsFlag = 0x20;

    /// <summary>Flag: the container holds a multichannel stream.</summary>
    public const ushort MultichannelFlag = 0x04;

    private const int FixedHeaderSize = 16;
    private const int VersionOffset = 4;
    private const int FlagsOffset = 6;
    private const int StreamCountOffset = 8;
    private const int TableEndOffset = 12;
    private const ushort Version = 1;
    private const ushort HighFlags = 0xFF00;
    private const ushort UnknownFlag = 0x02;
    private const ushort KnownFlags = HighFlags | LeadWordFlag | ExtraWordsFlag | MultichannelFlag | ChunkIndexFlag | UnknownFlag;
    private const int MaxStreams = 1 << 16;
    private const int MaxHeaderSize = 16 << 20;
    private const int IdBits = 29;
    private const uint IdMask = (1u << IdBits) - 1;
    private const int ChunkEntrySize = 8;

    /// <summary>Gets the magic of an AWC container.</summary>
    public static ReadOnlySpan<byte> Magic => "ADAT"u8;

    /// <summary>The container at <paramref name="offset"/>, or null when the bytes there are not a valid AWC header.</summary>
    public static AwcContainer? TryRead(IByteSource source, long offset)
    {
        var head = source.Read(offset, FixedHeaderSize);

        if (head.Length < FixedHeaderSize || !head.StartsWith(Magic))
        {
            return null;
        }

        var flags = BinaryPrimitives.ReadUInt16LittleEndian(head[FlagsOffset..]);
        var count = BinaryPrimitives.ReadUInt32LittleEndian(head[StreamCountOffset..]);
        var tableEnd = BinaryPrimitives.ReadUInt32LittleEndian(head[TableEndOffset..]);
        var valid = BinaryPrimitives.ReadUInt16LittleEndian(head[VersionOffset..]) == Version
            && (flags & HighFlags) == HighFlags
            && (flags & ~KnownFlags) == 0;

        if (!valid || count == 0 || count > MaxStreams || tableEnd > MaxHeaderSize)
        {
            return null;
        }

        var header = source.Read(offset, (int)tableEnd);

        if (header.Length < tableEnd)
        {
            return null;
        }

        var streams = ReadTables(header, flags, (int)count);

        if (streams is null)
        {
            return null;
        }

        var size = ContainerSize(streams, tableEnd);

        if (offset + size > source.Length)
        {
            return null;
        }

        return new AwcContainer(offset, size, flags, streams);
    }

    private static List<AwcStreamEntry>? ReadTables(ReadOnlySpan<byte> header, ushort flags, int count)
    {
        var position = FixedHeaderSize;

        if ((flags & LeadWordFlag) != 0)
        {
            position += sizeof(uint);
        }

        var indexStart = position;

        if ((flags & ChunkIndexFlag) != 0)
        {
            position += count * sizeof(ushort);
        }

        if ((flags & ExtraWordsFlag) != 0)
        {
            position += (count + 1) * sizeof(uint);
        }

        var infoStart = position;
        var chunkStart = infoStart + (count * sizeof(uint));

        if (chunkStart > header.Length)
        {
            return null;
        }

        return ReadStreams(header, flags, count, indexStart, infoStart, chunkStart);
    }

    private static List<AwcStreamEntry>? ReadStreams(ReadOnlySpan<byte> header, ushort flags, int count, int indexStart, int infoStart, int chunkStart)
    {
        var streams = new List<AwcStreamEntry>(count);
        var next = 0;

        for (var stream = 0; stream < count; stream++)
        {
            var info = BinaryPrimitives.ReadUInt32LittleEndian(header[(infoStart + (stream * sizeof(uint)))..]);
            var chunkCount = (int)(info >> IdBits);
            var indexed = (flags & ChunkIndexFlag) != 0;

            if (indexed && BinaryPrimitives.ReadUInt16LittleEndian(header[(indexStart + (stream * sizeof(ushort)))..]) != next)
            {
                return null;
            }

            var tableAt = chunkStart + ((next + chunkCount) * ChunkEntrySize);

            if (tableAt > header.Length)
            {
                return null;
            }

            var chunks = new AwcChunk[chunkCount];

            for (var index = 0; index < chunkCount; index++)
            {
                var entry = BinaryPrimitives.ReadUInt64LittleEndian(header[(chunkStart + ((next + index) * ChunkEntrySize))..]);
                chunks[index] = AwcChunk.Decode(entry);
            }

            streams.Add(new AwcStreamEntry(info & IdMask, chunks));
            next += chunkCount;
        }

        if (chunkStart + (next * ChunkEntrySize) != header.Length)
        {
            return null;
        }

        return streams;
    }

    private static long ContainerSize(IEnumerable<AwcStreamEntry> streams, long tableEnd)
    {
        var size = tableEnd;

        foreach (var stream in streams)
        {
            foreach (var chunk in stream.Chunks)
            {
                size = Math.Max(size, chunk.End);
            }
        }

        return size;
    }
}
