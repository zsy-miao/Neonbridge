// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// Queue slot 31, CopyPageMappingsBatchX(count, batches, pool, pool pages, flags) (call at 0x140f2cc7f; VIctory names
/// it): maps pages of a pool range at the batches' destinations (<see cref="XMemPhysicalMemory.CopyPageMappings"/>).
/// The game streams pages into its pool and maps them into its tiled heap this way. On the console the remap is queue
/// work, after the queue's earlier work; here the calling thread waits until the desktop queue has finished all it was
/// given (the queue's shim fence), then remaps on the CPU, before it returns and can submit more.
/// </summary>
internal static unsafe class QueuePageMappings
{
    public const int CopyPageMappingsSlot = 31;

    /// <summary>ID3D12Fence::GetCompletedValue and SetEventOnCompletion (d3d12.h).</summary>
    private const int GetCompletedValueSlot = 8;
    private const int SetEventOnCompletionSlot = 9;

    public static void Bind(XboxObjects objects)
    {
        delegate* unmanaged<XboxQueueObject*, uint, PageBatch*, ulong, uint, uint, int> copy = &CopyPageMappings;
        objects.SetHandler(XboxInterfaceKind.CommandQueue, CopyPageMappingsSlot, (nint)copy);
    }

    [UnmanagedCallersOnly]
    public static int CopyPageMappings(XboxQueueObject* queue, uint count, PageBatch* batches, ulong pool, uint poolPages, uint flags)
    {
        try
        {
            if (count > 0 && batches == null)
            {
                return HResult.InvalidArgument;
            }

            WaitForQueue(queue);
            ShimContext.Runtime.XMemPhysical.CopyPageMappings(count, batches, (nint)pool, poolPages);

            return HResult.Ok;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "QueuePageMappings.CopyPageMappings");

            return HResult.Fail;
        }
    }

    /// <summary>Blocks until every execution submitted on <paramref name="queue"/> so far has finished on the GPU.</summary>
    private static void WaitForQueue(XboxQueueObject* queue)
    {
        nint fence = queue->Fence;
        ulong last = Volatile.Read(ref queue->Signaled);

        if (fence == 0 || last == 0)
        {
            return;
        }

        if (((delegate* unmanaged<nint, ulong>)ComObject.Slot(fence, GetCompletedValueSlot))(fence) >= last)
        {
            return;
        }

        // A null event makes SetEventOnCompletion return once the fence reaches the value.
        _ = ((delegate* unmanaged<nint, ulong, nint, int>)ComObject.Slot(fence, SetEventOnCompletionSlot))(fence, last, 0);
    }
}
