// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Common;
using FiveStar.Formats.Compression;
using FiveStar.Formats.Tests.Support;
using FiveStar.Formats.Ttpf;
using FiveStar.Formats.Ttpf.BcPack;
using Xunit;

namespace FiveStar.Formats.Tests;

/// <summary>BCPack pages of the dump: every codec converges to the same symbols on every engine, and with the user's tables to blocks.</summary>
public sealed class BcPackDumpTests
{
    private const int PagesPerCodec = 3;

    [DumpFact]
    public void Pages_of_every_codec_converge_to_the_same_symbols_on_every_engine()
    {
        var engines = Enum.GetValues<BcPackEngine>().Where(BcPackDecoder.IsSupported).ToList();

        foreach (var (codec, page, stored) in SamplePages())
        {
            var count = BcPackDecoder.SymbolsPerLane(codec, page.VirtualSize);
            var expected = new ushort[BcPackDecoder.Lanes(codec) * count];
            var scalar = BcPackDecoder.DecodeSymbols(stored, codec, count, expected, BcPackEngine.Scalar);

            Assert.True(scalar.Success, $"{codec} page at {page.FileOffset:x}: {scalar.Describe()}");

            foreach (var engine in engines)
            {
                var symbols = new ushort[expected.Length];
                var result = BcPackDecoder.DecodeSymbols(stored, codec, count, symbols, engine);

                Assert.Equal(scalar, result);
                Assert.Equal(expected, symbols);
            }
        }
    }

    [TablesFact]
    public void With_the_tables_pages_of_every_codec_decode_to_valid_blocks_on_every_engine()
    {
        var tables = BcPackTables.Load(TablesFactAttribute.Path!);
        var engines = Enum.GetValues<BcPackEngine>().Where(BcPackDecoder.IsSupported).ToList();

        foreach (var (codec, page, stored) in SamplePages())
        {
            var expected = new byte[page.VirtualSize];
            var scalar = BcPackPageDecoder.Decode((int)codec, stored, page.VirtualSize, expected, tables, BcPackEngine.Scalar);

            Assert.True(scalar.Success, $"{codec} page at {page.FileOffset:x}: {scalar.Describe()}");
            Assert.Contains(expected, value => value != 0);

            foreach (var engine in engines)
            {
                var blocks = new byte[page.VirtualSize];

                Assert.True(BcPackPageDecoder.Decode((int)codec, stored, page.VirtualSize, blocks, tables, engine).Success);
                Assert.Equal(expected, blocks);
            }
        }
    }

    [Fact]
    public void A_table_file_with_other_content_is_refused()
    {
        var other = new byte[BcPackTables.FileSize];

        Assert.NotNull(BcPackTables.Check(other));
        Assert.NotNull(BcPackTables.Check(other.AsSpan(1)));
        Assert.Throws<InvalidDataException>(() => BcPackTables.Parse(other));
    }

    /// <summary>The first pages of each codec in the dump's page files.</summary>
    private static List<(BcPackCodec Codec, TexturePage Page, byte[] Stored)> SamplePages()
    {
        var found = new List<(BcPackCodec Codec, TexturePage Page, byte[] Stored)>();
        var files = Directory.EnumerateFiles(DumpFactAttribute.Folder!, "*.ttpf", SearchOption.AllDirectories).Order(StringComparer.Ordinal);

        foreach (var path in files)
        {
            using var source = new FileByteSource(path);
            var file = new TexturePageFile(source, new ZlibInflater());

            foreach (var page in file.Pages.Where(page => page.IsBcPack))
            {
                var codec = (BcPackCodec)page.Codec;

                if (found.Count(entry => entry.Codec == codec) < PagesPerCodec)
                {
                    found.Add((codec, page, source.Read(page.FileOffset, (int)page.FileLength).ToArray()));
                }
            }
        }

        Assert.Equal(Enum.GetValues<BcPackCodec>().Length * PagesPerCodec, found.Count);

        return found;
    }
}
