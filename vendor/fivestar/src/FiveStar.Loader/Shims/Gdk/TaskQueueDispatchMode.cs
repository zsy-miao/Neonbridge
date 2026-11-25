// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>XTaskQueueDispatchMode (XTaskQueue.h).</summary>
internal enum TaskQueueDispatchMode : uint
{
    /// <summary>Callbacks run when the title calls XTaskQueueDispatch.</summary>
    Manual = 0,

    /// <summary>Callbacks run on the thread pool, in parallel.</summary>
    ThreadPool = 1,

    /// <summary>Callbacks run on the thread pool one at a time, in order.</summary>
    SerializedThreadPool = 2,

    /// <summary>Callbacks run inline, inside the submit call.</summary>
    Immediate = 3,
}
