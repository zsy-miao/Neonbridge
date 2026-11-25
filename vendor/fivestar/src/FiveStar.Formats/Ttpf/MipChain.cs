// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Numerics;

namespace FiveStar.Formats.Ttpf;

/// <summary>
/// The mip chain of one slice (docs/xg.md, Layout): the mip tail block first, then the other mips smallest first, each
/// padded to whole tiles, so mip 0 ends the slice. A mip is in the tail when it fits a tile halved in width and at most
/// log2(tile bytes) - 4 mips remain from it; a chain of one mip has no tail. The chain splits into units, smallest
/// first: the tail block (if any), then each other mip.
/// </summary>
public sealed class MipChain
{
    /// <summary>The tail holds at most log2(tile bytes) - 4 mips.</summary>
    private const int TailMipBias = 4;

    private const int TailWidthDivisor = 2;

    /// <summary>Tail slot m (counted from the last slot) starts at 16 &lt;&lt; m bytes above slot 6 and m x 256 up to it.</summary>
    private const int TailLargeSlotShift = 4;
    private const int TailLastSmallSlot = 6;
    private const int TailSmallSlotShift = 8;

    private readonly List<MipPlacement> _mips = [];

    public MipChain(TextureFormat format, SwizzlePattern pattern, int width, int height, int mipCount)
    {
        TileBytes = (long)pattern.TileElements * format.BytesPerElement;
        var count = Math.Clamp(mipCount, 1, FullCount(width, height));
        var first = FirstTailMip(format, pattern, width, height, count);
        var placed = new MipPlacement[count];

        HasTail = first < count;
        Units = first;
        long offset = 0;

        if (HasTail)
        {
            Units++;
            offset = TileBytes;
        }

        for (var mip = first - 1; mip >= 0; mip--)
        {
            var (blockWidth, blockHeight) = Extent(format, width, height, mip);
            var size = pattern.PaddedSize(blockWidth, blockHeight, format.BytesPerElement);
            placed[mip] = new MipPlacement(blockWidth, blockHeight, offset, size, false, 0);
            offset += size;
        }

        var tailMips = BitOperations.Log2((ulong)TileBytes) - TailMipBias;

        for (var mip = first; mip < count; mip++)
        {
            var (blockWidth, blockHeight) = Extent(format, width, height, mip);
            placed[mip] = new MipPlacement(blockWidth, blockHeight, 0, TileBytes, true, SlotOffset(tailMips - 1 - (mip - first)));
        }

        _mips.AddRange(placed);
        SliceSize = offset;
    }

    public IReadOnlyList<MipPlacement> Mips => _mips;

    /// <summary>Gets the bytes of one slice.</summary>
    public long SliceSize { get; }

    /// <summary>Gets the bytes of one tile.</summary>
    public long TileBytes { get; }

    public bool HasTail { get; }

    /// <summary>Gets the unit count: the mips outside the tail, plus one for the tail block.</summary>
    public int Units { get; }

    /// <summary>Mips in a full chain down to 1x1.</summary>
    public static int FullCount(int width, int height)
    {
        return BitOperations.Log2((uint)Math.Max(width, height)) + 1;
    }

    /// <summary>Bytes of the smallest <paramref name="units"/> units of the chain.</summary>
    public long SmallestUnitsSize(int units)
    {
        long size = 0;
        var tailUnits = HasTail ? 1 : 0;

        if (units >= tailUnits)
        {
            size += tailUnits * TileBytes;
        }

        for (var unit = tailUnits; unit < units; unit++)
        {
            size += _mips[Units - 1 - unit].UnitSize;
        }

        return size;
    }

    private static int FirstTailMip(TextureFormat format, SwizzlePattern pattern, int width, int height, int count)
    {
        var tileBytes = (long)pattern.TileElements * format.BytesPerElement;
        var tailMips = BitOperations.Log2((ulong)tileBytes) - TailMipBias;

        for (var mip = 0; count > 1 && mip < count; mip++)
        {
            var (blockWidth, blockHeight) = Extent(format, width, height, mip);

            if (blockWidth <= pattern.TileWidth / TailWidthDivisor && blockHeight <= pattern.TileHeight && count - mip <= tailMips)
            {
                return mip;
            }
        }

        return count;
    }

    private static (int Width, int Height) Extent(TextureFormat format, int width, int height, int mip)
    {
        var mipWidth = Math.Max(1, width >> mip);
        var mipHeight = Math.Max(1, height >> mip);

        return ((mipWidth + format.BlockDimension - 1) / format.BlockDimension, (mipHeight + format.BlockDimension - 1) / format.BlockDimension);
    }

    private static long SlotOffset(int slot)
    {
        var clamped = Math.Max(0, slot);

        if (clamped > TailLastSmallSlot)
        {
            return 1L << (clamped + TailLargeSlotShift);
        }

        return (long)clamped << TailSmallSlotShift;
    }
}
