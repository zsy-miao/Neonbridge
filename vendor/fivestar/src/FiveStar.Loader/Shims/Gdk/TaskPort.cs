// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// One port (work or completion) of a task queue and its dispatch mode: Manual (run by XTaskQueueDispatch),
/// ThreadPool (the .NET thread pool), SerializedThreadPool (one at a time, in order, on the pool) or Immediate (inline).
/// </summary>
internal sealed class TaskPort : IDisposable
{
    private readonly object _gate = new();
    private readonly ConcurrentQueue<TaskEntry> _pending = new();
    private readonly SemaphoreSlim _available = new(0);
    private readonly HashSet<DelayedTask> _timers = [];
    private int _serialRunning;
    private int _inFlight;

    public TaskPort(TaskQueue owner, TaskQueuePort kind, TaskQueueDispatchMode mode)
    {
        Owner = owner;
        Kind = kind;
        Mode = mode;
    }

    public TaskQueue Owner { get; }

    public TaskQueuePort Kind { get; }

    public TaskQueueDispatchMode Mode { get; }

    /// <summary>The XTaskQueuePortHandle of this port.</summary>
    public nint Handle { get; set; }

    public bool IsEmpty
    {
        get
        {
            lock (_gate)
            {
                return _pending.IsEmpty && Volatile.Read(ref _inFlight) == 0 && _timers.Count == 0 && Volatile.Read(ref _serialRunning) == 0;
            }
        }
    }

    /// <summary>Queues <paramref name="entry"/>, after <paramref name="delayMs"/> when non-zero; E_ABORT once terminated.</summary>
    [HotPath]
    public int Submit(TaskEntry entry, TaskQueue via, uint delayMs)
    {
        if (Owner.Terminated || via.Terminated)
        {
            return HResult.Abort;
        }

        if (delayMs > 0)
        {
            SubmitDelayed(entry, via, delayMs);

            return HResult.Ok;
        }

        Enqueue(entry, via);

        return HResult.Ok;
    }

    /// <summary>XTaskQueueDispatch on a manual port: runs one callback, waiting up to <paramref name="timeoutMs"/> for one.</summary>
    [HotPath]
    public bool Dispatch(uint timeoutMs)
    {
        if (Mode != TaskQueueDispatchMode.Manual)
        {
            return false;
        }

        if (!_available.Wait(ToTimeout(timeoutMs)))
        {
            return false;
        }

        if (!_pending.TryDequeue(out var entry))
        {
            return false;
        }

        entry.Invoke(false);

        return true;
    }

    /// <summary>Termination: drops the timers, delivers every pending callback with canceled=true, waits for running ones.</summary>
    public void CancelAll()
    {
        lock (_gate)
        {
            foreach (var delayed in _timers)
            {
                delayed.Timer.Dispose();
            }

            _timers.Clear();
        }

        while (_pending.TryDequeue(out var entry))
        {
            entry.Invoke(true);
        }

        var spin = default(SpinWait);

        while (Volatile.Read(ref _inFlight) > 0 || Volatile.Read(ref _serialRunning) != 0)
        {
            spin.SpinOnce();
        }
    }

    /// <summary>Called by a delayed task's timer; ignored when termination dropped the timer.</summary>
    public void FireDelayed(DelayedTask delayed, TaskEntry entry, TaskQueue via)
    {
        lock (_gate)
        {
            if (!_timers.Remove(delayed))
            {
                return;
            }
        }

        delayed.Timer.Dispose();
        Enqueue(entry, via);
    }

    public void Dispose()
    {
        _available.Dispose();
    }

    /// <summary>XTaskQueueDispatch's timeout: UINT32_MAX waits forever, anything else is capped at int.MaxValue.</summary>
    private static int ToTimeout(uint timeoutMs)
    {
        if (timeoutMs == uint.MaxValue)
        {
            return Timeout.Infinite;
        }

        return (int)Math.Min(timeoutMs, int.MaxValue);
    }

    private static void RunPooled(QueuedTask task)
    {
        try
        {
            task.Entry.Invoke(false);
        }
        finally
        {
            Interlocked.Decrement(ref task.Port._inFlight);
        }
    }

    /// <summary>Runs the queued entries one at a time; ends when the queue is empty and no submit restarted it.</summary>
    private static void DrainSerialized(TaskPort port)
    {
        while (true)
        {
            while (port._pending.TryDequeue(out var entry))
            {
                entry.Invoke(false);
            }

            Volatile.Write(ref port._serialRunning, 0);

            if (port._pending.IsEmpty || Interlocked.CompareExchange(ref port._serialRunning, 1, 0) != 0)
            {
                return;
            }
        }
    }

    private void SubmitDelayed(TaskEntry entry, TaskQueue via, uint delayMs)
    {
        var delayed = new DelayedTask(this, entry, via);

        lock (_gate)
        {
            _timers.Add(delayed);
        }

        delayed.Timer.Change(delayMs, Timeout.Infinite);
    }

    [HotPath]
    private void Enqueue(TaskEntry entry, TaskQueue via)
    {
        switch (Mode)
        {
            case TaskQueueDispatchMode.Immediate:
                RunImmediate(entry);
                break;
            case TaskQueueDispatchMode.ThreadPool:
                RunOnPool(entry);
                break;
            case TaskQueueDispatchMode.SerializedThreadPool:
                EnqueueSerialized(entry);
                break;
            default:
                EnqueueManual(entry);
                break;
        }

        TaskQueue.NotifyMonitors(this, via);
    }

    [HotPath]
    private static void RunImmediate(TaskEntry entry)
    {
        entry.Invoke(false);
    }

    [HotPath]
    private void RunOnPool(TaskEntry entry)
    {
        Interlocked.Increment(ref _inFlight);
        ThreadPool.UnsafeQueueUserWorkItem(RunPooled, new QueuedTask(this, entry), false);
    }

    [HotPath]
    private void EnqueueSerialized(TaskEntry entry)
    {
        _pending.Enqueue(entry);

        if (Interlocked.CompareExchange(ref _serialRunning, 1, 0) == 0)
        {
            ThreadPool.UnsafeQueueUserWorkItem(DrainSerialized, this, false);
        }
    }

    [HotPath]
    private void EnqueueManual(TaskEntry entry)
    {
        _pending.Enqueue(entry);
        _available.Release();
    }
}
