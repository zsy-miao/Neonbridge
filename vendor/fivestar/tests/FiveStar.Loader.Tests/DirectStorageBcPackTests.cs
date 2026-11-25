// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using System.Text.Json;
using FiveStar.Formats.Common;
using FiveStar.Formats.Compression;
using FiveStar.Formats.Ttpf;
using FiveStar.Formats.Ttpf.BcPack;
using FiveStar.Loader.Tests.Support;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>
/// DirectStorage requests of BCPack pages: the compression byte is the page's codec times two (BC1 2, BC7 14), and the
/// destination gets the page's BC blocks, or placeholder blocks for a codec without a block decoder. The tests run one
/// after another, so the one that installs the user's tables does not change what the others see.
/// </summary>
public sealed unsafe class DirectStorageBcPackTests
{
    private const int Bc7Block = 16;
    private const byte Bc7ModeMask = 0x7F;
    private const byte Bc7Mode6 = 0x40;

    [Fact]
    public void A_bc1_request_decodes_to_the_pages_blocks_and_a_bc7_request_gets_mode_6_placeholder_blocks()
    {
        var (chunk, blocks) = SyntheticBc1Page();
        byte[] expected = new byte[blocks * Bc1Decoder.BlockSize];
        Assert.True(BcPackPageDecoder.Decode((int)BcPackCodec.Bc1, chunk, expected.Length, expected).Success);

        using var test = TestRuntime.Create();
        string path = Path.Combine(test.Folder, "page.ttpf");
        File.WriteAllBytes(path, chunk);

        byte[] bc1 = new byte[expected.Length];
        byte[] bc7 = new byte[0x1000];
        int result = Run(test, path, [(BcPackCodec.Bc1, bc1), (BcPackCodec.Bc7, bc7)], (uint)chunk.Length);

        Assert.Equal(0, result);
        Assert.Equal(expected, bc1);
        AssertPlaceholder(bc7);
    }

    [DumpFact]
    public void The_bc7_page_of_ttdb_graphics_completes_with_placeholders_and_its_first_bc1_page_matches_the_texture_reader()
    {
        const long Bc7PageOffset = 0x8dbd00;
        string path = Path.Combine(DumpFactAttribute.Folder!, "data", "ttdb", "graphics", "ttdb_graphics.ttpf");

        using var source = new FileByteSource(path);
        var pages = new TexturePageFile(source, new ZlibInflater());
        int bc7Index = Index(pages, page => page.FileOffset == Bc7PageOffset && page.Codec == (int)BcPackCodec.Bc7);
        int bc1Index = Index(pages, page => page.Codec == (int)BcPackCodec.Bc1 && page.IsDecodable);
        var bc7Page = pages.Pages[bc7Index];
        var bc1Page = pages.Pages[bc1Index];

        using var test = TestRuntime.Create();
        byte[] bc7 = new byte[bc7Page.VirtualSize];
        byte[] bc1 = new byte[bc1Page.VirtualSize];
        int result = RunPage(test, path, bc7Page, bc7);
        int bc1Result = RunPage(test, path, bc1Page, bc1);

        Assert.Equal(0, result);
        AssertPlaceholder(bc7);
        Assert.Equal(0, bc1Result);
        Assert.Equal(pages.PageData(bc1Index), bc1);
    }

    [TablesFact]
    public void With_the_bcpack_tables_the_bc7_page_of_ttdb_graphics_gets_its_real_blocks()
    {
        const long Bc7PageOffset = 0x8dbd00;

        string path = Path.Combine(DumpFactAttribute.Folder!, "data", "ttdb", "graphics", "ttdb_graphics.ttpf");
        using var source = new FileByteSource(path);
        var pages = new TexturePageFile(source, new ZlibInflater());
        var page = pages.Pages[Index(pages, page => page.FileOffset == Bc7PageOffset && page.Codec == (int)BcPackCodec.Bc7)];
        var tables = BcPackTables.Load(TablesFactAttribute.Path!);
        byte[] expected = new byte[page.VirtualSize];

        Assert.True(BcPackPageDecoder.Decode(page.Codec, source.Read(page.FileOffset, (int)page.FileLength).ToArray(), page.VirtualSize, expected, tables, BcPackEngine.Scalar).Success);

        using var test = TestRuntime.Create();
        byte[] bc7 = new byte[page.VirtualSize];
        BcPackPageDecoder.Install(tables);

        try
        {
            Assert.Equal(0, RunPage(test, path, page, bc7));
        }
        finally
        {
            BcPackPageDecoder.Install(null);
        }

        Assert.Equal(expected, bc7);
    }

    /// <summary>One queue: each request reads the same stored range as a page of its codec, then one status entry.</summary>
    private static int Run(TestRuntime test, string path, (BcPackCodec Codec, byte[] Output)[] requests, uint size, ulong offset = 0)
    {
        var api = StorageApi.Create(test);
        nint file = api.OpenFile(path);
        nint statuses = api.CreateStatusArray(1);
        nint queue = api.CreateQueue();
        var pins = requests.Select(request => GCHandle.Alloc(request.Output, GCHandleType.Pinned)).ToArray();

        for (int i = 0; i < requests.Length; i++)
        {
            var destination = (byte*)pins[i].AddrOfPinnedObject();
            byte compression = (byte)((int)requests[i].Codec << 1);
            StorageApi.EnqueueRead(queue, file, offset, size, destination, (uint)requests[i].Output.Length, compression);
        }

        StorageApi.EnqueueStatus(queue, statuses, 0);
        StorageApi.Submit(queue);
        int result = StorageApi.WaitStatus(statuses, 0);

        foreach (var pin in pins)
        {
            pin.Free();
        }

        StorageApi.Release(queue);
        StorageApi.Release(file);
        StorageApi.Release(statuses);

        return result;
    }

    private static int RunPage(TestRuntime test, string path, TexturePage page, byte[] output) =>
        Run(test, path, [((BcPackCodec)page.Codec, output)], (uint)page.FileLength, (ulong)page.FileOffset);

    private static int Index(TexturePageFile pages, Func<TexturePage, bool> match)
    {
        for (int i = 0; i < pages.Pages.Count; i++)
        {
            if (match(pages.Pages[i]))
            {
                return i;
            }
        }

        throw new InvalidOperationException("no such page in ttdb_graphics.ttpf");
    }

    /// <summary>Every block is the same BC7 mode 6 block.</summary>
    private static void AssertPlaceholder(byte[] blocks)
    {
        Assert.Equal(Bc7Mode6, blocks[0] & Bc7ModeMask);

        for (int at = Bc7Block; at < blocks.Length; at += Bc7Block)
        {
            Assert.True(blocks.AsSpan(at, Bc7Block).SequenceEqual(blocks.AsSpan(0, Bc7Block)), $"block at 0x{at:x}");
        }
    }

    /// <summary>The smallest synthetic BC1 stream of bcpack-rs (vendor/bcpack-rs/tests/vectors).</summary>
    private static (byte[] Chunk, int Blocks) SyntheticBc1Page()
    {
        string file = Directory.GetFiles(Path.Combine(AppContext.BaseDirectory, "BcPackVectors"), "*.json").OrderBy(name => new FileInfo(name).Length).First();
        using var document = JsonDocument.Parse(File.ReadAllText(file));
        var root = document.RootElement;

        return (Convert.FromHexString(root.GetProperty("chunk_hex").GetString()!), root.GetProperty("blocks").GetInt32());
    }
}
