// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Diagnostics;

/// <summary>
/// Holds the game's hang watchdog paused while the loader blocks a game thread on purpose (docs/debugging.md#hang-watchdog):
/// sets the pause flag the game itself sets around long work (0x1471b6330) and puts its earlier value back when the last
/// holder leaves, unless the game changed it meanwhile. Game data, not code; nothing without the game's image.
/// </summary>
internal static unsafe class WatchdogPause
{
    private const ulong PausedAddress = 0x1471B6330;
    private const byte Paused = 1;

    private static readonly object Gate = new();
    private static int _holders;
    private static byte _before;
    private static bool _set;

    /// <summary>Pauses the watchdog until the result is disposed.</summary>
    public static Hold Enter()
    {
        lock (Gate)
        {
            if (_holders++ == 0 && GameHangWatch.IsReadable(PausedAddress))
            {
                _before = *(byte*)PausedAddress;
                *(byte*)PausedAddress = Paused;
                _set = true;
            }
        }

        return default;
    }

    private static void Leave()
    {
        lock (Gate)
        {
            if (--_holders > 0 || !_set)
            {
                return;
            }

            _set = false;

            if (*(byte*)PausedAddress == Paused)
            {
                *(byte*)PausedAddress = _before;
            }
        }
    }

    /// <summary>Ends a pause taken by <see cref="Enter"/>.</summary>
    internal readonly struct Hold : IDisposable
    {
        public void Dispose()
        {
            Leave();
        }
    }
}
