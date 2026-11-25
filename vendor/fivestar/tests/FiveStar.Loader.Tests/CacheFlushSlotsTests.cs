// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Shims.D3D12X.Present;
using FiveStar.Loader.Shims.D3D12X.States;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>The console cache flushes (command-list slots 131, 132, 69) record a global UAV barrier on the desktop list.</summary>
public sealed unsafe class CacheFlushSlotsTests
{
    private const int ResourceBarrierSlot = 26;

    private static int _count;
    private static TransitionBarrier _written;

    [Fact]
    public void A_cache_flush_records_one_UAV_barrier_on_no_resource()
    {
        nint* vtable = (nint*)NativeMemory.AllocZeroed(ResourceBarrierSlot + 1, (nuint)sizeof(nint));
        nint list = (nint)NativeMemory.AllocZeroed((nuint)sizeof(nint));

        try
        {
            vtable[ResourceBarrierSlot] = (nint)(delegate* unmanaged<nint, uint, TransitionBarrier*, void>)&Record;
            *(nint**)list = vtable;
            _count = 0;

            CacheFlushSlots.Barrier(list);

            Assert.Equal(1, _count);
            Assert.Equal((TransitionBarrier.UavType, 0), (_written.Type, _written.Resource));
            Assert.Equal([131, 132, 69], new[] { CacheFlushSlots.FlushGpuCacheRangeSlot, CacheFlushSlots.FlushShaderCacheRangeSlot, CacheFlushSlots.FlushPipelineSlot });
        }
        finally
        {
            NativeMemory.Free(vtable);
            NativeMemory.Free((void*)list);
        }
    }

    [UnmanagedCallersOnly]
    private static void Record(nint list, uint count, TransitionBarrier* barriers)
    {
        _count += (int)count;
        _written = *barriers;
    }
}
