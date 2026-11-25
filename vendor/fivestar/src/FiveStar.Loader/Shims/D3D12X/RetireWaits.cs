// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;
using FiveStar.Loader.Shims.D3D12X.Binding;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// <c>--retire-waits-all-queues</c>: a test of whether the game recycles transient memory while another queue's work
/// still writes to it. With this on, a submission holding a 64-bit memory wait (slot 74, <see cref="MemoryWaits"/>)
/// first waits on the GPU (desktop queue Wait) for every other queue's work submitted so far. Each such wait is also
/// logged, every <see cref="LogEvery"/>, with each queue's signalled and completed shim fence values.
/// </summary>
internal static unsafe class RetireWaits
{
    /// <summary>ID3D12CommandQueue::Wait and ID3D12Fence::GetCompletedValue (d3d12.h).</summary>
    private const int QueueWaitSlot = 15;
    private const int GetCompletedValueSlot = 8;

    private const int MaxQueues = 32;
    private const int LogEvery = 64;

    private static readonly nint[] Queues = new nint[MaxQueues];
    private static int _queues;
    private static long _writes;
    private static long _waits;
    private static ulong _lastValue;
    private static ulong _lastAddress;
    private static LoaderLog? _log;

    /// <summary>Whether submissions with a slot-74 wait also wait for the other queues.</summary>
    public static bool Enabled { get; private set; }

    public static void Enable(bool enabled, LoaderLog log)
    {
        Enabled = enabled;
        _log = log;
    }

    /// <summary>A queue that got its shim fence (<see cref="ListSubmissions"/>).</summary>
    public static void Register(XboxQueueObject* queue)
    {
        int slot = Interlocked.Increment(ref _queues) - 1;

        if (slot < MaxQueues)
        {
            Volatile.Write(ref Queues[slot], (nint)queue);
        }
    }

    /// <summary>The number of the queue whose shim fence is <paramref name="fence"/>; 0 for none.</summary>
    public static int QueueOf(nint fence)
    {
        int queues = Math.Min(Volatile.Read(ref _queues), MaxQueues);

        for (int i = 0; i < queues; i++)
        {
            var queue = (XboxQueueObject*)Volatile.Read(ref Queues[i]);

            if (queue != null && queue->Fence == fence)
            {
                return queue->Number;
            }
        }

        return 0;
    }

    /// <summary>The value of the last slot-74 wait and its address, for reports.</summary>
    public static (ulong Value, ulong Address) LastWrite => (Volatile.Read(ref _lastValue), Volatile.Read(ref _lastAddress));

    /// <summary>A slot-74 wait for <paramref name="address"/> to reach <paramref name="value"/> recorded in <paramref name="list"/>.</summary>
    [HotPath]
    public static void Recorded(XboxObject* list, ulong address, ulong value)
    {
        Volatile.Write(ref _lastValue, value);
        Volatile.Write(ref _lastAddress, address);

        if (!Enabled)
        {
            return;
        }

        CommandListBindings.Of(list)->RetireWrite = 1;
        long count = Interlocked.Increment(ref _writes);

        if (count % LogEvery == 1)
        {
            Log(count, address, value);
        }
    }

    private static void Log(long count, ulong address, ulong value)
    {
        _log?.Info($"  [retire] wait #{count}: 0x{value:x} to 0x{address:x}; queues (signalled/completed):{Describe()}; {Interlocked.Read(ref _waits)} cross-queue waits so far");
    }

    /// <summary>Before <paramref name="queue"/> runs <paramref name="lists"/>: waits for the other queues when one holds a retire write.</summary>
    [HotPath]
    public static void BeforeExecute(XboxQueueObject* queue, nint* lists, uint count)
    {
        if (!Enabled || !HoldsRetireWrite(lists, count))
        {
            return;
        }

        nint inner = queue->Header.Inner;
        int queues = Volatile.Read(ref _queues);

        if (queues > MaxQueues)
        {
            queues = MaxQueues;
        }

        for (int i = 0; i < queues; i++)
        {
            var other = (XboxQueueObject*)Volatile.Read(ref Queues[i]);

            if (other == null || other == queue || other->Fence == 0)
            {
                continue;
            }

            ulong value = Volatile.Read(ref other->Signaled);
            _ = ((delegate* unmanaged<nint, nint, ulong, int>)ComObject.Slot(inner, QueueWaitSlot))(inner, other->Fence, value);
            Interlocked.Increment(ref _waits);
        }
    }

    [HotPath]
    private static bool HoldsRetireWrite(nint* lists, uint count)
    {
        for (uint i = 0; i < count; i++)
        {
            nint list = lists[i];

            if (list != 0 && ShimContext.Runtime.D3D12X.Objects.Unwrap(list) != list && CommandListBindings.Of((XboxObject*)list)->RetireWrite != 0)
            {
                return true;
            }
        }

        return false;
    }

    private static string Describe()
    {
        var text = new System.Text.StringBuilder();
        int queues = Math.Min(Volatile.Read(ref _queues), MaxQueues);

        for (int i = 0; i < queues; i++)
        {
            var queue = (XboxQueueObject*)Volatile.Read(ref Queues[i]);

            if (queue == null || queue->Fence == 0)
            {
                continue;
            }

            ulong completed = ((delegate* unmanaged<nint, ulong>)ComObject.Slot(queue->Fence, GetCompletedValueSlot))(queue->Fence);
            text.Append(System.Globalization.CultureInfo.InvariantCulture, $" #{queue->Number} {Volatile.Read(ref queue->Signaled)}/{completed}");
        }

        return text.ToString();
    }
}
