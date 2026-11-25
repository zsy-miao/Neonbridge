// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.CompilerServices;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Xg;

/// <summary>
/// CopyIntoSubresource and CopyFromSubresource: element by element between row-major data (rows of
/// <c>rowPitch</c> bytes, depth slices of <c>slicePitch</c> bytes) and the resource's console layout. A subresource
/// is mip + slice x mips (D3D12 order, the plane passed apart); a 3D resource's subresource is its mip, and its depth
/// slices are the layout's slices.
/// </summary>
internal static unsafe class XgCopier
{
    private const uint Texture3D = 4;

    [HotPath]
    public static int Into(in XgSurface surface, byte* resource, uint planeIndex, uint subresource, byte* linear, uint rowPitch, uint slicePitch)
    {
        return Copy(surface, resource, planeIndex, subresource, linear, rowPitch, slicePitch, true);
    }

    [HotPath]
    public static int From(in XgSurface surface, byte* linear, uint planeIndex, uint subresource, byte* resource, uint rowPitch, uint slicePitch)
    {
        return Copy(surface, resource, planeIndex, subresource, linear, rowPitch, slicePitch, false);
    }

    private static int Copy(in XgSurface surface, byte* resource, uint planeIndex, uint subresource, byte* linear, uint rowPitch, uint slicePitch, bool into)
    {
        if (resource == null || linear == null || planeIndex >= (uint)surface.PlaneCount)
        {
            return HResult.InvalidArgument;
        }

        ref readonly XgPlane plane = ref surface.Planes[(int)planeIndex];
        bool volume = surface.Desc.Dimension == Texture3D;
        int mipIndex = (int)(subresource % (uint)plane.MipCount);
        int firstSlice = volume ? 0 : (int)(subresource / (uint)plane.MipCount);
        int sliceCount = volume ? Math.Max(1, plane.Slices >> mipIndex) : 1;

        if (firstSlice >= plane.Slices)
        {
            return HResult.InvalidArgument;
        }

        for (int depth = 0; depth < sliceCount; depth++)
        {
            CopySlice(plane, mipIndex, firstSlice + depth, resource, linear + ((ulong)depth * slicePitch), rowPitch, into);
        }

        return HResult.Ok;
    }

    private static void CopySlice(in XgPlane plane, int mipIndex, int slice, byte* resource, byte* linear, uint rowPitch, bool into)
    {
        ref readonly XgMip mip = ref plane.Mips[mipIndex];
        uint elementBytes = (uint)plane.BytesPerElement;

        for (uint y = 0; y < mip.HeightElements; y++)
        {
            byte* row = linear + ((ulong)y * rowPitch);

            if (plane.BlockBytes == 0)
            {
                byte* start = resource + XgAddress.Element(plane, mipIndex, slice, 0, y);
                CopyBytes(start, row, mip.WidthElements * elementBytes, into);
                continue;
            }

            for (uint x = 0; x < mip.WidthElements; x++)
            {
                byte* element = resource + XgAddress.Element(plane, mipIndex, slice, x, y);
                CopyBytes(element, row + (x * elementBytes), elementBytes, into);
            }
        }
    }

    private static void CopyBytes(byte* resource, byte* linear, uint bytes, bool into)
    {
        if (into)
        {
            Unsafe.CopyBlockUnaligned(resource, linear, bytes);
        }
        else
        {
            Unsafe.CopyBlockUnaligned(linear, resource, bytes);
        }
    }
}
