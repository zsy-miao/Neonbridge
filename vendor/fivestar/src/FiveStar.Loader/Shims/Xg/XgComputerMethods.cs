// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Xg;

/// <summary>
/// The implemented slots of the XGTextureAddressComputer vtable. Signatures come from the game's calls (docs/xg.md);
/// slots 3-5, which the game was not seen calling, follow the XDK method order that slots 2 and 6-10 match.
/// </summary>
internal static unsafe class XgComputerMethods
{
    [HotPath]
    [UnmanagedCallersOnly]
    public static uint AddRef(nint self)
    {
        try
        {
            return (uint)Interlocked.Increment(ref ((XgComputerObject*)self)->References);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XgComputerMethods.AddRef");

            return default;
        }
    }

    [HotPath]
    [UnmanagedCallersOnly]
    public static uint Release(nint self)
    {
        try
        {
            int left = Interlocked.Decrement(ref ((XgComputerObject*)self)->References);

            if (left == 0)
            {
                NativeMemory.Free((void*)self);
            }

            return (uint)left;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XgComputerMethods.Release");

            return default;
        }
    }

    /// <summary>Slot 2: GetResourceLayout(XG_RESOURCE_LAYOUT*).</summary>
    [HotPath]
    [UnmanagedCallersOnly]
    public static int GetResourceLayout(nint self, byte* layout)
    {
        try
        {
            if (layout == null)
            {
                return HResult.Pointer;
            }

            XgLayoutWriter.Write(((XgComputerObject*)self)->Surface, layout);

            return HResult.Ok;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XgComputerMethods.GetResourceLayout");

            return HResult.Fail;
        }
    }

    /// <summary>Slot 3: UINT64 GetResourceSizeBytes().</summary>
    [HotPath]
    [UnmanagedCallersOnly]
    public static ulong GetResourceSizeBytes(nint self)
    {
        return ((XgComputerObject*)self)->Surface.Size;
    }

    /// <summary>Slot 4: UINT64 GetResourceBaseAlignmentBytes().</summary>
    [HotPath]
    [UnmanagedCallersOnly]
    public static ulong GetResourceBaseAlignmentBytes(nint self)
    {
        return ((XgComputerObject*)self)->Surface.Alignment;
    }

    /// <summary>Slot 5: UINT64 GetMipLevelOffsetBytes(UINT plane, UINT mip).</summary>
    [HotPath]
    [UnmanagedCallersOnly]
    public static ulong GetMipLevelOffsetBytes(nint self, uint plane, uint mip)
    {
        try
        {
            return XgAddress.Mip(((XgComputerObject*)self)->Surface, plane, mip);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XgComputerMethods.GetMipLevelOffsetBytes");

            return default;
        }
    }

    /// <summary>
    /// Slot 6: UINT64 GetTexelElementOffsetBytes(UINT plane, UINT mip, UINT64 x, UINT y, UINT zOrSlice, UINT sample,
    /// UINT* bitOffset). x and y count elements (the game divides BC coordinates by 4); the sample is ignored (no MSAA
    /// layouts) and the bit offset is 0 (no sub-byte elements).
    /// </summary>
    [HotPath]
    [UnmanagedCallersOnly]
    public static ulong GetTexelElementOffsetBytes(nint self, uint plane, uint mip, ulong x, uint y, uint zOrSlice, uint sample, uint* bitOffset)
    {
        try
        {
            if (bitOffset != null)
            {
                *bitOffset = 0;
            }

            return XgAddress.Texel(((XgComputerObject*)self)->Surface, plane, mip, x, y, zOrSlice);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XgComputerMethods.GetTexelElementOffsetBytes");

            return default;
        }
    }

    /// <summary>Slot 8: CopyIntoSubresource(void* resource, UINT plane, UINT subresource, const void* linear, UINT rowPitch, UINT slicePitch).</summary>
    [HotPath]
    [UnmanagedCallersOnly]
    public static int CopyIntoSubresource(nint self, byte* resource, uint plane, uint subresource, byte* linear, uint rowPitch, uint slicePitch)
    {
        try
        {
            return XgCopier.Into(((XgComputerObject*)self)->Surface, resource, plane, subresource, linear, rowPitch, slicePitch);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XgComputerMethods.CopyIntoSubresource");

            return HResult.Fail;
        }
    }

    /// <summary>
    /// Slot 9: CopyFromSubresource(void* linear, UINT plane, UINT subresource, UINT unknown, const void* resource,
    /// UINT rowPitch, UINT slicePitch); the game passes 0 as the fourth argument.
    /// </summary>
    [HotPath]
    [UnmanagedCallersOnly]
    public static int CopyFromSubresource(nint self, byte* linear, uint plane, uint subresource, uint unknown, byte* resource, uint rowPitch, uint slicePitch)
    {
        try
        {
            return XgCopier.From(((XgComputerObject*)self)->Surface, linear, plane, subresource, resource, rowPitch, slicePitch);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XgComputerMethods.CopyFromSubresource");

            return HResult.Fail;
        }
    }

    /// <summary>Slot 10: GetResourceTiling with the arguments of ID3D12Device::GetResourceTiling after the resource.</summary>
    [HotPath]
    [UnmanagedCallersOnly]
    public static void GetResourceTiling(nint self, uint* tileCount, XgPackedMipInfo* packed, XgTileShape* shape, uint* tilingCount, uint first, XgSubresourceTiling* tilings)
    {
        try
        {
            XgTiling.Get(((XgComputerObject*)self)->Surface, tileCount, packed, shape, tilingCount, first, tilings);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XgComputerMethods.GetResourceTiling");
        }
    }
}
