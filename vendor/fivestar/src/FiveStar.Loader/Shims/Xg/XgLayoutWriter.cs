// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.CompilerServices;

namespace FiveStar.Loader.Shims.Xg;

/// <summary>
/// Fills XG_RESOURCE_LAYOUT (0x1b40 bytes) as the game reads it. Read by the game (0x140979c40, 0x145121980):
/// plane entries of 0x6c8 bytes from 0x18, a plane's array slice stride at +0x10 and its mip entries of 0x70 bytes
/// from +0x38, a mip's SizeBytes (+0x00), OffsetBytes (+0x08), Slice2DSizeBytes (+0x10) and PitchBytes (+0x1c). The
/// other fields follow the XDK order of the same structures and are not read by the game (docs/xg.md).
/// </summary>
internal static unsafe class XgLayoutWriter
{
    public const int LayoutBytes = 0x1b40;

    private const int ResourceSize = 0x00;
    private const int ResourceAlignment = 0x08;
    private const int ResourceMipLevels = 0x10;
    private const int ResourcePlanes = 0x14;
    private const int FirstPlane = 0x18;
    private const int PlaneBytes = 0x6c8;
    private const int ResourceDimension = 0x1b38;

    private const int PlaneSize = 0x08;
    private const int PlaneSliceStride = 0x10;
    private const int PlaneOffset = 0x18;
    private const int PlaneAlignment = 0x20;
    private const int PlaneBytesPerElement = 0x28;
    private const int FirstMip = 0x38;
    private const int MipBytes = 0x70;

    private const int MipSize = 0x00;
    private const int MipOffset = 0x08;
    private const int MipSlice2DSize = 0x10;
    private const int MipPitchPixels = 0x18;
    private const int MipPitchBytes = 0x1c;
    private const int MipAlignment = 0x20;
    private const int MipPaddedWidth = 0x24;
    private const int MipPaddedHeight = 0x28;
    private const int MipPaddedDepth = 0x2c;
    private const int MipWidth = 0x30;
    private const int MipHeight = 0x34;
    private const int MipDepth = 0x38;

    [HotPath]
    public static void Write(in XgSurface surface, byte* layout)
    {
        new Span<byte>(layout, LayoutBytes).Clear();
        Put(layout + ResourceSize, surface.Size);
        Put(layout + ResourceAlignment, surface.Alignment);
        Put(layout + ResourceMipLevels, (uint)surface.Planes[0].MipCount);
        Put(layout + ResourcePlanes, (uint)surface.PlaneCount);
        Put(layout + ResourceDimension, surface.Desc.Dimension);

        for (int index = 0; index < surface.PlaneCount; index++)
        {
            WritePlane(in surface.Planes[index], layout + FirstPlane + (index * PlaneBytes));
        }
    }

    private static void WritePlane(in XgPlane plane, byte* entry)
    {
        ulong alignment = plane.BlockBytes == 0 ? XgLayoutBuilder.LinearMipAlignment :(ulong)plane.BlockBytes;
        Put(entry + PlaneSize, plane.Size);
        Put(entry + PlaneSliceStride, plane.SliceStride);
        Put(entry + PlaneOffset, plane.Offset);
        Put(entry + PlaneAlignment, alignment);
        Put(entry + PlaneBytesPerElement, (uint)plane.BytesPerElement);

        for (int index = 0; index < plane.MipCount; index++)
        {
            ref readonly XgMip mip = ref plane.Mips[index];
            byte* target = entry + FirstMip + (index * MipBytes);
            uint pitchBytes = mip.PitchElements * (uint)plane.BytesPerElement;
            Put(target + MipSize, mip.Size);
            Put(target + MipOffset, plane.Offset + mip.Offset);
            Put(target + MipSlice2DSize, mip.Slice2DSize);

            Put(target + MipPitchPixels, mip.PitchElements * (uint)plane.BlockDimension);
            Put(target + MipPitchBytes, pitchBytes);
            Put(target + MipAlignment, (uint)alignment);
            Put(target + MipPaddedWidth, mip.PitchElements);
            Put(target + MipPaddedHeight, mip.PaddedHeightElements);
            Put(target + MipPaddedDepth, mip.DepthElements);

            Put(target + MipWidth, mip.WidthElements);
            Put(target + MipHeight, mip.HeightElements);
            Put(target + MipDepth, mip.DepthElements);
        }
    }

    private static void Put(byte* target, ulong value)
    {
        Unsafe.WriteUnaligned(target, value);
    }

    private static void Put(byte* target, uint value)
    {
        Unsafe.WriteUnaligned(target, value);
    }
}
