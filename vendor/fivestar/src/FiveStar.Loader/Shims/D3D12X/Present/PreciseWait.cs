// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;

namespace FiveStar.Loader.Shims.D3D12X.Present;

/// <summary>Sleeps until a Stopwatch timestamp on a high-resolution waitable timer, without allocating.</summary>
internal static unsafe class PreciseWait
{
    /// <summary>Waitable-timer due times are in 100 ns units.</summary>
    private const long TimerUnitsPerSecond = 10_000_000;

    private const long MillisecondsPerSecond = 1000;

    /// <summary>Returns at or after <paramref name="timestamp"/> (<see cref="Stopwatch.GetTimestamp"/> units).</summary>
    [HotPath]
    public static void Until(long timestamp)
    {
        long remaining = timestamp - Stopwatch.GetTimestamp();

        if (remaining <= 0)
        {
            return;
        }

        long due = -(remaining * TimerUnitsPerSecond / Stopwatch.Frequency);

        if (due == 0)
        {
            return;
        }

        nint timer = PresentNative.CreateWaitableTimerExW(0, null, PresentNative.HighResolutionTimer, PresentNative.TimerAllAccess);

        if (timer == 0)
        {
            Thread.Sleep((int)(remaining * MillisecondsPerSecond / Stopwatch.Frequency));

            return;
        }

        if (PresentNative.SetWaitableTimer(timer, &due, 0, 0, 0, 0) != 0)
        {
            _ = PresentNative.WaitForSingleObject(timer, PresentNative.Infinite);
        }

        _ = PresentNative.CloseHandle(timer);
    }
}
