// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Common;
using FiveStar.Formats.Compression;
using FiveStar.Formats.Rpf;
using FiveStar.Formats.Tests.Support;
using Xunit;

namespace FiveStar.Formats.Tests;

/// <summary>The carver on a hand-built archive data area.</summary>
public sealed class ZlibEntryCarverTests
{
    private const int Chunk = 0x10000;

    [Fact]
    public void An_archive_carves_into_its_entries_with_raw_pso_tails_cut_at_the_declared_size()
    {
        var (archive, text, pso, xml, offsets) = Archive();
        using var source = new MemoryByteSource(archive);
        var carver = new ZlibEntryCarver(source, new ZlibStreamProbe(source, new ZlibInflater()));

        var blobs = carver.Carve(0).ToList();

        Assert.Equal(offsets, blobs.Select(blob => blob.Offset));
        Assert.Equal([BlobKind.Txt, BlobKind.Psin, BlobKind.Xml], blobs.Select(blob => blob.Kind));
        Assert.Equal(text, blobs[0].Data);
        Assert.Equal(pso, blobs[1].Data);
        Assert.Equal(1, blobs[1].RawChunks);
        Assert.Equal(xml, blobs[2].Data);
    }

    [Fact]
    public void A_file_mapped_in_windows_smaller_than_its_entries_carves_the_same()
    {
        var (archive, text, pso, xml, _) = Archive();
        var file = Path.GetTempFileName();

        try
        {
            File.WriteAllBytes(file, archive);
            using var source = new WindowedFileSource(file, Chunk);
            var carver = new ZlibEntryCarver(source, new ZlibStreamProbe(source, new ZlibInflater()));

            Assert.Equal([text, pso, xml], carver.Carve(0).Select(blob => blob.Data));
        }
        finally
        {
            File.Delete(file);
        }
    }

    // A text entry of two zlib chunks, a PSO file whose incompressible tail is stored raw, an XML entry.
    private static (byte[] Archive, byte[] Text, byte[] Pso, byte[] Xml, long[] Offsets) Archive()
    {
        var text = Deflate.Pattern(Chunk + 1000);
        var pso = PsoFile(Chunk + 0x300);
        var xml = "<?xml version=\"1.0\"?><a/>"u8.ToArray();

        var archive = new ByteWriter().PadTo(0x10);
        var textAt = AddEntry(archive, text);
        long psoAt = archive.Position;
        archive.Bytes(Deflate.Zlib(pso[..Chunk])).Align(16, 0).Bytes(pso.AsSpan(Chunk)).Align(16, 0);
        var xmlAt = AddEntry(archive, xml);
        archive.PadTo(archive.Position + Chunk);

        return (archive.ToArray(), text, pso, xml, [textAt, psoAt, xmlAt]);
    }

    private static long AddEntry(ByteWriter archive, byte[] data)
    {
        long start = archive.Position;

        for (var offset = 0; offset < data.Length; offset += Chunk)
        {
            var chunk = data.AsSpan(offset, Math.Min(Chunk, data.Length - offset)).ToArray();
            archive.Bytes(Deflate.Zlib(chunk)).Align(16, 0);
        }

        return start;
    }

    // PSIN chunk of noise, then CHKS {BE total size, checksum, 'zppp'}.
    private static byte[] PsoFile(int total)
    {
        var file = new ByteWriter().Bytes("PSIN"u8).U32BigEndian((uint)(total - 20));
        file.Bytes(Deflate.Noise(total - 20 - 8, 99));
        file.Bytes("CHKS"u8).U32BigEndian(20).U32BigEndian((uint)total).U32(0x12345678).Bytes("zppp"u8);

        return file.ToArray();
    }
}
