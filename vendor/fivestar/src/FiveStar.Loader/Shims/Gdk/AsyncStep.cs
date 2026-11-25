// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>A managed step of an async operation that runs through a task queue port.</summary>
internal abstract class AsyncStep
{
    /// <param name="canceled">True when the queue was terminated before the step could run.</param>
    public abstract void Run(bool canceled);
}
