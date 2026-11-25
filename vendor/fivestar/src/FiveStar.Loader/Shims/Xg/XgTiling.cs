// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Numerics;

namespace FiveStar.Loader.Shims.Xg;

/// <summary>
/// GetResourceTiling over the console layout, with D3D12 semantics and 64 KB tiles (the game maps reserved
/// resources with it at 0x1403441f0). In a 64 KB mode the mips outside the tail are the standard mips and the tail
/// block is the one packed tile at the start of the slice; any other mode reports every mip as packed.
/// </summary>
internal static unsafe class XgTiling
{
    /// <summary>D3D12_PACKED_TILE: the start tile of a packed mip.</summary>
    private const uint PackedTile = uint.MaxValue;

    private const uint Texture3D = 4;

    [HotPath]
    public static void Get(in XgSurface surface, uint* tileCount, XgPackedMipInfo* packed, XgTileShape* shape, uint* tilingCount, uint first, XgSubresourceTiling* tilings)
    {
        ref readonly XgPlane plane = ref surface.Planes[0];
        ulong tileBytes = XgSwizzles.LargeBlockBytes;
        int standard = plane.BlockBytes == XgSwizzles.LargeBlockBytes ? plane.FirstTailMip : 0;

        if (tileCount != null)
        {
            *tileCount = (uint)(XgLayoutBuilder.AlignUp(surface.Size, tileBytes) / tileBytes);
        }

        if (packed != null)
        {
            ulong packedBytes = standard == 0 ? plane.SliceStride : (ulong)plane.BlockBytes;
            packed->NumStandardMips = (byte)standard;
            packed->NumPackedMips = (byte)(plane.MipCount - standard);
            packed->NumTilesForPackedMips = plane.MipCount > standard ? (uint)(XgLayoutBuilder.AlignUp(packedBytes, tileBytes) / tileBytes) : 0;
            packed->StartTileIndexInOverallResource = (uint)(plane.Offset / tileBytes);
        }

        if (shape != null)
        {
            WriteShape(plane, shape);
        }

        if (tilingCount != null && tilings != null)
        {
            WriteTilings(surface, standard, tilingCount, first, tilings);
        }
    }

    private static void WriteShape(in XgPlane plane, XgTileShape* shape)
    {
        *shape = default;

        if (plane.BlockBytes == 0)
        {
            return;
        }

        int elementLog2 = BitOperations.Log2((uint)plane.BytesPerElement);
        var swizzle = plane.Thick ? XgSwizzles.FindVolume(XgSwizzles.LargeBlockBytes, elementLog2) : XgSwizzles.Find(XgSwizzles.LargeBlockBytes, elementLog2);
        shape->WidthInTexels = (uint)(swizzle.TileWidth * plane.BlockDimension);
        shape->HeightInTexels = (uint)(swizzle.TileHeight * plane.BlockDimension);
        shape->DepthInTexels = (uint)swizzle.TileDepth;
    }

    private static void WriteTilings(in XgSurface surface, int standard, uint* tilingCount, uint first, XgSubresourceTiling* tilings)
    {
        ref readonly XgPlane plane = ref surface.Planes[0];
        int slices = surface.Desc.Dimension == Texture3D ? 1 : plane.Slices;
        uint total = (uint)(plane.MipCount * slices);
        uint count = first >= total ? 0 : Math.Min(*tilingCount, total - first);

        for (uint index = 0; index < count; index++)
        {
            uint subresource = first + index;
            int mipIndex = (int)(subresource % (uint)plane.MipCount);
            int slice = (int)(subresource / (uint)plane.MipCount);
            ref readonly XgMip mip = ref plane.Mips[mipIndex];
            XgSubresourceTiling* tiling = tilings + index;

            if (mipIndex >= standard)
            {
                *tiling = default;
                tiling->StartTileIndexInOverallResource = PackedTile;
                continue;
            }

            tiling->WidthInTiles = mip.PitchElements / (uint)plane.Swizzle.TileWidth;
            tiling->HeightInTiles = (ushort)(mip.PaddedHeightElements / (uint)plane.Swizzle.TileHeight);
            tiling->DepthInTiles = (ushort)(XgLayoutBuilder.AlignUp(mip.DepthElements, (ulong)plane.Swizzle.TileDepth) / (ulong)plane.Swizzle.TileDepth);
            tiling->StartTileIndexInOverallResource = (uint)((plane.Offset + ((ulong)slice * plane.SliceStride) + mip.Offset) / (ulong)XgSwizzles.LargeBlockBytes);
        }

        *tilingCount = count;
    }
}
