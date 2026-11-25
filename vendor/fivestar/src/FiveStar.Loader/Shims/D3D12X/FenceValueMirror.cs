// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;
using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// Fence slot 12 returns the address of the fence's completed value, which the game then polls (wait loop at
/// 0x144fb0ad0; UINT64_MAX means removed). The desktop only answers GetCompletedValue, so a mirror thread keeps one
/// cell per such fence current: each fence has SetEventOnCompletion(value + 1) on a shared event, and every wake-up
/// re-reads all fences. Released fences leave the set first.
/// </summary>
internal sealed unsafe class FenceValueMirror : IDisposable
{
    public const int CompletedValueAddressSlot = 12;

    /// <summary>ID3D12Fence::GetCompletedValue and ::SetEventOnCompletion (d3d12.h).</summary>
    private const int GetCompletedValueSlot = 8;
    private const int SetEventOnCompletionSlot = 9;

    /// <summary>X queue slot 16 (Signal) is desktop ID3D12CommandQueue slot 14 (d3d12.h).</summary>
    public const int QueueSignalSlot = 16;
    private const int DesktopQueueSignalSlot = 14;

    /// <summary>X queue slot 17 (Wait) is desktop slot 15.</summary>
    public const int QueueWaitSlot = 17;
    private const int DesktopQueueWaitSlot = 15;

    /// <summary>A re-read even without a completion, in case an event registration was missed.</summary>
    private const int IdleMilliseconds = 16;

    private readonly object _gate = new();
    private readonly HashSet<nint> _fences = [];
    private readonly AutoResetEvent _changed = new(false);
    private Thread? _thread;
    private volatile bool _stopping;
    private long _longestLatency;

    /// <summary>Adds the fence (once) and returns its cell, current as of now.</summary>
    public ulong* Track(XboxFenceObject* fence)
    {
        lock (_gate)
        {
            if (_fences.Add((nint)fence))
            {
                Refresh(fence, true);
                EnsureThread();
            }
        }

        return &fence->CompletedValue;
    }

    /// <summary>Takes a fence out before its wrapper is freed.</summary>
    public void Forget(XboxFenceObject* fence)
    {
        lock (_gate)
        {
            _fences.Remove((nint)fence);
        }
    }

    /// <summary>Each tracked fence as "cell/completed", for logs.</summary>
    public string Describe()
    {
        var text = new System.Text.StringBuilder();

        lock (_gate)
        {
            foreach (nint fence in _fences)
            {
                nint inner = ((XboxFenceObject*)fence)->Header.Inner;
                ulong completed = ((delegate* unmanaged<nint, ulong>)ComObject.Slot(inner, GetCompletedValueSlot))(inner);
                text.Append(System.Globalization.CultureInfo.InvariantCulture, $" {Volatile.Read(ref ((XboxFenceObject*)fence)->CompletedValue)}/{completed}");
            }
        }

        return text.ToString();
    }

    /// <summary>
    /// The fence whose cell is at <paramref name="cell"/> (a game wait's target, --watch-hang): its wrapper, the cell, the
    /// desktop completed value and the last value a queue Signal asked for; empty when no tracked fence has that cell.
    /// </summary>
    public string DescribeCell(ulong cell)
    {
        lock (_gate)
        {
            foreach (nint fence in _fences)
            {
                var wrapper = (XboxFenceObject*)fence;

                if ((ulong)&wrapper->CompletedValue != cell)
                {
                    continue;
                }

                nint inner = wrapper->Header.Inner;
                ulong completed = ((delegate* unmanaged<nint, ulong>)ComObject.Slot(inner, GetCompletedValueSlot))(inner);
                double ago = Stopwatch.GetElapsedTime(Volatile.Read(ref wrapper->SignaledAt)).TotalSeconds;

                return $"fence 0x{fence:x}: cell {Volatile.Read(ref wrapper->CompletedValue)}, desktop completed {completed}, last Signal {Volatile.Read(ref wrapper->SignaledValue)} ({ago:F1} s ago)";
            }
        }

        return string.Empty;
    }

    /// <summary>The fence wrapper whose completed-value cell is at <paramref name="cell"/>, or 0.</summary>
    public nint FenceAt(ulong cell)
    {
        lock (_gate)
        {
            foreach (nint fence in _fences)
            {
                if ((ulong)&((XboxFenceObject*)fence)->CompletedValue == cell)
                {
                    return fence;
                }
            }
        }

        return 0;
    }

    /// <summary>A game thread waits for the fence at <paramref name="cell"/> to reach <paramref name="target"/>: the fence and the work it waits for.</summary>
    public string DescribeWait(ulong cell, ulong target)
    {
        return $"{DescribeCell(cell)}; {QueueProgress.DescribeFenceWait(FenceAt(cell), target)}";
    }

    public void Dispose()
    {
        _stopping = true;
        _changed.Set();
        _thread?.Join();
        _changed.Dispose();
    }

