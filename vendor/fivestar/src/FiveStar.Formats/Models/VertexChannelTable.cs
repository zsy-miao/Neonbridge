// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Formats.Models;

/// <summary>
/// The table at the start of a vertex buffer: one u32 per slot, 0 for an absent stream, else the format byte in bits
/// 0-7 and the stream's byte offset / 2 in bits 8-31.
/// </summary>
public static class VertexChannelTable
{
    /// <summary>Slots in the table; the word after them belongs to the position header.</summary>
    public const int Slots = 23;

    private const int FormatMask = 0xFF;
    private const int OffsetShift = 8;
    private const int OffsetUnit = 2;

    public static IReadOnlyList<VertexChannel> Read(ReadOnlySpan<byte> buffer)
    {
        var streams = new List<VertexChannel>();

        if (buffer.Length < Slots * sizeof(uint))
        {
            throw new ModelFormatException($"vertex buffer of {buffer.Length} bytes has no stream table");
        }

        for (var slot = 0; slot < Slots; slot++)
        {
            var word = BinaryPrimitives.ReadUInt32LittleEndian(buffer[(slot * sizeof(uint))..]);

            if (word == 0)
            {
                continue;
            }

            streams.Add(new VertexChannel(slot, (byte)(word & FormatMask), (int)(word >> OffsetShift) * OffsetUnit));
        }

        return streams;
    }
}
