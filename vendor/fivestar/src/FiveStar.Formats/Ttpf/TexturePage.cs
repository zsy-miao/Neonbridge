// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Ttpf.BcPack;

namespace FiveStar.Formats.Ttpf;

/// <summary>
/// One page of a texture page file. Page-table entry (two u32): <c>lo = flag &lt;&lt; 24 | file offset / 0x100</c>,
/// <c>hi = tag &lt;&lt; 20 | virtual address / 0x4000</c>; flag bits 6-7 add 0x1000 steps to the virtual address,
/// bit 5 marks zlib, bits 2-4 name the codec (0 stored, 1/3/4/5/6/7 BCPack for BC1/BC3/BC4/BC5/BC6H/BC7, <see cref="BcPackCodec"/>).
/// </summary>
/// <param name="VirtualAddress">Where the page's bytes sit in the texture address space.</param>
/// <param name="Flag">The flag byte.</param>
/// <param name="FileOffset">File offset of the stored bytes.</param>
/// <param name="FileLength">Stored bytes, up to the next page's offset (or the end of the file).</param>
/// <param name="VirtualSize">Distance to the next table entry's address (0 for the last; can be negative).</param>
/// <param name="Tag">The top 12 bits of <c>hi</c>.</param>
public sealed record TexturePage(long VirtualAddress, byte Flag, long FileOffset, long FileLength, long VirtualSize, uint Tag)
{
    /// <summary>The virtual size assumed for a page whose size is unknown.</summary>
    public const int DefaultVirtualSize = 0x4000;

    /// <summary>The codec of BCPack BC1 pages (DSTORAGE_BCPACK_MODE BC1).</summary>
    public const int Bc1Codec = 1;

    private const int ZlibFlag = 0x20;
    private const int CodecShift = 2;
    private const int CodecMask = 7;

    /// <summary>Gets a value indicating whether the stored bytes are a zlib stream.</summary>
    public bool IsZlib => (Flag & ZlibFlag) != 0;

    /// <summary>Gets the codec number (0 = stored).</summary>
    public int Codec => (Flag >> CodecShift) & CodecMask;

    /// <summary>Gets a value indicating whether the page is BCPack.</summary>
    public bool IsBcPack => !IsZlib && Codec != 0;

    /// <summary>
    /// Gets a value indicating whether the page's bytes can be read: stored, zlib, or BCPack of a codec with a block
    /// decoder (<see cref="BcPackPageDecoder"/>) and a known virtual size of whole blocks.
    /// </summary>
    public bool IsDecodable => !IsBcPack || (BcPackPageDecoder.CanDecode(Codec) && BcPackDecoder.SymbolsPerLane((BcPackCodec)Codec, VirtualSize) > 0);

    /// <summary>Gets a value indicating whether the page decodes only approximately (<see cref="BcPackPageDecoder.IsApproximate"/>).</summary>
    public bool IsApproximate => IsBcPack && BcPackPageDecoder.IsApproximate(Codec);

    /// <summary>Gets the BC1 blocks a BCPack BC1 page decodes to, or 0 when its virtual size is unknown or not whole blocks.</summary>
    public int Bc1Blocks
    {
        get
        {
            if (VirtualSize <= 0 || VirtualSize % Bc1Decoder.BlockSize != 0 || VirtualSize / Bc1Decoder.BlockSize > Bc1Decoder.MaxBlocks)
            {
                return 0;
            }

            return (int)(VirtualSize / Bc1Decoder.BlockSize);
        }
    }

    /// <summary>Gets the virtual size, with <see cref="DefaultVirtualSize"/> standing in for 0.</summary>
    public long EffectiveVirtualSize
    {
        get
        {
            var size = VirtualSize == 0 ? DefaultVirtualSize : VirtualSize;

            return size;
        }
    }
}
