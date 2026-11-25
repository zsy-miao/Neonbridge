// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// The in-process XTaskQueue (XTaskQueue.h): queues and ports, composite queues, delayed callbacks, waiters, monitors,
/// termination and the process task queue. On termination, pending callbacks run with canceled=true on the
/// terminating thread (or a pool thread when not waiting) instead of through their port; a duplicated handle is the
/// same pointer with a reference count.
/// </summary>
internal sealed unsafe class TaskQueueService
{
    private readonly HandleTable<TaskQueue> _queues = new("XTQUEUE");
    private readonly HandleTable<TaskPort> _ports = new("XTQPORT");
    private readonly object _processGate = new();
    private readonly NativeBoundary _boundary;
    private long _token;
    private TaskQueue? _process;
    private bool _processCleared;

    public TaskQueueService(NativeBoundary boundary)
    {
        _boundary = boundary;
    }

    public TaskQueue? Get(nint handle)
    {
        return _queues.Get(handle);
    }

    public TaskQueue CreateQueue(TaskQueueDispatchMode work, TaskQueueDispatchMode completion)
    {
        var queue = new TaskQueue(true);
        queue.Work = new TaskPort(queue, TaskQueuePort.Work, work);
        queue.Completion = new TaskPort(queue, TaskQueuePort.Completion, completion);
        queue.Handle = _queues.Add(queue);
        queue.Work.Handle = _ports.Add(queue.Work);
        queue.Completion.Handle = _ports.Add(queue.Completion);

        return queue;
    }

    /// <summary>The queue of an XAsyncBlock: its own, else the process task queue (created on first use; null once cleared).</summary>
    public TaskQueue? ResolveQueue(nint handle)
    {
        if (handle != 0)
        {
            return _queues.Get(handle);
        }

        lock (_processGate)
        {
            if (_process is null && !_processCleared)
            {
                _process = CreateQueue(TaskQueueDispatchMode.ThreadPool, TaskQueueDispatchMode.ThreadPool);
                _boundary.Trace($"process task queue created 0x{_process.Handle:x} (ThreadPool/ThreadPool)");
            }

            return _process;
        }
    }

    public int Create(uint workMode, uint completionMode, nint* queue)
    {
        if (queue == null || workMode > (uint)TaskQueueDispatchMode.Immediate || completionMode > (uint)TaskQueueDispatchMode.Immediate)
        {
            return HResult.InvalidArgument;
        }

        var created = CreateQueue((TaskQueueDispatchMode)workMode, (TaskQueueDispatchMode)completionMode);
        *queue = created.Handle;
        _boundary.Trace($"XTaskQueueCreate({(TaskQueueDispatchMode)workMode}, {(TaskQueueDispatchMode)completionMode}) -> 0x{created.Handle:x}");

        return HResult.Ok;
    }

    public int GetPort(nint queue, uint port, nint* portHandle)
    {
        var found = Get(queue)?.Port(port);

        if (found is null || portHandle == null)
        {
            return HResult.InvalidArgument;
        }

        *portHandle = found.Handle;

        return HResult.Ok;
    }

    public int CreateComposite(nint workPort, nint completionPort, nint* queue)
    {
        var work = _ports.Get(workPort);
        var completion = _ports.Get(completionPort);

        if (work is null || completion is null || queue == null)
        {
            return HResult.InvalidArgument;
        }

        var composite = new TaskQueue(false) { Work = work, Completion = completion };
        composite.Handle = _queues.Add(composite);
        *queue = composite.Handle;

        return HResult.Ok;
    }

    [HotPath]
    public bool Dispatch(nint queue, uint port, uint timeoutMs)
    {
        var found = Get(queue)?.Port(port);

        return found is not null && found.Dispatch(timeoutMs);
    }

    /// <summary>XTaskQueueSubmitCallback / XTaskQueueSubmitDelayedCallback; queue 0 is the process queue.</summary>
    [HotPath]
    public int Submit(nint queue, uint port, uint delayMs, nint context, nint callback)
    {
        var target = queue == 0 ? ResolveQueue(0) : Get(queue);
        var found = target?.Port(port);

        if (target is null || found is null || callback == 0)
        {
            return HResult.InvalidArgument;
        }

        return found.Submit(new TaskEntry(callback, context, null), target, delayMs);
    }

