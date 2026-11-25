// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;
using FiveStar.Loader.Diagnostics;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>The hang watch's stall line: only the waits that grew during the stall, not a boot's totals.</summary>
public sealed class BlockingWaitsTests
{
    [Fact]
    public void A_stall_names_only_the_wait_time_spent_since_the_heartbeat_moved()
    {
        long second = Stopwatch.Frequency;
        var atBeat = new Dictionary<string, long> { ["pipeline creation budget"] = 4 * second, ["small compute pipeline at first use"] = 5 * second };
        var now = new Dictionary<string, long> { ["pipeline creation budget"] = 4 * second, ["small compute pipeline at first use"] = 6 * second, ["driver compile memory budget"] = 2 * second };

        string stall = BlockingWaits.Summary("since", now, atBeat);
        string quiet = BlockingWaits.Summary("since", atBeat, atBeat);

        Assert.Equal("since: driver compile memory budget 2.0 s, small compute pipeline at first use 1.0 s", stall);
        Assert.Equal("since: none", quiet);
    }
}
