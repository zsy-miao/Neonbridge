// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>The counts of a warm-up run by <see cref="WarmOutcome"/>, and the driver time of the pipelines it built.</summary>
internal sealed class WarmTally
{
    /// <summary>Pipelines between two drops of the loaded DXIL, so a full run does not hold every program.</summary>
    public const int ForgetInterval = 500;

    private readonly int[] _counts = new int[Enum.GetValues<WarmOutcome>().Length];
    private long _buildTicks;
    private int _done;

    /// <summary>Pipelines handled so far.</summary>
    public int Done => Volatile.Read(ref _done);

    public int this[WarmOutcome outcome] => Volatile.Read(ref _counts[(int)outcome]);

    /// <summary>The summed driver time of the built pipelines (more than the wall time with several jobs).</summary>
    public TimeSpan BuildTime => TimeSpan.FromTicks(Interlocked.Read(ref _buildTicks));

    public string Summary =>
        $"{this[WarmOutcome.Built]} built ({BuildTime.TotalMinutes:F1} min of driver time), {this[WarmOutcome.Skipped]} skipped, " +
        $"{this[WarmOutcome.Untranslated]} without translation, {this[WarmOutcome.Refused]} refused, {this[WarmOutcome.Deferred]} left out for memory";

    public void Count(WarmOutcome outcome)
    {
        Interlocked.Increment(ref _counts[(int)outcome]);
        Interlocked.Increment(ref _done);
    }

    public void AddBuildTime(TimeSpan time)
    {
        Interlocked.Add(ref _buildTicks, time.Ticks);
    }
}
