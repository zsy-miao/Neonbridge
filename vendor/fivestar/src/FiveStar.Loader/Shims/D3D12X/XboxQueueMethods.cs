// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;
using FiveStar.Loader.Shims.D3D12X.DebugLayer;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>Command-queue slots that take an array of wrapped lists: per-frame, so allocation-free.</summary>
internal static unsafe class XboxQueueMethods
{
    /// <summary>X slot 35, read as ExecuteCommandListsX: (count, lists, frame object, flags 0 or 3).</summary>
    public const int ExecuteCommandListsXSlot = 35;

    /// <summary>X slot 12: desktop ExecuteCommandLists two slots later, like the queue's other desktop methods.</summary>
    public const int ExecuteCommandListsSlot = 12;

    /// <summary>ID3D12CommandQueue::ExecuteCommandLists in the desktop vtable (d3d12.h).</summary>
    private const int DesktopExecuteSlot = 10;

    /// <summary>Lists unwrapped per desktop call; the game submits at most 256 at once (0x142481e90).</summary>
    private const int Batch = 256;

    /// <summary>
    /// Slot 35. The frame object and the flags have no desktop counterpart: the lists are submitted in order and the
    /// call succeeds.
    /// </summary>
    [HotPath]
    [UnmanagedCallersOnly]
    public static int ExecuteCommandListsX(XboxObject* self, uint count, nint* lists, nint frame, uint flags)
    {
        try
        {
            if (QueueTrace.Enabled)
            {
                QueueTrace.Execute(self, count, frame, flags);
            }

            Execute(self, count, lists);

            return HResult.Ok;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XboxQueueMethods.ExecuteCommandListsX");

            return HResult.Fail;
        }
    }

    /// <summary>Slot 12, desktop ExecuteCommandLists(count, lists).</summary>
    [HotPath]
    [UnmanagedCallersOnly]
    public static void ExecuteCommandLists(XboxObject* self, uint count, nint* lists)
    {
        try
        {
            if (QueueTrace.Enabled)
            {
                QueueTrace.Execute(self, count, 0, 0);
            }

            Execute(self, count, lists);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XboxQueueMethods.ExecuteCommandLists");
        }
    }

    [HotPath]
    private static void Execute(XboxObject* self, uint count, nint* lists)
    {
        if (lists == null)
        {
            return;
        }

        var d3d12 = ShimContext.Runtime.D3D12X;
        var objects = d3d12.Objects;
        d3d12.Textures.RefreshWatched(objects.DesktopDevice, d3d12.Presenter.Presents);
        d3d12.Residency?.Poll(objects.DesktopDevice, d3d12.Bindings.Window.WindowFlags, d3d12.Presenter.Presents);

        nint inner = self->Inner;
        var execute = (delegate* unmanaged<nint, uint, nint*, void>)ComObject.Slot(inner, DesktopExecuteSlot);
        nint* desktop = stackalloc nint[Batch];

        for (uint first = 0; first < count; first += Batch)
        {
            uint size = count - first;

            if (size > Batch)
            {
                size = Batch;
            }

            for (uint i = 0; i < size; i++)
            {
                nint list = lists[first + i];
                desktop[i] = objects.Unwrap(list);

                if (desktop[i] != list)
                {
                    ListSubmissions.WaitIfRunning(list);
                    MemoryWaitWriters.Submitted(list, (XboxQueueObject*)self);
                }
            }

            RetireWaits.BeforeExecute((XboxQueueObject*)self, lists + first, size);
            Submit(self, lists + first, desktop, size, execute);
        }
    }

    /// <summary>
    /// Runs the lists in order. A list with memory waits or segments (<see cref="MemoryWaits"/>, <see cref="ListSplits"/>)
    /// goes alone after the lists before it: each of its segments runs after the waits that precede it.
    /// </summary>
    [HotPath]
    private static void Submit(XboxObject* self, nint* lists, nint* desktop, uint count, delegate* unmanaged<nint, uint, nint*, void> execute)
    {
        var objects = ShimContext.Runtime.D3D12X.Objects;
        var queue = (XboxQueueObject*)self;
        nint inner = self->Inner;
        uint start = 0;

        for (uint i = 0; i <= count; i++)
        {
            bool waits = i < count && desktop[i] != lists[i] && MemoryWaits.HasWaits(lists[i]);

            if (i < count && !waits)
            {
                continue;
            }

            if (i > start)
            {
                execute(inner, i - start, desktop + start);
                ListSubmissions.Submitted(queue, lists + start, i - start, objects);
            }

            if (waits)
            {
                SubmitSegments(queue, lists[i], desktop[i], execute);
                ListSubmissions.Submitted(queue, lists + i, 1, objects);
            }

            start = i + 1;
        }
    }

    /// <summary>The segments of <paramref name="list"/> (a wrapper whose desktop list is <paramref name="last"/>), each after its waits.</summary>
    [HotPath]
    private static void SubmitSegments(XboxQueueObject* queue, nint list, nint last, delegate* unmanaged<nint, uint, nint*, void> execute)
    {
        var segments = ListSegments.Of((XboxObject*)list);
        nint inner = queue->Header.Inner;

        for (int s = 0; s <= segments->Count; s++)
        {
            MemoryWaits.Before(queue, list, s);
            nint segment = segments->Segment(s, last);
            execute(inner, 1, &segment);
        }
    }
}
