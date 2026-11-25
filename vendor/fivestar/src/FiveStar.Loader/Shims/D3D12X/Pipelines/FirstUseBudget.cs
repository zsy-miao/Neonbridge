// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// The time the game's threads may spend making pipelines at their first use, shared by all of them: a bucket that
/// fills by <see cref="RefillPerSecond"/> of a second each second up to <see cref="Capacity"/>. The game renders a
/// loading-screen frame on five Subrender workers and its hang watchdog ends the game after 30 s without a frame
/// (docs/debugging.md#hang-watchdog); a frame meeting hundreds of new pipelines that each took 0.25 to 0.5 s ended the
/// story load. With the bucket empty a first use keeps its stand-in.
/// </summary>
internal static class FirstUseBudget
{
    private const double RefillPerSecond = 0.25;
    private const double CapacitySeconds = 1;

    private static readonly long Capacity = (long)(CapacitySeconds * Stopwatch.Frequency);
    private static readonly object Gate = new();
    private static long _available = Capacity;
    private static long _filled = Stopwatch.GetTimestamp();

    /// <summary>Whether time is left now.</summary>
    public static bool Available()
    {
        lock (Gate)
        {
            Refill();

            return _available > 0;
        }
    }

    /// <summary>Takes <paramref name="ticks"/> (Stopwatch units) spent from the bucket.</summary>
    public static void Spend(long ticks)
    {
        lock (Gate)
        {
            Refill();
            _available -= ticks;
        }
    }

    private static void Refill()
    {
        long now = Stopwatch.GetTimestamp();
        _available = Math.Min(Capacity, _available + (long)((now - _filled) * RefillPerSecond));
        _filled = now;
    }
}
