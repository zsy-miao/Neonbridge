// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Numerics;

namespace FiveStar.Loader.Shims.Xg;

/// <summary>
/// Computes the console layout of a texture (docs/xg.md): per plane, array slices of one mip chain each; in a slice
/// the mip tail block comes first and the other mips follow smallest first, each padded to whole blocks, so mip 0
/// ends the slice. A mip is in the tail when the chain has more than one mip, it fits a block halved in width and at
/// most log2(block bytes) - 4 mips remain from it to the end of the chain; inside the tail, mip k starts at the byte
/// offset of the RDNA address library's mip tail table. A linear plane holds the mips in order from mip 0, rows
/// padded to 256 bytes.
/// </summary>
internal static class XgLayoutBuilder
{
    /// <summary>D3D12_RESOURCE_DIMENSION_TEXTURE1D: no height.</summary>
    private const uint Texture1D = 2;

    /// <summary>D3D12_RESOURCE_DIMENSION_TEXTURE3D: DepthOrArraySize is a depth.</summary>
    private const uint Texture3D = 4;

    private const int StencilBytes = 1;
    private const int PixelBlock = 1;
    private const int MaxElementLog2 = XgSwizzles.ElementSizes - 1;

    /// <summary>Linear rows are padded to 256 bytes and mips start on 512 bytes (D3D12 placed footprint alignment).</summary>
    private const int LinearRowBytes = 256;
    public const int LinearMipAlignment = 512;

    /// <summary>The tail holds at most log2(block bytes) - 4 mips (thin resources).</summary>
    private const int TailMipBias = 4;

    /// <summary>A mip fits the tail when it fits a block halved in width (log2 of the block bytes is even).</summary>
    private const int TailWidthDivisor = 2;

    /// <summary>Tail slot m (counted from the last slot) is at 16 &lt;&lt; m bytes above slot 6 and m x 256 bytes up to it.</summary>
    private const int TailLargeSlotShift = 4;
    private const int TailLastSmallSlot = 6;
    private const int TailSmallSlotShift = 8;

    [HotPath]
    public static void Build(in XgResourceDesc desc, out XgSurface surface)
    {
        surface = default;
        surface.Desc = desc;
        var format = XgFormats.Find(desc.Format);
        uint width = AtLeastOne(desc.Width);
        uint height = desc.Dimension == Texture1D ? 1u : AtLeastOne(desc.Height);

        int mips = MipCount(desc.MipLevels, width, height);
        int slices = (int)AtLeastOne(desc.DepthOrArraySize);
        long mip0Bytes = (long)Elements(width, format.BlockDimension) * Elements(height, format.BlockDimension) * format.BytesPerElement;
        uint mode = XgSwizzleModes.Mode(desc.Layout, mip0Bytes);
        int blockBytes = XgSwizzleModes.BlockBytes(mode);
        bool thick = desc.Dimension == Texture3D && XgSwizzleModes.IsThick(mode);

        surface.PlaneCount = format.StencilPlane ? XgLimits.MaxPlanes : 1;
        ulong offset = 0;

        for (int index = 0; index < surface.PlaneCount; index++)
        {
            ref XgPlane plane = ref surface.Planes[index];
            int bytesPerElement = index == 0 ? format.BytesPerElement : StencilBytes;
            int blockDimension = index == 0 ? format.BlockDimension : PixelBlock;
            plane.Thick = thick;
            BuildPlane(ref plane, bytesPerElement, blockDimension, blockBytes, width, height, mips, slices);

            ulong alignment = PlaneAlignment(plane.BlockBytes);
            offset = AlignUp(offset, alignment);
            plane.Offset = offset;
            offset += plane.Size;
            surface.Alignment = alignment;
        }

        surface.Size = AlignUp(offset, surface.Alignment);
    }

    /// <summary>Elements covering a pixel extent.</summary>
    [HotPath]
    public static uint Elements(uint pixels, int blockDimension)
    {
        return (pixels + (uint)blockDimension - 1) / (uint)blockDimension;
    }

