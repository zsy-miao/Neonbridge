// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>A registered <c>void XTaskQueueMonitorCallback(void* context, XTaskQueueHandle queue, XTaskQueuePort port)</c>.</summary>
internal readonly record struct TaskQueueMonitor(nint Callback, nint Context);