    /// <summary>Fence slot 12: the address the game reads the completed value from.</summary>
    [UnmanagedCallersOnly]
    public static ulong* CompletedValueAddress(XboxFenceObject* self)
    {
        try
        {
            return ShimContext.Runtime.D3D12X.Fences.Track(self);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "FenceValueMirror.CompletedValueAddress");

            return default;
        }
    }

    /// <summary>The longest Signal-to-cell time (Stopwatch ticks) since the last call, then starts over.</summary>
    public long TakeLongestLatency()
    {
        return Interlocked.Exchange(ref _longestLatency, 0);
    }

    /// <summary>Queue slot 16, Signal(fence, value): notes when the value was asked for, then forwards.</summary>
    [HotPath]
    [UnmanagedCallersOnly]
    public static int QueueSignal(XboxObject* queue, nint fence, ulong value)
    {
        try
        {
            return Signal(queue, fence, value);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "FenceValueMirror.QueueSignal");

            return HResult.Fail;
        }
    }

    /// <summary>Notes the Signal on a wrapped fence and forwards it to the desktop queue.</summary>
    [HotPath]
    public static int Signal(XboxObject* queue, nint fence, ulong value)
    {
        nint inner = ShimContext.Runtime.D3D12X.Objects.Unwrap(fence);

        if (inner != fence)
        {
            var wrapper = (XboxFenceObject*)fence;
            Volatile.Write(ref wrapper->SignaledAt, Stopwatch.GetTimestamp());
            Volatile.Write(ref wrapper->SignaledValue, value);
        }

        QueueProgress.FenceCall(queue, QueueEventKind.Signal, fence, value);

        return ((delegate* unmanaged<nint, nint, ulong, int>)ComObject.Slot(queue->Inner, DesktopQueueSignalSlot))(queue->Inner, inner, value);
    }

    /// <summary>Queue slot 17, Wait(fence, value): noted for <see cref="QueueProgress"/>, then desktop Wait.</summary>
    [HotPath]
    [UnmanagedCallersOnly]
    public static int QueueWait(XboxObject* queue, nint fence, ulong value)
    {
        try
        {
            nint inner = ShimContext.Runtime.D3D12X.Objects.Unwrap(fence);
            QueueProgress.FenceCall(queue, QueueEventKind.Wait, fence, value);

            return ((delegate* unmanaged<nint, nint, ulong, int>)ComObject.Slot(queue->Inner, DesktopQueueWaitSlot))(queue->Inner, inner, value);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "FenceValueMirror.QueueWait");

            return HResult.Fail;
        }
    }

    /// <summary>Fence slot 2: Release that drops the fence from the mirror with the last reference.</summary>
    [UnmanagedCallersOnly]
    public static uint ReleaseFence(XboxFenceObject* self)
    {
        try
        {
            nint inner = self->Header.Inner;
            uint left = ((delegate* unmanaged<nint, uint>)ComObject.Slot(inner, ComObject.ReleaseSlot))(inner);

            if (left == 0)
            {
                ShimContext.Runtime.D3D12X.Fences.Forget(self);
                NativeMemory.Free(self);
            }

            return left;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "FenceValueMirror.ReleaseFence");

            return default;
        }
    }

    private void NoteLatency(XboxFenceObject* fence, ulong value)
    {
        long at = Volatile.Read(ref fence->SignaledAt);

        if (at == 0 || value == ulong.MaxValue || value < Volatile.Read(ref fence->SignaledValue))
        {
            return;
        }

        long latency = Stopwatch.GetTimestamp() - at;

        if (latency > Volatile.Read(ref _longestLatency))
        {
            Volatile.Write(ref _longestLatency, latency);
        }
    }

    private void EnsureThread()
    {
        if (_thread is not null)
        {
            return;
        }

        _thread = new Thread(Run) { IsBackground = true, Name = "fivestar fence mirror" };
        _thread.Start();
    }

    private void Run()
    {
        while (!_stopping)
        {
            _changed.WaitOne(IdleMilliseconds);

            lock (_gate)
            {
                foreach (nint fence in _fences)
                {
                    Refresh((XboxFenceObject*)fence, false);
                }
            }
        }
    }

    /// <summary>Copies the completed value into the cell; when it moved, waits for the next one on the shared event.</summary>
    private void Refresh(XboxFenceObject* fence, bool first)
    {
        nint inner = fence->Header.Inner;
        ulong value = ((delegate* unmanaged<nint, ulong>)ComObject.Slot(inner, GetCompletedValueSlot))(inner);

        if (!first && value == Volatile.Read(ref fence->CompletedValue))
        {
            return;
        }

        Volatile.Write(ref fence->CompletedValue, value);
        NoteLatency(fence, value);

        if (value != ulong.MaxValue)
        {
            nint handle = _changed.SafeWaitHandle.DangerousGetHandle();
            _ = ((delegate* unmanaged<nint, ulong, nint, int>)ComObject.Slot(inner, SetEventOnCompletionSlot))(inner, value + 1, handle);
        }
    }
}
