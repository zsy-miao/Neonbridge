// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using FiveStar.Formats.Common;

namespace FiveStar.Formats.Audio;

/// <summary>
/// Splits the data chunk of a multichannel Opus stream into one packet list per channel. The chunk is a run of
/// blocks of <see cref="AwcStreamFormat.BlockSize"/> bytes. A block starts with 0x18 bytes per channel (u32 unknown,
/// u32 page count, u32 samples to skip, u32 samples, two u32 zero), then each channel's seek table (start sample of
/// each page on the stream's timeline), zero padding to 2 KiB, and each channel's pages in channel order. A block may
/// start before the previous one ended; packets that end before the channel's audio so far are dropped. Seek tables
/// count the samples of a stereo channel's two channels together.
/// </summary>
public static class AwcBlockReader
{
    private const int ChannelHeaderSize = 0x18;
    private const int PageCountOffset = 4;

    /// <summary>Fills one packet list per channel; returns why the chunk does not split cleanly, or null.</summary>
    public static string? Read(IByteSource source, AwcChunk data, long containerOffset, AwcStreamFormat format, List<AwcPacket>[] channels)
    {
        var ends = new long[channels.Length];
        long consumed = 0;

        for (var block = 0; block < format.BlockCount; block++)
        {
            var start = containerOffset + data.Offset + consumed;
            var size = (int)Math.Min(format.BlockSize, data.Size - consumed);

            if (size <= 0)
            {
                return "block past data";
            }

            var problem = ReadBlock(source.Read(start, size), start, format, channels, ends);

            if (problem is not null)
            {
                return problem;
            }

            consumed += size;
        }

        if (consumed != data.Size)
        {
            return "data past blocks";
        }

        return null;
    }

    private static string? ReadBlock(ReadOnlySpan<byte> block, long offset, AwcStreamFormat format, List<AwcPacket>[] channels, long[] ends)
    {
        var count = channels.Length;

        if (block.Length < count * ChannelHeaderSize)
        {
            return "block header";
        }

        var seekTables = new long[count];
        long tables = count * ChannelHeaderSize;

        for (var channel = 0; channel < count; channel++)
        {
            if (PageCount(block, channel) < 0)
            {
                return "page count";
            }

            seekTables[channel] = tables;
            tables += (long)PageCount(block, channel) * sizeof(uint);
        }

        var pageStart = AlignUp(tables, OpusPageReader.PageSize);

        for (var channel = 0; channel < count; channel++)
        {
            var pagesSize = (long)PageCount(block, channel) * OpusPageReader.PageSize;

            if (pageStart + pagesSize > block.Length)
            {
                return "pages past block";
            }

            if (pagesSize == 0)
            {
                continue;
            }

            var interleaved = Math.Max((uint)format.Channels[channel].Format.Channels, 1u);
            var start = BinaryPrimitives.ReadUInt32LittleEndian(block[(int)seekTables[channel]..]) / interleaved;
            var pages = block.Slice((int)pageStart, (int)pagesSize);
            var problem = ReadChannel(pages, offset + pageStart, start, channels[channel], ref ends[channel]);

            if (problem is not null)
            {
                return problem;
            }

            pageStart += pagesSize;
        }

        return null;
    }

    private static string? ReadChannel(ReadOnlySpan<byte> pages, long offset, long start, List<AwcPacket> packets, ref long end)
    {
        var block = new List<AwcPacket>();

        if (!OpusPageReader.TryRead(pages, offset, block))
        {
            return "bad page";
        }

        var position = start;

        foreach (var packet in block)
        {
            if (position + packet.Samples <= end)
            {
                position += packet.Samples;

                continue;
            }

            if (position != end)
            {
                return "block gap";
            }

            packets.Add(packet);
            position += packet.Samples;
            end = position;
        }

        return null;
    }

    private static int PageCount(ReadOnlySpan<byte> block, int channel) =>
        BinaryPrimitives.ReadInt32LittleEndian(block[((channel * ChannelHeaderSize) + PageCountOffset)..]);

    private static long AlignUp(long value, int alignment) => (value + alignment - 1) / alignment * alignment;
}
