// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>A queue to finish terminating and its <c>XTaskQueueTerminatedCallback(void* context)</c> (0 for none).</summary>
internal readonly record struct TerminationRequest(TaskQueue Queue, nint Context, nint Callback);
