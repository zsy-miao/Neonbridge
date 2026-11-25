// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Formats.Ttpf;

/// <summary>
/// Writes a one-mip DDS file with a DX10 header (layout from the DirectDraw Surface documentation:
/// <c>DDS_HEADER</c> of 124 bytes after the magic, then <c>DDS_HEADER_DXT10</c> of 20 bytes).
/// </summary>
public static class DdsWriter
{
    private const int HeaderSize = 124;
    private const int Dx10HeaderSize = 20;
    private const int MagicSize = 4;

    // DDS_HEADER field offsets from the start of the file.
    private const int SizeField = 4;
    private const int FlagsField = 8;
    private const int HeightField = 12;
    private const int WidthField = 16;
    private const int PitchField = 20;
    private const int DepthField = 24;
    private const int MipCountField = 28;
    private const int PixelFormatSizeField = 76;
    private const int PixelFormatFlagsField = 80;
    private const int FourCcField = 84;
    private const int CapsField = 108;
    private const int Caps2Field = 112;

    // DDSD_CAPS | DDSD_HEIGHT | DDSD_WIDTH | DDSD_PIXELFORMAT | DDSD_LINEARSIZE, plus DDSD_DEPTH for a volume.
    private const uint HeaderFlags = 0x1 | 0x2 | 0x4 | 0x1000 | 0x80000;
    private const uint DepthFlag = 0x800000;
    private const uint PixelFormatSize = 32;
    private const uint FourCcFlag = 0x4;
    private const uint CapsTexture = 0x1000;
    private const uint CapsComplex = 0x8;
    private const uint Caps2AllCubeFaces = 0xFE00;
    private const uint Caps2Volume = 0x200000;

    // DDS_HEADER_DXT10: format, dimension (3 = 2D, 4 = 3D), misc flag (4 = cube), array size, misc flags 2.
    private const uint Texture2D = 3;
    private const uint Texture3D = 4;
    private const uint MiscTextureCube = 0x4;
    private const int DimensionField = 4;
    private const int MiscField = 8;
    private const int ArraySizeField = 12;

    public static ReadOnlySpan<byte> Magic => "DDS "u8;

    /// <summary>A DDS file of mip 0: <paramref name="data"/> holds the images of <paramref name="layout"/> one after another.</summary>
    public static byte[] Build(int width, int height, int dxgiFormat, ReadOnlySpan<byte> data, TextureLayout layout)
    {
        var file = new byte[MagicSize + HeaderSize + Dx10HeaderSize + data.Length];
        var span = file.AsSpan();
        var (flags, depth, caps, caps2) = (HeaderFlags, 0u, CapsTexture, 0u);
        var (dimension, misc, arraySize) = (Texture2D, 0u, (uint)layout.Images);

        if (layout.Kind == TextureKind.Volume)
        {
            (flags, depth, caps, caps2, dimension) = (HeaderFlags | DepthFlag, (uint)layout.Depth, CapsTexture | CapsComplex, Caps2Volume, Texture3D);
        }
        else if (layout.Kind == TextureKind.Cube)
        {
            (caps, caps2, misc, arraySize) = (CapsTexture | CapsComplex, Caps2AllCubeFaces, MiscTextureCube, 1u);
        }

        Magic.CopyTo(span);
        WriteU32(span, SizeField, HeaderSize);
        WriteU32(span, FlagsField, flags);
        WriteU32(span, HeightField, (uint)height);
        WriteU32(span, WidthField, (uint)width);

        WriteU32(span, PitchField, (uint)(data.Length / layout.Images / layout.Depth));
        WriteU32(span, DepthField, depth);
        WriteU32(span, MipCountField, 1);
        WriteU32(span, PixelFormatSizeField, PixelFormatSize);
        WriteU32(span, PixelFormatFlagsField, FourCcFlag);

        "DX10"u8.CopyTo(span[FourCcField..]);
        WriteU32(span, CapsField, caps);
        WriteU32(span, Caps2Field, caps2);

        var dx10 = MagicSize + HeaderSize;
        WriteU32(span, dx10, (uint)dxgiFormat);
        WriteU32(span, dx10 + DimensionField, dimension);
        WriteU32(span, dx10 + MiscField, misc);
        WriteU32(span, dx10 + ArraySizeField, arraySize);
        data.CopyTo(span[(dx10 + Dx10HeaderSize)..]);

        return file;
    }

    private static void WriteU32(Span<byte> span, int offset, uint value)
    {
        BinaryPrimitives.WriteUInt32LittleEndian(span[offset..], value);
    }
}
