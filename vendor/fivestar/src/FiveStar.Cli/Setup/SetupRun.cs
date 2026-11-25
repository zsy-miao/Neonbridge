// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Setup;

/// <summary>
/// One run of the setup steps: which steps are selected, which are finished (up to date or run now), the expected time
/// of the steps still to run, and when the run started.
/// </summary>
internal sealed class SetupRun
{
    private readonly HashSet<string> _finished = new(StringComparer.Ordinal);
    private readonly IReadOnlySet<string>? _only;

    public SetupRun(IReadOnlySet<string>? only, bool force, long started, TimeSpan expected)
    {
        _only = only;
        Force = force;
        Started = started;
        Remaining = expected;
    }

    public bool Force { get; }

    /// <summary>The timestamp of the start, from the run's time provider.</summary>
    public long Started { get; }

    /// <summary>The expected time of the steps still to run.</summary>
    public TimeSpan Remaining { get; set; }

    /// <summary>Whether <c>--only</c> selects the step; every step without it.</summary>
    public bool Selects(string step)
    {
        return _only is null || _only.Contains(step);
    }

    public void Finish(string step)
    {
        _finished.Add(step);
    }

    public bool Finished(string step)
    {
        return _finished.Contains(step);
    }
}