    /// <summary>A size field read as at least 1, as D3D12 does for height and depth.</summary>
    [HotPath]
    public static uint AtLeastOne(ulong value)
    {
        if (value == 0)
        {
            return 1;
        }

        return (uint)value;
    }

    [HotPath]
    public static ulong AlignUp(ulong value, ulong alignment)
    {
        return (value + alignment - 1) / alignment * alignment;
    }

    private static void BuildPlane(ref XgPlane plane, int bytesPerElement, int blockDimension, int blockBytes, uint width, uint height, int mips, int slices)
    {
        plane.BytesPerElement = bytesPerElement;
        plane.BlockDimension = blockDimension;
        plane.MipCount = mips;
        plane.Slices = slices;
        plane.FirstTailMip = mips;
        SetMipExtents(ref plane, width, height);

        bool swizzled = BitOperations.IsPow2(bytesPerElement) && BitOperations.Log2((uint)bytesPerElement) <= MaxElementLog2;
        plane.BlockBytes = swizzled ? blockBytes : 0;

        plane.Thick &= plane.BlockBytes != 0;

        if (plane.BlockBytes == 0)
        {
            BuildLinear(ref plane);
        }
        else if (plane.Thick)
        {
            plane.Swizzle = XgSwizzles.FindVolume(plane.BlockBytes, BitOperations.Log2((uint)bytesPerElement));
            BuildThick(ref plane);
        }
        else
        {
            plane.Swizzle = XgSwizzles.Find(plane.BlockBytes, BitOperations.Log2((uint)bytesPerElement));
            BuildTiled(ref plane);
        }

        plane.Size = plane.Thick ? plane.SliceStride : plane.SliceStride * (ulong)slices;
    }

    /// <summary>
    /// A thick volume: each mip padded to whole bricks (x, then y, then z), smallest mip first so mip 0 ends the
    /// chain; no mip tail (the shipped volumes have one mip).
    /// </summary>
    private static void BuildThick(ref XgPlane plane)
    {
        var swizzle = plane.Swizzle;
        ulong offset = 0;

        for (int index = plane.MipCount - 1; index >= 0; index--)
        {
            ref XgMip mip = ref plane.Mips[index];
            mip.DepthElements = (uint)Math.Max(1, plane.Slices >> index);
            mip.PitchElements = (uint)AlignUp(mip.WidthElements, (ulong)swizzle.TileWidth);
            mip.PaddedHeightElements = (uint)AlignUp(mip.HeightElements, (ulong)swizzle.TileHeight);
            mip.Slice2DSize = (ulong)mip.PitchElements * mip.PaddedHeightElements * (ulong)plane.BytesPerElement;
            mip.Size = mip.Slice2DSize * AlignUp(mip.DepthElements, (ulong)swizzle.TileDepth);

            mip.Offset = offset;
            offset += mip.Size;
        }

        plane.SliceStride = offset;
    }

    private static void SetMipExtents(ref XgPlane plane, uint width, uint height)
    {
        for (int index = 0; index < plane.MipCount; index++)
        {
            ref XgMip mip = ref plane.Mips[index];
            mip.WidthElements = Elements(Math.Max(1u, width >> index), plane.BlockDimension);
            mip.HeightElements = Elements(Math.Max(1u, height >> index), plane.BlockDimension);
            mip.DepthElements = 1;
        }
    }

