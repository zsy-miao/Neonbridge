// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// A queued callback: a native <c>void XTaskQueueCallback(void* context, bool canceled)</c> (XTaskQueue.h), or an
/// async operation's work step when <see cref="Async"/> is set.
/// </summary>
internal readonly unsafe record struct TaskEntry(nint Callback, nint Context, AsyncStep? Async)
{
    public void Invoke(bool canceled)
    {
        if (Async is not null)
        {
            Async.Run(canceled);

            return;
        }

        byte flag = canceled ? GdkModule.True : GdkModule.False;
        ((delegate* unmanaged<nint, byte, void>)Callback)(Context, flag);
    }
}