    /// <summary>
    /// XTaskQueueRegisterWaiter. The wait stays registered (re-armed after each signal) until it is unregistered:
    /// right for auto-reset events; a manual-reset event that stays signaled would repeat.
    /// </summary>
    public int RegisterWaiter(nint queue, uint port, nint waitHandle, nint context, nint callback, ulong* token)
    {
        var target = Get(queue);
        var found = target?.Port(port);

        if (target is null || found is null || waitHandle == 0 || callback == 0 || token == null)
        {
            return HResult.InvalidArgument;
        }

        var waiter = new TaskWaiter(waitHandle, found, target, new TaskEntry(callback, context, null));
        var registration = ThreadPool.UnsafeRegisterWaitForSingleObject(waiter, waiter.OnSignaled, null, Timeout.Infinite, false);
        ulong issued = NextToken();

        lock (target.Waiters)
        {
            target.Waiters[issued] = registration;
        }

        *token = issued;

        return HResult.Ok;
    }

    public void UnregisterWaiter(nint queue, ulong token)
    {
        var target = Get(queue);

        if (target is null)
        {
            return;
        }

        RegisteredWaitHandle? registration;

        lock (target.Waiters)
        {
            target.Waiters.Remove(token, out registration);
        }

        registration?.Unregister(null);
    }

    public int RegisterMonitor(nint queue, nint context, nint callback, ulong* token)
    {
        var target = Get(queue);

        if (target is null || callback == 0 || token == null)
        {
            return HResult.InvalidArgument;
        }

        ulong issued = NextToken();

        lock (target.Monitors)
        {
            target.Monitors[issued] = new TaskQueueMonitor(callback, context);
        }

        *token = issued;

        return HResult.Ok;
    }

    public void UnregisterMonitor(nint queue, ulong token)
    {
        var target = Get(queue);

        if (target is null)
        {
            return;
        }

        lock (target.Monitors)
        {
            target.Monitors.Remove(token);
        }
    }

    public int Duplicate(nint queue, nint* duplicated)
    {
        var target = Get(queue);

        if (target is null || duplicated == null)
        {
            return HResult.InvalidArgument;
        }

        target.AddRef();
        *duplicated = queue;

        return HResult.Ok;
    }

    /// <summary>XTaskQueueCloseHandle: the last reference terminates the queue (without waiting) and frees the handle.</summary>
    public void Close(nint queue)
    {
        var target = Get(queue);

        if (target is null || target.Release() > 0)
        {
            return;
        }

        if (!target.Terminated)
        {
            Terminate(target, false, 0, 0);
        }

        _queues.Remove(queue);
        _boundary.Trace($"XTaskQueueCloseHandle(0x{queue:x}): last reference");
    }

    public int Terminate(nint queue, bool wait, nint context, nint callback)
    {
        var target = Get(queue);

        if (target is null)
        {
            return HResult.InvalidArgument;
        }

        Terminate(target, wait, context, callback);

        return HResult.Ok;
    }

    public static void Terminate(TaskQueue queue, bool wait, nint context, nint callback)
    {
        queue.Terminated = true;

        lock (queue.Waiters)
        {
            foreach (var registration in queue.Waiters.Values)
            {
                registration.Unregister(null);
            }

            queue.Waiters.Clear();
        }

        var request = new TerminationRequest(queue, context, callback);

        if (wait)
        {
            FinishTermination(request);

            return;
        }

        ThreadPool.UnsafeQueueUserWorkItem(FinishTermination, request, false);
    }

    /// <summary>XTaskQueueGetCurrentProcessTaskQueue: the caller receives a reference it closes.</summary>
    public bool GetProcessQueue(nint* queue)
    {
        var process = ResolveQueue(0);

        if (queue == null)
        {
            return false;
        }

        if (process is null)
        {
            *queue = 0;

            return false;
        }

        process.AddRef();
        *queue = process.Handle;

        return true;
    }

    public void SetProcessQueue(nint queue)
    {
        lock (_processGate)
        {
            var target = Get(queue);
            target?.AddRef();
            _process = target;
            _processCleared = target is null;
        }
    }

    /// <summary>bool XTaskQueueIsEmpty(XTaskQueueHandle queue, XTaskQueuePort port): true for an unknown queue or port.</summary>
    public bool IsEmpty(nint queue, uint port)
    {
        var found = Get(queue)?.Port(port);

        return found is null || found.IsEmpty;
    }

    private static void FinishTermination(TerminationRequest request)
    {
        var queue = request.Queue;

        if (queue.OwnsPorts)
        {
            queue.Work.CancelAll();
            queue.Completion.CancelAll();
        }

        if (request.Callback != 0)
        {
            ((delegate* unmanaged<nint, void>)request.Callback)(request.Context);
        }
    }

    private ulong NextToken()
    {
        return (ulong)Interlocked.Increment(ref _token);
    }
}
