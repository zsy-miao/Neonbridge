// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using FiveStar.Loader.Shims.Xg;
using FiveStar.Loader.Tests.Support;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>
/// xg_xs.dll texture computers against textures of the shipped package. The sizes are the spans the textures occupy
/// in their page files (ttdb_interiors_1_3.ttpf #161 and ttdb_graphics.ttpf #736); the dump tests detile those
/// textures through the computer and re-tile them byte for byte.
/// </summary>
public sealed unsafe class XgTests
{
    private const int Ok = 0;
    private const int GetResourceLayoutSlot = 2;
    private const int GetResourceSizeBytesSlot = 3;
    private const int GetMipLevelOffsetBytesSlot = 5;
    private const int GetTexelElementOffsetBytesSlot = 6;
    private const int CopyIntoSubresourceSlot = 8;
    private const int CopyFromSubresourceSlot = 9;
    private const int GetResourceTilingSlot = 10;
    private const int ReleaseSlot = 1;

    private const uint Bc1 = 71;
    private const uint Large64KbRX = 0x100 + 27;
    private const uint Small4KbRX = 0x100 + 23;

    /// <summary>A 2048x2048 BC1 texture with 7 mips in 64 KB blocks: mips 3-6 share the tail block.</summary>
    private static readonly XgResourceDesc LargeTexture = Texture(2048, 2048, 7, Large64KbRX);

    /// <summary>A 256x128 BC1 texture with 8 mips in 4 KB blocks: mips 2-7 share the tail block.</summary>
    private static readonly XgResourceDesc SmallTexture = Texture(256, 128, 8, Small4KbRX);

    [Fact]
    public void A_64_kb_texture_gets_the_size_and_mip_places_of_the_shipped_one()
    {
        using var test = TestRuntime.Create();
        nint computer = Create(test, LargeTexture);
        var layout = new byte[XgLayoutWriter.LayoutBytes];

        fixed (byte* buffer = layout)
        {
            Assert.Equal(Ok, ((delegate* unmanaged<nint, byte*, int>)Slot(computer, GetResourceLayoutSlot))(computer, buffer));
        }

        // Mip 0 (512x512 blocks) ends the texture; mip 1 follows the tail block and mip 2; mip 3 is the first tail mip,
        // half a block in. Mip entries start at 0x18 + 0x38 and are 0x70 bytes apart.
        Assert.Equal(0x2b0000ul, U64(layout, 0x00));
        Assert.Equal(0x2b0000ul, U64(layout, 0x18 + 0x10));
        Assert.Equal(0xb0000ul, U64(layout, 0x50 + 0x08));
        Assert.Equal(0x200000ul, U64(layout, 0x50 + 0x00));

        Assert.Equal(4096u, BinaryPrimitives.ReadUInt32LittleEndian(layout.AsSpan(0x50 + 0x1c)));
        Assert.Equal(0x10000ul + 0x20000, U64(layout, 0x50 + 0x70 + 0x08));
        Assert.Equal(0x8000ul, U64(layout, 0x50 + (3 * 0x70) + 0x08));

        uint tiles;
        XgPackedMipInfo packed;
        XgTileShape shape;
        uint count = 2;
        var tilings = stackalloc XgSubresourceTiling[2];
        ((delegate* unmanaged<nint, uint*, XgPackedMipInfo*, XgTileShape*, uint*, uint, XgSubresourceTiling*, void>)Slot(computer, GetResourceTilingSlot))(computer, &tiles, &packed, &shape, &count, 0, tilings);

        Assert.Equal(43u, tiles);
        Assert.Equal((3, 4, 1u, 0u), (packed.NumStandardMips, packed.NumPackedMips, packed.NumTilesForPackedMips, packed.StartTileIndexInOverallResource));
        Assert.Equal((512u, 256u), (shape.WidthInTexels, shape.HeightInTexels));
        Assert.Equal((4u, (ushort)8, 11u), (tilings[0].WidthInTiles, tilings[0].HeightInTiles, tilings[0].StartTileIndexInOverallResource));
        Assert.Equal(0u, ((delegate* unmanaged<nint, uint>)Slot(computer, ReleaseSlot))(computer));
    }

    [Fact]
    public void A_volume_in_a_4_kb_s_mode_is_laid_out_in_the_shipped_bricks()
    {
        // 256x32x8 R16_FLOAT in SW_4KB_S: 8x16x16 bricks (x, z, y, z, y, x, ...), so the 8 slices pad to 16: 0x40000
        // bytes, the size of the game's archive chunk for it (the 2D-slice layout gave 0x20000).
        var desc = new XgResourceDesc { Dimension = 4, Width = 256, Height = 32, DepthOrArraySize = 8, MipLevels = 1, Format = 54, SampleCount = 1, Layout = 0x105 };
        XgLayoutBuilder.Build(desc, out var surface);
        ref readonly XgPlane plane = ref surface.Planes[0];

        Assert.Equal(0x40000ul, surface.Size);
        Assert.Equal((2ul, 4ul, 8ul), (XgAddress.Element(plane, 0, 0, 1, 0), XgAddress.Element(plane, 0, 1, 0, 0), XgAddress.Element(plane, 0, 0, 0, 1)));
        Assert.Equal((0x1000ul, 0x20000ul), (XgAddress.Element(plane, 0, 0, 8, 0), XgAddress.Element(plane, 0, 0, 0, 16)));
    }

