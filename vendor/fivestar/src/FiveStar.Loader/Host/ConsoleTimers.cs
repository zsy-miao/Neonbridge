// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Host;

/// <summary>
/// Gives the process the console's millisecond waits, for every sleep and timed wait of every thread. The game paces
/// threads with short sleeps: its audio mixer sleeps about 4 ms per 256-frame block (5.3 ms at 48 kHz). A desktop
/// process sleeps in 15.6 ms timer ticks unless it asks for finer ones, and then the mixer makes a third of the audio
/// the output plays. Windows 11 also drops the request while the window is hidden, unless the process opts out of that
/// throttling. Taken before any mapped code runs and given back when the run ends (<see cref="Dispose"/>).
/// </summary>
internal sealed unsafe class ConsoleTimers : IDisposable
{
    /// <summary>ProcessPowerThrottling in PROCESS_INFORMATION_CLASS.</summary>
    private const int ProcessPowerThrottling = 4;

    /// <summary>PROCESS_POWER_THROTTLING_CURRENT_VERSION.</summary>
    private const uint ThrottlingVersion = 1;

    /// <summary>PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION.</summary>
    private const uint IgnoreTimerResolution = 4;

    /// <summary>The u32 fields of PROCESS_POWER_THROTTLING_STATE.</summary>
    private const int StateWords = 3;

    private const uint Milliseconds = 1;

    private readonly bool _taken;

    private ConsoleTimers(bool taken)
    {
        _taken = taken;
    }

    public static ConsoleTimers Take(LoaderLog log)
    {
        // PROCESS_POWER_THROTTLING_STATE: version, control mask, state mask; a clear state bit keeps the resolution.
        uint* state = stackalloc uint[] { ThrottlingVersion, IgnoreTimerResolution, 0 };
        bool kept = Kernel32.SetProcessInformation(Kernel32.GetCurrentProcess(), ProcessPowerThrottling, state, StateWords * sizeof(uint)) != 0;
        bool fine = Winmm.timeBeginPeriod(Milliseconds) == 0;
        string hidden = kept ? "kept while the window is hidden" : "may be dropped while the window is hidden";
        string resolution = fine ? $"{Milliseconds} ms timer resolution" : "timeBeginPeriod failed: 15.6 ms timer ticks";

        log.Info($"  [timer] {resolution}, {hidden}");

        return new ConsoleTimers(fine);
    }

    /// <summary>The desktop's timer ticks (<c>--desktop-timers</c>): nothing is taken.</summary>
    public static ConsoleTimers Desktop(LoaderLog log)
    {
        log.Info("  [timer] desktop timer ticks (--desktop-timers)");

        return new ConsoleTimers(false);
    }

    public void Dispose()
    {
        if (_taken)
        {
            _ = Winmm.timeEndPeriod(Milliseconds);
        }
    }
}
