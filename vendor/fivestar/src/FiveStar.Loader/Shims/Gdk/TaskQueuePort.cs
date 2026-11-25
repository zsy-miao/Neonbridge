// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>XTaskQueuePort (XTaskQueue.h).</summary>
internal enum TaskQueuePort : uint
{
    Work = 0,
    Completion = 1,
}