    [Fact]
    public void Element_offsets_follow_the_64_kb_blocks_and_the_tail_slot()
    {
        using var test = TestRuntime.Create();
        nint computer = Create(test, LargeTexture);
        var mipOffset = (delegate* unmanaged<nint, uint, uint, ulong>)Slot(computer, GetMipLevelOffsetBytesSlot);

        // Mip 0 starts at 0xb0000 with 4 blocks of 128x64 elements per row; mip 3 is the first tail mip, half a block in.
        Assert.Equal(0x2b0000ul, ((delegate* unmanaged<nint, ulong>)Slot(computer, GetResourceSizeBytesSlot))(computer));
        Assert.Equal(0xb0000ul, mipOffset(computer, 0, 0));

        Assert.Equal(0xb0000ul, TexelOffset(computer, 0, 0, 0));
        Assert.Equal(0xc0000ul, TexelOffset(computer, 0, 128, 0));
        Assert.Equal(0xf0000ul, TexelOffset(computer, 0, 0, 64));
        Assert.Equal(0x8000ul, TexelOffset(computer, 3, 0, 0));
        Assert.Equal(0ul, TexelOffset(computer, 7, 0, 0));
        ((delegate* unmanaged<nint, uint>)Slot(computer, ReleaseSlot))(computer);
    }

    [Fact]
    public void A_4_kb_texture_gets_the_size_of_the_shipped_one_and_unknown_layouts_get_a_suggestion()
    {
        using var test = TestRuntime.Create();
        var suggest = (delegate* unmanaged<XgResourceDesc*, uint, uint*, int>)test.Shim(XgModule.Module, "XGSuggestSwizzleMode");
        nint computer = Create(test, SmallTexture);
        var layout = new byte[XgLayoutWriter.LayoutBytes];
        var unknown = SmallTexture with { Layout = 0 };
        uint suggested;

        fixed (byte* buffer = layout)
        {
            Assert.Equal(Ok, ((delegate* unmanaged<nint, byte*, int>)Slot(computer, GetResourceLayoutSlot))(computer, buffer));
        }

        Assert.Equal(0x6000ul, U64(layout, 0x00));
        Assert.Equal(0x2000ul, U64(layout, 0x50 + 0x08));
        Assert.Equal(Ok, suggest(&unknown, 0, &suggested));
        Assert.Equal(Small4KbRX, suggested);
        ((delegate* unmanaged<nint, uint>)Slot(computer, ReleaseSlot))(computer);
    }

    [DumpFact]
    public void The_shipped_64_kb_texture_detiles_into_matching_mips_and_tiles_back_byte_for_byte()
    {
        DetileAndRetile(Path.Combine("data", "ttdb", "interiors", "ttdb_interiors_1_3.ttpf"), 161, LargeTexture);
    }

    [DumpFact]
    public void The_shipped_4_kb_texture_detiles_into_matching_mips_and_tiles_back_byte_for_byte()
    {
        DetileAndRetile(Path.Combine("data", "ttdb", "graphics", "ttdb_graphics.ttpf"), 736, SmallTexture);
    }

    /// <summary>
    /// Each mip read through CopyFromSubresource must look like the previous mip shrunk by half (BC1 endpoint colour
    /// correlation above 0.85, about 0 with a wrong tail slot), and CopyIntoSubresource of all mips must rebuild the
    /// shipped bytes of mip 0.
    /// </summary>
    private static void DetileAndRetile(string relative, int index, XgResourceDesc desc)
    {
        using var file = new TexturePageSample(Path.Combine(DumpFactAttribute.Folder!, relative));
        byte[] shipped = file.Texture(index)!;

        using var test = TestRuntime.Create();
        nint computer = Create(test, desc);

        var copyFrom = ((delegate* unmanaged<nint, byte*, uint, uint, uint, byte*, uint, uint, int>)Slot(computer, CopyFromSubresourceSlot));
        var copyInto = ((delegate* unmanaged<nint, byte*, uint, uint, byte*, uint, uint, int>)Slot(computer, CopyIntoSubresourceSlot));
        var rebuilt = new byte[shipped.Length];
        byte[]? previous = null;

        for (uint mip = 0; mip < desc.MipLevels; mip++)
        {
            int width = Math.Max(1, ((int)desc.Width >> (int)mip) / 4);
            int height = Math.Max(1, ((int)desc.Height >> (int)mip) / 4);
            var linear = new byte[width * height * 8];

            fixed (byte* tiled = shipped, target = rebuilt, rows = linear)
            {
                Assert.Equal(Ok, copyFrom(computer, rows, 0, mip, 0, tiled, (uint)width * 8, (uint)linear.Length));
                Assert.Equal(Ok, copyInto(computer, target, 0, mip, rows, (uint)width * 8, (uint)linear.Length));
            }

            AssertElementsAt(computer, shipped, linear, mip, width, height);

            if (previous is not null && width >= 4 && height >= 4)
            {
                Assert.True(Correlation(previous, linear, width, height) > 0.85, $"mip {mip} does not match mip {mip - 1}");
            }

            previous = linear;
        }

        int mip0 = (int)desc.Width / 4 * ((int)desc.Height / 4) * 8;
        Assert.Equal(shipped.AsSpan(shipped.Length - mip0).ToArray(), rebuilt.AsSpan(rebuilt.Length - mip0).ToArray());
        ((delegate* unmanaged<nint, uint>)Slot(computer, ReleaseSlot))(computer);
    }

