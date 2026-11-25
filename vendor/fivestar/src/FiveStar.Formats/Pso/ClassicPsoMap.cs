// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Formats.Pso;

/// <summary>
/// The block map of a classic (big-endian) PSO file, chunk <c>PMAP</c>: u32 root block id, u32 0, u16 count, padding
/// to +0x18, then <c>{u32 struct hash, u32 file offset, u32 0, u32 length}</c> per block.
/// </summary>
/// <param name="Root">Root block id.</param>
/// <param name="Entries">The blocks in map order.</param>
public sealed record ClassicPsoMap(uint Root, IReadOnlyList<ClassicPsoBlock> Entries)
{
    private const int RootField = 8;
    private const int CountField = 0x10;
    private const int EntriesOffset = 0x18;
    private const int EntrySize = 16;
    private const int OffsetField = 4;
    private const int LengthField = 12;

    /// <summary>True when the file has a <c>PMAP</c> chunk.</summary>
    public static bool IsClassic(IReadOnlyList<PsoChunk> chunks)
    {
        foreach (var chunk in chunks)
        {
            if (chunk.Tag == "PMAP")
            {
                return true;
            }
        }

        return false;
    }

    public static ClassicPsoMap Read(byte[] data)
    {
        PsoChunk? map = null;

        foreach (var chunk in PsoFraming.Chunks(data))
        {
            if (chunk.Tag == "PMAP")
            {
                map = chunk;
            }
        }

        if (!map.HasValue)
        {
            throw new PsoException("PMAP", false);
        }

        var span = data.AsSpan(map.Value.Offset);
        var root = BinaryPrimitives.ReadUInt32BigEndian(span[RootField..]);
        var count = BinaryPrimitives.ReadUInt16BigEndian(span[CountField..]);
        var entries = new List<ClassicPsoBlock>(count);

        for (var index = 0; index < count; index++)
        {
            var entry = span[(EntriesOffset + (EntrySize * index))..];
            entries.Add(new ClassicPsoBlock(
                BinaryPrimitives.ReadUInt32BigEndian(entry),
                BinaryPrimitives.ReadUInt32BigEndian(entry[OffsetField..]),
                BinaryPrimitives.ReadUInt32BigEndian(entry[LengthField..])));
        }

        return new ClassicPsoMap(root, entries);
    }
}
