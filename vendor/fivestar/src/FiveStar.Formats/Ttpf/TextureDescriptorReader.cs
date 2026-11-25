// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Formats.Ttpf;

/// <summary>Decodes the 16-byte <c>ETPF</c> descriptor <c>{u32 d0, u32 d1, u32 o0, u32 o1}</c>.</summary>
public static class TextureDescriptorReader
{
    private const int D1Field = 4;
    private const int O0Field = 8;
    private const int O1Field = 12;
    private const uint DimensionMask = 0x3FFF;
    private const int HeightShift = 14;
    private const int MipShift = 28;

    /// <summary>d1 bits 0-2 add to the d0 mip field; the sum is the stored mip count (docs/textures.md).</summary>
    private const uint ExtraMipMask = 7;
    private const int FormatShift = 3;
    private const uint FormatMask = 0x7F;
    private const uint AddressMask = 0x1FFFFF;
    private const long AddressUnit = 0x1000;

    public static TextureDescriptor Read(ReadOnlySpan<byte> data, int index)
    {
        var d0 = BinaryPrimitives.ReadUInt32LittleEndian(data);
        var d1 = BinaryPrimitives.ReadUInt32LittleEndian(data[D1Field..]);
        var o0 = BinaryPrimitives.ReadUInt32LittleEndian(data[O0Field..]);
        var o1 = BinaryPrimitives.ReadUInt32LittleEndian(data[O1Field..]);

        return new TextureDescriptor(
            index,
            (int)(d0 & DimensionMask) + 1,
            (int)((d0 >> HeightShift) & DimensionMask) + 1,
            (int)(d0 >> MipShift) + (int)(d1 & ExtraMipMask),
            d1,
            (int)((d1 >> FormatShift) & FormatMask),
            (o0 & AddressMask) * AddressUnit,
            o0,
            o1);
    }
}
