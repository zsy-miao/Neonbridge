// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using FiveStar.Formats.Audio;
using FiveStar.Formats.Common;
using FiveStar.Formats.Tests.Support;
using Xunit;

namespace FiveStar.Formats.Tests;

/// <summary>An AWC container built in memory, read back and wrapped in Ogg Opus.</summary>
public sealed class AwcTests
{
    private const uint StreamId = 0x0ABCDEF;
    private const byte CeltFullBand20Ms = 0xF8;
    private const int PacketSamples = 960;
    private const uint Frames = 2000;
    private const int TableEnd = 36;
    private const int FormatAt = TableEnd;
    private const int DataAt = 0x800;

    [Fact]
    public void A_single_opus_stream_reads_into_packets_and_wraps_into_valid_ogg_pages()
    {
        var container = BuildContainer();
        using var source = new MemoryByteSource(PlaceAt(container, 0x1000));

        var found = new AwcScanner(source).Scan(0).Single();
        var track = new AwcTrackReader(source).Read(found).Single();
        using var ogg = new MemoryStream();
        OggOpusWriter.Write(ogg, source, track, 1);
        var pages = OggPages(ogg.ToArray());

        Assert.Equal(0x1000, found.Offset);
        Assert.Equal(DataAt + (2 * OpusPageReader.PageSize), found.Size);
        Assert.Null(track.Problem);
        Assert.Equal(StreamId, track.Id);

        Assert.Equal(AwcCodec.Opus, track.Codec);
        Assert.Equal([40, 200, 7], track.Packets.Select(packet => packet.Length).ToArray());
        Assert.Equal(3 * PacketSamples, track.PacketSamples);

        Assert.Equal(3, pages.Count);
        Assert.Equal("OpusHead", System.Text.Encoding.ASCII.GetString(pages[0].Body, 0, 8));
        Assert.Equal((long)(OggOpusWriter.PreSkip + Frames), pages[^1].Granule);
        Assert.All(pages, page => Assert.True(page.CrcMatches));
    }

    [Fact]
    public void A_header_whose_tables_do_not_add_up_is_refused()
    {
        var container = BuildContainer();
        BinaryPrimitives.WriteUInt32LittleEndian(container.AsSpan(12), TableEnd + 8);
        using var source = new MemoryByteSource(container);

        Assert.Null(AwcReader.TryRead(source, 0));
        Assert.Empty(new AwcScanner(source).Scan(0));
    }

    private static byte[] BuildContainer()
    {
        var writer = new ByteWriter()
            .Bytes("ADAT"u8).U16(1).U16(0xFF00).U32(1).U32(TableEnd)
            .U32(StreamId | (2u << 29))
            .U64(Chunk(AwcChunkTag.Format, FormatAt, AwcFormat.ChunkSize))
            .U64(Chunk(AwcChunkTag.Data, DataAt, 2 * OpusPageReader.PageSize));

        writer.U32(Frames).U32(uint.MaxValue).U16(48000).U16(0).PadTo(FormatAt + 0x1C).U8((byte)AwcCodec.Opus).U8(1).U16(0);
        writer.PadTo(DataAt);

        Packet(writer, 40);
        Packet(writer, 200);
        writer.PadTo(DataAt + OpusPageReader.PageSize);
        Packet(writer, 7);
        writer.PadTo(DataAt + (2 * OpusPageReader.PageSize));

        return writer.ToArray();
    }

    private static ulong Chunk(AwcChunkTag tag, int offset, int size) =>
        ((ulong)tag << 56) | ((ulong)size << 28) | (uint)offset;

    private static void Packet(ByteWriter writer, int length)
    {
        writer.U16((ushort)length).U8(CeltFullBand20Ms);

        for (var index = 1; index < length; index++)
        {
            writer.U8((byte)index);
        }
    }

    private static byte[] PlaceAt(byte[] container, int offset)
    {
        var data = new byte[offset + container.Length];
        container.CopyTo(data, offset);

        return data;
    }

    private static List<OggPage> OggPages(byte[] file)
    {
        var pages = new List<OggPage>();
        var position = 0;

        while (position < file.Length)
        {
            var segments = file[position + 26];
            var bodySize = file.AsSpan(position + 27, segments).ToArray().Sum(value => value);
            var size = 27 + segments + bodySize;
            var page = file.AsSpan(position, size).ToArray();
            var stored = BinaryPrimitives.ReadUInt32LittleEndian(page.AsSpan(22));
            page.AsSpan(22, 4).Clear();

            pages.Add(new OggPage(
                BinaryPrimitives.ReadInt64LittleEndian(page.AsSpan(6)),
                page.AsSpan(27 + segments).ToArray(),
                OggCrc.Compute(page) == stored));
            position += size;
        }

        return pages;
    }

    private sealed record OggPage(long Granule, byte[] Body, bool CrcMatches);
}
