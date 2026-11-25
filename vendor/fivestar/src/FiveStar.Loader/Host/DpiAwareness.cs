// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Host;

/// <summary>
/// Makes the process per-monitor DPI aware (v2) before the game makes its window. Unaware, Windows scales the window's
/// pixels by the display's scale factor: the game's 3840x2160 window becomes 5760x3240 on a 150% 4K screen, of which
/// only the top-left part is on screen, and the frame is drawn at a lower resolution and stretched.
/// </summary>
internal static class DpiAwareness
{
    /// <summary>DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2.</summary>
    private const nint PerMonitorV2 = -4;

    public static void Take(LoaderLog log)
    {
        if (User32.SetProcessDpiAwarenessContext(PerMonitorV2) != 0)
        {
            log.Info("  [display] per-monitor DPI aware");

            return;
        }

        log.Info($"  [display] SetProcessDpiAwarenessContext failed (error {Marshal.GetLastPInvokeError()}); awareness stays as it was");
    }
}
