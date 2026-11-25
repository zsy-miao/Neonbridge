// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.CompilerServices;

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>
/// The decoder's stall test: a decode fails when the stream position, the number of symbols handed over and the sum
/// of the lanes' control states all stay the same for more than <see cref="LaneClock.StallLimit"/> clocks.
/// </summary>
internal struct ProgressWatch
{
    private int _position;
    private long _symbols;
    private int _controlSum;
    private long _lastChange;

    public static ProgressWatch Start()
    {
        var watch = default(ProgressWatch);
        watch._position = -1;
        watch._symbols = -1;
        watch._controlSum = -1;

        return watch;
    }

    /// <summary>Whether nothing changed for too long, given the counters after clock <paramref name="cycle"/>.</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    public bool Stalled(int position, long symbols, int controlSum, long cycle)
    {
        if (position != _position || symbols != _symbols || controlSum != _controlSum)
        {
            _position = position;
            _symbols = symbols;
            _controlSum = controlSum;
            _lastChange = cycle;

            return false;
        }

        return cycle - _lastChange > LaneClock.StallLimit;
    }
}
