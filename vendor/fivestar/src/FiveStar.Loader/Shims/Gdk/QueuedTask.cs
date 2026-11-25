// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>A task on its way to the thread pool: the port it belongs to and the entry to run.</summary>
internal readonly record struct QueuedTask(TaskPort Port, TaskEntry Entry);
