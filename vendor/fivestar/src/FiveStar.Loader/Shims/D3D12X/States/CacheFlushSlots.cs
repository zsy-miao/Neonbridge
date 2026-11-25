// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X.States;

/// <summary>
/// Command-list slots 131 (FlushGpuCacheRangeX), 132 (FlushShaderCacheRangeX) and 69 (FlushPipelineX in the newer list
/// layout), stubs before (names from the D3D12.X layout other ports use; facts only: XCL makes them a global UAV
/// barrier). The desktop has no cache flushes of its own: work recorded after one must see the writes before it, which
/// a UAV barrier on no resource orders. In this game's boot lists slots 131 and 132 carry register writes (register
/// offset, value, 0, mask; docs/d3d12x-shim.md), where the barrier only orders and is harmless.
/// </summary>
internal static unsafe class CacheFlushSlots
{
    public const int FlushGpuCacheRangeSlot = 131;
    public const int FlushShaderCacheRangeSlot = 132;
    public const int FlushPipelineSlot = 69;

    public static void Bind(XboxObjects objects)
    {
        delegate* unmanaged<XboxObject*, int> flush = &Flush;

        foreach (int slot in new[] { FlushGpuCacheRangeSlot, FlushShaderCacheRangeSlot, FlushPipelineSlot })
        {
            objects.SetHandler(XboxInterfaceKind.CommandList, slot, (nint)flush);
        }
    }

    /// <summary>Any of the flushes: a global UAV barrier on the desktop list; the arguments (ranges, flags) do not matter. Returns S_OK.</summary>
    [HotPath]
    [UnmanagedCallersOnly]
    public static int Flush(XboxObject* self)
    {
        try
        {
            ListSegments.Of(self)->Commands++;
            Barrier(self->Inner);

            return HResult.Ok;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "CacheFlushSlots.Flush");

            return HResult.Fail;
        }
    }

    /// <summary>The barrier a flush records on the desktop <paramref name="list"/>: a UAV barrier on no resource (every UAV access).</summary>
    [HotPath]
    internal static void Barrier(nint list)
    {
        ResourceBarrierSlots.Uav(list, 0);
    }
}
