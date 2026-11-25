// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Formats.Audio;

/// <summary>
/// Splits AWC Opus data into packets. The data is a run of 2 KiB pages; a page holds whole packets, each after a u16
/// length, and ends with zero padding (a zero length, or fewer than two bytes left).
/// </summary>
public static class OpusPageReader
{
    /// <summary>Size of a page.</summary>
    public const int PageSize = 0x800;

    private const int LengthSize = sizeof(ushort);

    /// <summary>
    /// Appends the packets of <paramref name="pages"/> (at archive offset <paramref name="offset"/>) to
    /// <paramref name="packets"/>; false when a packet crosses its page, has an invalid table of contents, or the
    /// padding is not zero.
    /// </summary>
    public static bool TryRead(ReadOnlySpan<byte> pages, long offset, List<AwcPacket> packets)
    {
        for (var start = 0; start < pages.Length; start += PageSize)
        {
            var page = pages.Slice(start, Math.Min(PageSize, pages.Length - start));

            if (!TryReadPage(page, offset + start, packets))
            {
                return false;
            }
        }

        return true;
    }

    private static bool TryReadPage(ReadOnlySpan<byte> page, long offset, List<AwcPacket> packets)
    {
        var position = 0;

        while (position + LengthSize <= page.Length)
        {
            var length = BinaryPrimitives.ReadUInt16LittleEndian(page[position..]);

            if (length == 0)
            {
                break;
            }

            var body = position + LengthSize;

            if (body + length > page.Length)
            {
                return false;
            }

            var samples = OpusPacket.SampleCount(page.Slice(body, length));

            if (samples < 0)
            {
                return false;
            }

            packets.Add(new AwcPacket(offset + body, length, samples));
            position = body + length;
        }

        return page[position..].IndexOfAnyExcept((byte)0) < 0;
    }
}