    private static void BuildLinear(ref XgPlane plane)
    {
        uint pitchAlignment = (uint)(LinearRowBytes >> Math.Min(BitOperations.TrailingZeroCount(plane.BytesPerElement), BitOperations.Log2(LinearRowBytes)));
        ulong offset = 0;

        for (int index = 0; index < plane.MipCount; index++)
        {
            ref XgMip mip = ref plane.Mips[index];
            mip.PitchElements = (uint)AlignUp(mip.WidthElements, pitchAlignment);
            mip.PaddedHeightElements = mip.HeightElements;
            mip.Slice2DSize = (ulong)mip.PitchElements * mip.PaddedHeightElements * (ulong)plane.BytesPerElement;
            mip.Size = mip.Slice2DSize;

            offset = AlignUp(offset, LinearMipAlignment);
            mip.Offset = offset;
            offset += mip.Size;
        }

        plane.SliceStride = AlignUp(offset, LinearMipAlignment);
    }

    private static void BuildTiled(ref XgPlane plane)
    {
        var swizzle = plane.Swizzle;
        int tailMips = BitOperations.Log2((uint)plane.BlockBytes) - TailMipBias;
        // A chain of one mip has no tail: a lone mip that would fit the tail starts at offset 0, as the shipped textures show.
        bool hasTail = plane.BlockBytes > XgSwizzles.MicroBlockBytes && plane.MipCount > 1;

        for (int index = 0; hasTail && index < plane.MipCount; index++)
        {
            ref XgMip mip = ref plane.Mips[index];

            if (mip.WidthElements <= swizzle.TileWidth / TailWidthDivisor && mip.HeightElements <= swizzle.TileHeight && plane.MipCount - index <= tailMips)
            {
                plane.FirstTailMip = index;
                break;
            }
        }

        ulong offset = plane.FirstTailMip < plane.MipCount ? (ulong)plane.BlockBytes : 0;

        for (int index = plane.FirstTailMip - 1; index >= 0; index--)
        {
            ref XgMip mip = ref plane.Mips[index];
            mip.PitchElements = (uint)AlignUp(mip.WidthElements, (ulong)swizzle.TileWidth);
            mip.PaddedHeightElements = (uint)AlignUp(mip.HeightElements, (ulong)swizzle.TileHeight);
            mip.Slice2DSize = (ulong)mip.PitchElements * mip.PaddedHeightElements * (ulong)plane.BytesPerElement;
            mip.Size = mip.Slice2DSize;

            mip.Offset = offset;
            offset += mip.Size;
        }

        for (int index = plane.FirstTailMip; index < plane.MipCount; index++)
        {
            SetTailMip(ref plane.Mips[index], swizzle, plane.BytesPerElement, tailMips - 1 - (index - plane.FirstTailMip));
        }

        plane.SliceStride = offset;
    }

    /// <summary>Places a tail mip in slot <paramref name="slot"/>; it is addressed with the full block swizzle from its first element.</summary>
    private static void SetTailMip(ref XgMip mip, XgSwizzle swizzle, int bytesPerElement, int slot)
    {
        int clamped = Math.Max(0, slot);
        ulong start = clamped > TailLastSmallSlot ? 1ul << (clamped + TailLargeSlotShift) : (ulong)clamped << TailSmallSlotShift;
        uint element = (uint)(start / (ulong)bytesPerElement);

        mip.InTail = 1;
        mip.Offset = start;
        mip.TailX = swizzle.X(element);
        mip.TailY = swizzle.Y(element);
        mip.PitchElements = (uint)(swizzle.TileWidth / TailWidthDivisor);
        mip.PaddedHeightElements = (uint)swizzle.TileHeight;

        mip.Slice2DSize = (ulong)mip.PitchElements * mip.PaddedHeightElements * (ulong)bytesPerElement;
        mip.Size = mip.Slice2DSize;
    }

    private static int MipCount(ushort requested, uint width, uint height)
    {
        int full = BitOperations.Log2(Math.Max(width, height)) + 1;

        if (requested == 0)
        {
            return Math.Min(full, XgLimits.MaxMips);
        }

        return Math.Min((int)requested, XgLimits.MaxMips);
    }

    private static ulong PlaneAlignment(int blockBytes)
    {
        if (blockBytes == 0)
        {
            return LinearMipAlignment;
        }

        return (ulong)blockBytes;
    }
}
