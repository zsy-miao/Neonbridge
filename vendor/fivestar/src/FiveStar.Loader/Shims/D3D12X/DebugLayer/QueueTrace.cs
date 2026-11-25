// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X.DebugLayer;

/// <summary>
/// <c>--d3d12-trace-queues</c>: the first <see cref="Limit"/> queue submissions, queue Signal and Wait and CPU fence
/// signals in the log, in call order, to see the game's cross-queue protocol. Queues and fences are numbered in the
/// order they first show up.
/// </summary>
internal static unsafe class QueueTrace
{
    /// <summary>X queue slots Signal and Wait: desktop ID3D12CommandQueue 14 and 15, two slots later (d3d12.h).</summary>
    public const int QueueSignalSlot = 16;
    public const int QueueWaitSlot = 17;
    private const int DesktopQueueWaitSlot = 15;
    private const int DesktopGetDescSlot = 18;

    /// <summary>ID3D12Fence::Signal (CPU side) and ::GetCompletedValue, same slots on the X fence.</summary>
    public const int FenceSignalSlot = 10;
    private const int GetCompletedValueSlot = 8;

    /// <summary>Lines written before the trace stops.</summary>
    private const int Limit = 4000;

    private static readonly object Gate = new();
    private static readonly Dictionary<nint, int> Queues = [];
    private static readonly Dictionary<nint, int> Fences = [];
    private static LoaderLog? _log;
    private static int _lines;

    /// <summary>True once <see cref="Enable"/> ran; checked on the submission path before anything is formatted.</summary>
    public static bool Enabled { get; private set; }

    /// <summary>Binds the traced slots; the queue's ExecuteCommandLists handlers report through <see cref="Execute"/>.</summary>
    public static void Enable(XboxObjects objects, LoaderLog log)
    {
        _log = log;
        Enabled = true;

        delegate* unmanaged<XboxObject*, nint, ulong, int> signal = &Signal;
        delegate* unmanaged<XboxObject*, nint, ulong, int> wait = &Wait;
        delegate* unmanaged<XboxObject*, ulong, int> fenceSignal = &FenceSignal;
        objects.SetHandler(XboxInterfaceKind.CommandQueue, QueueSignalSlot, (nint)signal);
        objects.SetHandler(XboxInterfaceKind.CommandQueue, QueueWaitSlot, (nint)wait);
        objects.SetHandler(XboxInterfaceKind.Fence, FenceSignalSlot, (nint)fenceSignal);
    }

    /// <summary>One submission of <paramref name="count"/> lists; <paramref name="frame"/> and <paramref name="flags"/> are the X arguments.</summary>
    public static void Execute(XboxObject* queue, uint count, nint frame, uint flags)
    {
        Write($"queue {QueueId(queue)}: execute {count} list(s), frame 0x{frame:x}, flags 0x{flags:x}");
    }

    [UnmanagedCallersOnly]
    private static int Signal(XboxObject* self, nint fence, ulong value)
    {
        try
        {
            nint inner = ShimContext.Runtime.D3D12X.Objects.Unwrap(fence);
            Write($"queue {QueueId(self)}: signal fence {FenceId(fence)} = {value} (completed {Completed(inner)})");

            return FenceValueMirror.Signal(self, fence, value);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "QueueTrace.Signal");

            return HResult.Fail;
        }
    }

    [UnmanagedCallersOnly]
    private static int Wait(XboxObject* self, nint fence, ulong value)
    {
        try
        {
            nint inner = ShimContext.Runtime.D3D12X.Objects.Unwrap(fence);
            Write($"queue {QueueId(self)}: WAIT fence {FenceId(fence)} >= {value} (completed {Completed(inner)})");

            return ((delegate* unmanaged<nint, nint, ulong, int>)ComObject.Slot(self->Inner, DesktopQueueWaitSlot))(self->Inner, inner, value);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "QueueTrace.Wait");

            return HResult.Fail;
        }
    }

    [UnmanagedCallersOnly]
    private static int FenceSignal(XboxObject* self, ulong value)
    {
        try
        {
            Write($"cpu: signal fence {FenceId((nint)self)} = {value} (completed {Completed(self->Inner)})");

            return ((delegate* unmanaged<nint, ulong, int>)ComObject.Slot(self->Inner, FenceSignalSlot))(self->Inner, value);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "QueueTrace.FenceSignal");

            return HResult.Fail;
        }
    }

    private static ulong Completed(nint fence)
    {
        return ((delegate* unmanaged<nint, ulong>)ComObject.Slot(fence, GetCompletedValueSlot))(fence);
    }

    private static int QueueId(XboxObject* queue)
    {
        int count = Queues.Count;
        int id = IdOf(Queues, (nint)queue);

        if (id == count)
        {
            CommandQueueDescription description;
            _ = ((delegate* unmanaged<nint, CommandQueueDescription*, CommandQueueDescription*>)ComObject.Slot(queue->Inner, DesktopGetDescSlot))(queue->Inner, &description);
            Write($"queue {id} is 0x{(nint)queue:x}: type {description.Type}, priority {description.Priority}, flags 0x{description.Flags:x}");
        }

        return id;
    }

    private static int FenceId(nint fence)
    {
        return IdOf(Fences, fence);
    }

    private static int IdOf(Dictionary<nint, int> ids, nint pointer)
    {
        lock (Gate)
        {
            if (!ids.TryGetValue(pointer, out int id))
            {
                id = ids.Count;
                ids[pointer] = id;
            }

            return id;
        }
    }

    private static void Write(string line)
    {
        if (Interlocked.Increment(ref _lines) > Limit)
        {
            return;
        }

        _log?.Info($"  [queues] t{Environment.CurrentManagedThreadId} {line}");
    }
}
