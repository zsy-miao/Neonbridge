// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Microsoft.Win32.SafeHandles;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// An XTaskQueueRegisterWaiter registration: a thread-pool wait on the title's handle that queues the callback on the
/// port each time the handle is signaled.
/// </summary>
internal sealed class TaskWaiter : WaitHandle
{
    private readonly TaskPort _port;
    private readonly TaskQueue _queue;
    private readonly TaskEntry _entry;

    public TaskWaiter(nint handle, TaskPort port, TaskQueue queue, TaskEntry entry)
    {
        SafeWaitHandle = new SafeWaitHandle(handle, false);
        _port = port;
        _queue = queue;
        _entry = entry;
    }

    public void OnSignaled(object? state, bool timedOut)
    {
        _port.Submit(_entry, _queue, 0);
    }
}
