// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>An XTaskQueue: a work port and a completion port (owned, or borrowed by a composite queue), monitors and waiters.</summary>
internal sealed unsafe class TaskQueue
{
    private int _refCount = 1;
    private volatile bool _terminated;

    public TaskQueue(bool ownsPorts)
    {
        OwnsPorts = ownsPorts;
    }

    public nint Handle { get; set; }

    public TaskPort Work { get; set; } = null!;

    public TaskPort Completion { get; set; } = null!;

    /// <summary>False for a composite queue, whose ports belong to other queues.</summary>
    public bool OwnsPorts { get; }

    public bool Terminated
    {
        get => _terminated;
        set => _terminated = value;
    }

    public Dictionary<ulong, TaskQueueMonitor> Monitors { get; } = [];

    public Dictionary<ulong, RegisteredWaitHandle> Waiters { get; } = [];

    public TaskPort? Port(uint port)
    {
        switch ((TaskQueuePort)port)
        {
            case TaskQueuePort.Work:
                return Work;
            case TaskQueuePort.Completion:
                return Completion;
            default:
                return null;
        }
    }

    public void AddRef()
    {
        Interlocked.Increment(ref _refCount);
    }

    /// <summary>Drops a reference; returns the references left.</summary>
    public int Release()
    {
        return Interlocked.Decrement(ref _refCount);
    }

    /// <summary>Calls the monitors of the port's owner and, for a composite queue, of the queue submitted through.</summary>
    public static void NotifyMonitors(TaskPort port, TaskQueue via)
    {
        Notify(port.Owner, port);

        if (via != port.Owner)
        {
            Notify(via, port);
        }
    }

    private static void Notify(TaskQueue queue, TaskPort port)
    {
        // Most queues have no monitor: skip the lock.
        if (queue.Monitors.Count == 0)
        {
            return;
        }

        TaskQueueMonitor[] monitors;

        lock (queue.Monitors)
        {
            if (queue.Monitors.Count == 0)
            {
                return;
            }

            monitors = [.. queue.Monitors.Values];
        }

        foreach (var monitor in monitors)
        {
            ((delegate* unmanaged<nint, nint, uint, void>)monitor.Callback)(monitor.Context, queue.Handle, (uint)port.Kind);
        }
    }
}
