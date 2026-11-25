// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Xg;

/// <summary>Byte offsets of elements inside a resource laid out by <see cref="XgLayoutBuilder"/>.</summary>
internal static class XgAddress
{
    /// <summary>
    /// GetTexelElementOffsetBytes: the offset from the resource start of element (x, y) of a mip of an array (or depth)
    /// slice, or 0 when the plane, mip or slice does not exist.
    /// </summary>
    [HotPath]
    public static ulong Texel(in XgSurface surface, uint planeIndex, uint mipIndex, ulong x, uint y, uint slice)
    {
        if (planeIndex >= (uint)surface.PlaneCount)
        {
            return 0;
        }

        ref readonly XgPlane plane = ref surface.Planes[(int)planeIndex];

        if (mipIndex >= (uint)plane.MipCount || slice >= (uint)plane.Slices || x > uint.MaxValue)
        {
            return 0;
        }

        return Element(plane, (int)mipIndex, (int)slice, (uint)x, y);
    }

    /// <summary>GetMipLevelOffsetBytes: the offset from the resource start of a mip of array slice 0, or 0 when it does not exist.</summary>
    [HotPath]
    public static ulong Mip(in XgSurface surface, uint planeIndex, uint mipIndex)
    {
        if (planeIndex >= (uint)surface.PlaneCount || mipIndex >= (uint)surface.Planes[(int)planeIndex].MipCount)
        {
            return 0;
        }

        ref readonly XgPlane plane = ref surface.Planes[(int)planeIndex];

        return plane.Offset + plane.Mips[(int)mipIndex].Offset;
    }

    /// <summary>
    /// The offset from the resource start of element (x, y) of a mip of an array slice. Blocks are row-major across
    /// the mip's padded pitch; a tail mip is addressed with the whole block's swizzle from its first element.
    /// </summary>
    [HotPath]
    public static ulong Element(in XgPlane plane, int mipIndex, int slice, uint x, uint y)
    {
        ref readonly XgMip mip = ref plane.Mips[mipIndex];
        ulong sliceStart = plane.Offset + ((ulong)slice * plane.SliceStride);
        ulong bytesPerElement = (ulong)plane.BytesPerElement;

        if (plane.BlockBytes == 0)
        {
            return sliceStart + mip.Offset + ((((ulong)y * mip.PitchElements) + x) * bytesPerElement);
        }

        var swizzle = plane.Swizzle;

        if (plane.Thick)
        {
            return plane.Offset + mip.Offset + Brick(plane, mip, x, y, (uint)slice);
        }

        if (mip.InTail != 0)
        {
            return sliceStart + ((ulong)swizzle.Index(mip.TailX + x, mip.TailY + y) * bytesPerElement);
        }

        uint tileWidth = (uint)swizzle.TileWidth;
        uint tileHeight = (uint)swizzle.TileHeight;
        ulong tile = ((ulong)(y / tileHeight) * (mip.PitchElements / tileWidth)) + (x / tileWidth);
        ulong inside = (ulong)swizzle.Index(x % tileWidth, y % tileHeight) * bytesPerElement;

        return sliceStart + mip.Offset + (tile * (ulong)plane.BlockBytes) + inside;
    }

    /// <summary>The offset inside a thick mip of element (x, y, z): bricks in x, then y, then z.</summary>
    [HotPath]
    private static ulong Brick(in XgPlane plane, in XgMip mip, uint x, uint y, uint z)
    {
        var swizzle = plane.Swizzle;
        uint width = (uint)swizzle.TileWidth;
        uint height = (uint)swizzle.TileHeight;
        uint depth = (uint)swizzle.TileDepth;

        ulong columns = mip.PitchElements / width;
        ulong rows = mip.PaddedHeightElements / height;
        ulong brick = ((((ulong)(z / depth) * rows) + (y / height)) * columns) + (x / width);
        ulong inside = (ulong)swizzle.Index(x % width, y % height, z % depth) * (ulong)plane.BytesPerElement;

        return (brick * (ulong)plane.BlockBytes) + inside;
    }
}
