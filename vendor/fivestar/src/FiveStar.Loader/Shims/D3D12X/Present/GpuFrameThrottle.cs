// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X.Present;

/// <summary>
/// Keeps the game at most <see cref="Lag"/> presents ahead of the GPU. The frame events of slot 80 come from a timer
/// (<see cref="FramePacer"/>), so on their own they let the CPU run ahead of a GPU that falls behind, as during the
/// story load: the game then retires transient memory by frame count while GPU passes of earlier frames still write
/// into it (argument records over the pool's block-end nodes, docs/d3d12x-shim.md). At each present the throttle notes
/// how far every queue has been submitted (its shim fence, <see cref="ListSubmissions"/>) and waits until the GPU has
/// finished everything submitted up to the present <see cref="Lag"/> before. A wait longer than 5 s (by default)
/// gives up (counted), so a lost fence cannot freeze the game. A wait past <see cref="ReleaseWaitsMilliseconds"/>
/// releases the queues' memory waits (<see cref="MemoryWaits.ReleaseAll"/>): the console has no such throttle, and the
/// writer a held queue waits for may be work the game submits after this present (a 5 s stall at one story-load frame).
/// </summary>
internal sealed unsafe class GpuFrameThrottle : IDisposable
{
    /// <summary>Presents the game may be ahead of the GPU.</summary>
    public const int Lag = 2;

    /// <summary>Queues followed; the game makes a handful.</summary>
    private const int MaxQueues = 32;

    /// <summary>How long a present waits for the GPU before the queues' memory waits are released.</summary>
    public const long ReleaseWaitsMilliseconds = 250;

    /// <summary>The default longest wait of a present for the GPU.</summary>
    public const uint DefaultLongestWaitMilliseconds = 5000;

    /// <summary>ID3D12Fence::GetCompletedValue and SetEventOnCompletion (d3d12.h).</summary>
    private const int GetCompletedValueSlot = 8;
    private const int SetEventOnCompletionSlot = 9;

    private readonly object _gate = new();
    private readonly nint[] _queues = new nint[MaxQueues];
    private readonly ulong[][] _marks = [new ulong[MaxQueues], new ulong[MaxQueues], new ulong[MaxQueues]];
    private readonly nint _event;
    private readonly uint _longestWait;
    private int _queueCount;
    private long _presents;
    private long _waits;
    private long _givenUp;

    public GpuFrameThrottle()
        : this(DefaultLongestWaitMilliseconds)
    {
    }

    /// <param name="longestWait">Milliseconds a present waits for the GPU at most before it goes on.</param>
    internal GpuFrameThrottle(uint longestWait)
    {
        _longestWait = longestWait;
        _event = Kernel32.CreateEventW(0, 0, 0, null);
    }

    /// <summary>Presents that waited for the GPU, and waits that gave up.</summary>
    public string Summary => $"{Interlocked.Read(ref _waits)} presents waited for the GPU ({Interlocked.Read(ref _givenUp)} gave up after {_longestWait} ms)";

    /// <summary>A queue whose submissions are followed (its shim fence exists); added once.</summary>
    public void Follow(XboxQueueObject* queue)
    {
        lock (_gate)
        {
            for (int i = 0; i < _queueCount; i++)
            {
                if (_queues[i] == (nint)queue)
                {
                    return;
                }
            }

            if (_queueCount < MaxQueues)
            {
                _queues[_queueCount++] = (nint)queue;
            }
        }
    }

    /// <summary>At a present: notes every queue's submissions, then waits for those of the present <see cref="Lag"/> before.</summary>
    public void Presented()
    {
        long present = Interlocked.Increment(ref _presents);
        Binding.PassControls.Rules.NextFrame();
        var marks = _marks[present % _marks.Length];
        bool waited = false;
        bool gaveUp = false;
        int count;

        lock (_gate)
        {
            count = _queueCount;

            for (int i = 0; i < count; i++)
            {
                marks[i] = Volatile.Read(ref ((XboxQueueObject*)_queues[i])->Signaled);
            }
        }

        if (present <= Lag)
        {
            return;
        }

        var due = _marks[(present - Lag) % _marks.Length];

        for (int i = 0; i < count; i++)
        {
            var queue = (XboxQueueObject*)_queues[i];
            var outcome = WaitFor(queue->Fence, due[i]);
            waited |= outcome != Outcome.Done;
            gaveUp |= outcome == Outcome.GaveUp;
        }

        if (waited)
        {
            Interlocked.Increment(ref _waits);
        }

        if (gaveUp)
        {
            Interlocked.Increment(ref _givenUp);
        }
    }

    /// <summary>Every followed queue's shim fence and the value it has been submitted to now: what <see cref="Passed"/> checks.</summary>
    public (nint Fence, ulong Value)[] Submitted()
    {
        lock (_gate)
        {
            var marks = new (nint Fence, ulong Value)[_queueCount];

            for (int i = 0; i < _queueCount; i++)
            {
                var queue = (XboxQueueObject*)_queues[i];
                marks[i] = (queue->Fence, Volatile.Read(ref queue->Signaled));
            }

            return marks;
        }
    }

    /// <summary>Whether the GPU has finished everything submitted when <paramref name="marks"/> were taken (<see cref="Submitted"/>).</summary>
    public static bool Passed((nint Fence, ulong Value)[] marks)
    {
        foreach (var (fence, value) in marks)
        {
            if (fence != 0 && value != 0 && Completed(fence) < value)
            {
                return false;
            }
        }

        return true;
    }

    public void Dispose()
    {
        if (_event != 0)
        {
            _ = Kernel32.CloseHandle(_event);
        }
    }

    /// <summary>Waits until <paramref name="fence"/> reaches <paramref name="value"/>, at most the longest wait.</summary>
    internal Outcome WaitFor(nint fence, ulong value)
    {
        if (fence == 0 || value == 0 || Completed(fence) >= value)
        {
            return Outcome.Done;
        }

        long start = Environment.TickCount64;
        long deadline = start + _longestWait;
        bool released = false;
        _ = ((delegate* unmanaged<nint, ulong, nint, int>)ComObject.Slot(fence, SetEventOnCompletionSlot))(fence, value, _event);

        // The event is shared: a wake-up may be an earlier wait's, so the value is checked each time.
        while (Completed(fence) < value)
        {
            long now = Environment.TickCount64;
            long left = deadline - now;

            if (left <= 0)
            {
                return Outcome.GaveUp;
            }

            if (!released && now - start >= ReleaseWaitsMilliseconds)
            {
                released = true;
                MemoryWaits.ReleaseAll();
            }

            long slice = Math.Min(left, ReleaseWaitsMilliseconds);
            _ = PresentNative.WaitForSingleObject(_event, (uint)slice);
        }

        return Outcome.Waited;
    }

    private static ulong Completed(nint fence)
    {
        return ((delegate* unmanaged<nint, ulong>)ComObject.Slot(fence, GetCompletedValueSlot))(fence);
    }

    /// <summary>How a wait for the GPU ended.</summary>
    internal enum Outcome
    {
        Done,
        Waited,
        GaveUp,
    }
}
