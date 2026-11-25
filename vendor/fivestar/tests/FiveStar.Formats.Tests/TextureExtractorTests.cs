// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using FiveStar.Formats.Common;
using FiveStar.Formats.Compression;
using FiveStar.Formats.Tests.Support;
using FiveStar.Formats.Ttpf;
using Xunit;

namespace FiveStar.Formats.Tests;

/// <summary>Texture extraction from a hand-built page file holding one R8 texture in a 4 KB tile.</summary>
public sealed class TextureExtractorTests
{
    private const int Size = 64;
    private const int R8Unorm = 61;

    [Fact]
    public void A_swizzled_texture_comes_out_as_a_row_major_dds()
    {
        using var source = new MemoryByteSource(PageFile(Size));
        var file = new TexturePageFile(source, new ZlibInflater());
        var extractor = new TextureExtractor(file);

        var (status, data, layout) = extractor.ReadMip0(file.Descriptors[0]);
        var dds = DdsWriter.Build(Size, Size, R8Unorm, data, layout!);

        // Pattern xxxxyyyyyxyx: element bit 4 is y bit 0, bit 9 is x bit 4, bit 10 y bit 5, bit 11 x bit 5.
        Assert.True(status.IsOk);
        Assert.Equal(1 % 251, data![(0 * Size) + 1]);
        Assert.Equal(16 % 251, data[(1 * Size) + 0]);
        Assert.Equal(512 % 251, data[(0 * Size) + 16]);
        Assert.Equal(1024 % 251, data[(32 * Size) + 0]);
        Assert.Equal(2048 % 251, data[(0 * Size) + 32]);

        Assert.Equal("DDS "u8.ToArray(), dds[..4]);
        Assert.Equal((uint)Size, BinaryPrimitives.ReadUInt32LittleEndian(dds.AsSpan(12)));
        Assert.Equal((uint)R8Unorm, BinaryPrimitives.ReadUInt32LittleEndian(dds.AsSpan(128)));
        Assert.Equal(data, dds[148..]);
    }

    [Fact]
    public void A_lone_mip_that_would_fit_the_mip_tail_is_read_from_the_start_of_its_tile()
    {
        const int small = 16;
        using var source = new MemoryByteSource(PageFile(small));
        var file = new TexturePageFile(source, new ZlibInflater());

        var (status, data, layout) = new TextureExtractor(file).ReadMip0(file.Descriptors[0]);

        // Without a tail the 16x16 mip uses the first 256 elements: element 16 is y bit 0, element 255 is (15, 15).
        Assert.True(status.IsOk);
        Assert.Equal(1, layout!.Images);
        Assert.Equal(16, data![(1 * small) + 0]);
        Assert.Equal(255 % 251, data[(15 * small) + 15]);
    }

    // RDS5 header and one page entry (plus sentinel), ETPF with one descriptor at 0x100, a stored 16 KiB page at
    // 0x200 whose last 4 KiB hold the texture (element e of the tile = e % 251).
    private static byte[] PageFile(int size)
    {
        var file = new ByteWriter().Bytes("RDS5"u8).U32(2).U32(0).U32(0).U32(0).U32(0);
        file.U32(0x200 >> 8).U32(0).U32(0).U32(0);
        file.PadTo(0x100).Bytes("ETPF"u8).U32(634).U32(1).U32(0x40);

        // Descriptor: size x size, one mip (d0 field 1, d1 bits 0-2 = 0), format R8 in d1 bits 3-9, 2D (kind 2 in
        // bits 20-23), layout mode 1 (4 KB tiles), address 0x3000.
        var extent = (uint)size - 1;
        file.U32(extent | (extent << 14) | (1u << 28)).U32((61u << 3) | (2u << 20) | (1u << 24)).U32(3).U32(0);
        file.U64(0x0123456789ABCDEF);
        file.PadTo(0x200).Bytes(new byte[0x3000]);

        for (var element = 0; element < 0x1000; element++)
        {
            file.U8((byte)(element % 251));
        }

        return file.ToArray();
    }
}
