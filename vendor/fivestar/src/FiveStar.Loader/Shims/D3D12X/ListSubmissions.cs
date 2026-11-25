// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;
using FiveStar.Loader.Shims.D3D12X.DebugLayer;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// The console runs a closed command list again while its previous submission is still executing: the game records
/// a few lists once at device creation (X slots 131 and 132, 0x142478ba4) and submits one of them with every present
/// (0x142482660). The desktop does not allow that (the debug layer drops the submission, error #553), so each queue
/// signals a fence of its own after every submission, each list remembers that value, and a list still in flight
/// is waited for on the CPU before it runs again.
/// </summary>
internal static unsafe class ListSubmissions
{
    /// <summary>ID3D12Fence::GetCompletedValue and ::SetEventOnCompletion; ID3D12CommandQueue::Signal (d3d12.h).</summary>
    private const int GetCompletedValueSlot = 8;
    private const int SetEventOnCompletionSlot = 9;
    private const int QueueSignalSlot = 14;

    /// <summary>ID3D12Device::CreateFence.</summary>
    private const int CreateFenceSlot = 36;

    private static readonly Guid FenceIid = new("0a753dcf-c4d8-4b91-adf6-be5a60d95a76");

    private const long LongWaitMilliseconds = 100;
    private const long LongWaitsLogged = 16;

    private static long _waits;
    private static long _longWaits;

    /// <summary>Submissions that had to wait for the list's previous run.</summary>
    public static long Waits => Interlocked.Read(ref _waits);

    /// <summary>Before a submission: blocks until the previous run of <paramref name="list"/> (a wrapper) has finished.</summary>
    [HotPath]
    public static void WaitIfRunning(nint list)
    {
        var submission = ListSubmission.Of(list);
        nint fence = submission->Fence;

        if (fence == 0 || Completed(fence) >= submission->Value)
        {
            return;
        }

        Interlocked.Increment(ref _waits);
        long start = Environment.TickCount64;
        MemoryWaits.ReleaseBehind(fence);

        // A null event makes SetEventOnCompletion return once the value is reached (or the device is removed).
        _ = ((delegate* unmanaged<nint, ulong, nint, int>)ComObject.Slot(fence, SetEventOnCompletionSlot))(fence, submission->Value, 0);
        long waited = Environment.TickCount64 - start;

        if (waited >= LongWaitMilliseconds && Interlocked.Increment(ref _longWaits) <= LongWaitsLogged)
        {
            WarnLongWait(list, waited);
        }
    }

    private static void WarnLongWait(nint list, long waited)
    {
        ShimContext.Runtime.Log.Warn($"[d3d12_xs] submitting list 0x{list:x} waited {waited} ms on the CPU for its previous run (thread {Environment.CurrentManagedThreadId})");
    }

    /// <summary>After a submission: signals the queue's fence and records the value in every wrapped list.</summary>
    [HotPath]
    public static void Submitted(XboxQueueObject* queue, nint* lists, uint count, XboxObjects objects)
    {
        if (queue->Fence == 0 && !CreateFence(queue, objects.DesktopDevice))
        {
            return;
        }

        ulong value = Interlocked.Increment(ref queue->Signaled);
        nint inner = queue->Header.Inner;
        _ = ((delegate* unmanaged<nint, nint, ulong, int>)ComObject.Slot(inner, QueueSignalSlot))(inner, queue->Fence, value);

        for (uint i = 0; i < count; i++)
        {
            nint list = lists[i];

            if (list != 0 && objects.Unwrap(list) != list)
            {
                *ListSubmission.Of(list) = new ListSubmission { Fence = queue->Fence, Value = value };
                GpuBreadcrumbs.Submitted(list, queue->Fence, value, (int)i);
            }
        }

        QueueProgress.Executed(queue, lists, count, value, objects);
    }

    private static ulong Completed(nint fence)
    {
        return ((delegate* unmanaged<nint, ulong>)ComObject.Slot(fence, GetCompletedValueSlot))(fence);
    }

    private static bool CreateFence(XboxQueueObject* queue, nint device)
    {
        if (device == 0)
        {
            return false;
        }

        Guid iid = FenceIid;
        nint fence = 0;
        int hr = ((delegate* unmanaged<nint, ulong, int, Guid*, nint*, int>)ComObject.Slot(device, CreateFenceSlot))(device, 0, 0, &iid, &fence);

        if (HResult.Failed(hr))
        {
            return false;
        }

        queue->Fence = fence;
        RetireWaits.Register(queue);
        ShimContext.Runtime.D3D12X.FrameThrottle.Follow(queue);

        return true;
    }
}