    /// <summary>Every BC1 block of a read-back mip sits in the shipped bytes at its GetTexelElementOffsetBytes offset.</summary>
    private static void AssertElementsAt(nint computer, byte[] shipped, byte[] linear, uint mip, int width, int height)
    {
        for (int y = 0; y < height; y++)
        {
            for (int x = 0; x < width; x++)
            {
                int offset = (int)TexelOffset(computer, mip, (ulong)x, (uint)y);

                if (!shipped.AsSpan(offset, 8).SequenceEqual(linear.AsSpan(((y * width) + x) * 8, 8)))
                {
                    Assert.Fail($"mip {mip} element ({x}, {y}) is not at offset 0x{offset:x}");
                }
            }
        }
    }

    /// <summary>Correlation of each block's endpoint brightness with the mean of the four blocks it shrinks.</summary>
    private static double Correlation(byte[] larger, byte[] smaller, int width, int height)
    {
        var small = new List<double>();
        var shrunk = new List<double>();

        for (int y = 0; y < height; y++)
        {
            for (int x = 0; x < width; x++)
            {
                small.Add(Brightness(smaller, (y * width) + x));
                shrunk.Add((Brightness(larger, (2 * y * 2 * width) + (2 * x)) + Brightness(larger, (2 * y * 2 * width) + (2 * x) + 1)
                    + Brightness(larger, (((2 * y) + 1) * 2 * width) + (2 * x)) + Brightness(larger, (((2 * y) + 1) * 2 * width) + (2 * x) + 1)) / 4);
            }
        }

        double meanA = small.Average();
        double meanB = shrunk.Average();
        double ab = small.Zip(shrunk, (a, b) => (a - meanA) * (b - meanB)).Sum();
        double aa = small.Sum(a => (a - meanA) * (a - meanA));
        double bb = shrunk.Sum(b => (b - meanB) * (b - meanB));

        return ab / Math.Sqrt(aa * bb);
    }

    /// <summary>Sum of a BC1 block's two RGB565 endpoints, green counted once and red and blue twice.</summary>
    private static double Brightness(byte[] blocks, int block)
    {
        double total = 0;

        for (int endpoint = 0; endpoint < 2; endpoint++)
        {
            int color = BinaryPrimitives.ReadUInt16LittleEndian(blocks.AsSpan((block * 8) + (endpoint * 2)));
            total += (((color >> 11) & 31) * 2) + ((color >> 5) & 63) + ((color & 31) * 2);
        }

        return total;
    }

    private static XgResourceDesc Texture(ulong width, uint height, ushort mips, uint layout)
    {
        return new XgResourceDesc { Dimension = 3, Width = width, Height = height, DepthOrArraySize = 1, MipLevels = mips, Format = Bc1, SampleCount = 1, Layout = layout };
    }

    private static nint Create(TestRuntime test, XgResourceDesc desc)
    {
        var create = (delegate* unmanaged<XgResourceDesc*, nint*, int>)test.Shim(XgModule.Module, "XGCreateTextureComputer");
        nint computer;

        Assert.Equal(Ok, create(&desc, &computer));

        return computer;
    }

    /// <summary>Slot 6 as the game calls it (0x145151e7d): plane 0, mip, x, y, slice 0, sample 0, no bit offset.</summary>
    private static ulong TexelOffset(nint computer, uint mip, ulong x, uint y)
    {
        var texel = (delegate* unmanaged<nint, uint, uint, ulong, uint, uint, uint, uint*, ulong>)Slot(computer, GetTexelElementOffsetBytesSlot);

        return texel(computer, 0, mip, x, y, 0, 0, null);
    }

    private static nint Slot(nint computer, int slot)
    {
        return (*(nint**)computer)[slot];
    }

    private static ulong U64(byte[] data, int offset)
    {
        return BinaryPrimitives.ReadUInt64LittleEndian(data.AsSpan(offset));
    }
}
