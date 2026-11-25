// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;
using FiveStar.Loader.Loading;

namespace FiveStar.Loader.Diagnostics;

/// <summary>
/// <c>--sample-threads &lt;s&gt;</c>: every s seconds logs the calls through shims, stubs and GDK slots since the last
/// sample (busiest first), a line of loader counters, and the stack of every thread of the process
/// (<see cref="ThreadStacks"/>).
/// </summary>
internal sealed class ThreadSampler
{
    private const int MaxFrames = 24;
    private const int TopCalls = 96;
    private const string Prefix = "  [sample] ";

    private readonly ThreadStacks _stacks;
    private readonly CallTrace _trace;
    private readonly Func<string> _counters;
    private readonly LoaderLog _log;
    private readonly ThreadProfiler? _profiler;
    private readonly Stopwatch _clock = Stopwatch.StartNew();
    private Dictionary<string, ulong> _lastTotals = new(StringComparer.Ordinal);

    /// <summary>
    /// <paramref name="counters"/> gives one line of other counters (DirectStorage) logged with each sample;
    /// <paramref name="profile"/> adds a <see cref="ThreadProfiler"/> burst to each sample.
    /// </summary>
    public ThreadSampler(ImageTable images, CallTrace trace, Func<string> counters, bool profile, LoaderLog log)
    {
        _stacks = new ThreadStacks(images, MaxFrames);
        _trace = trace;
        _counters = counters;
        _log = log;
        _profiler = profile ? new ThreadProfiler(images, log) : null;
    }

    public void Start(TimeSpan interval)
    {
        var thread = new Thread(() => Run(interval)) { IsBackground = true, Name = "thread sampler" };
        thread.Start();
        _log.Info($"{Prefix}every {interval.TotalSeconds} s");
    }

    private void Run(TimeSpan interval)
    {
        for (int number = 1; ; number++)
        {
            Thread.Sleep(interval);
            _log.Info($"{Prefix}#{number} at {_clock.Elapsed.TotalSeconds:F1} s: {_counters()}");
            LogCalls();
            _stacks.Log(_log, Prefix);
            _profiler?.Run(_stacks.Modules());
        }
    }

    private void LogCalls()
    {
        var totals = _trace.Totals();
        var busiest = totals
            .Select(pair => (pair.Key, Delta: pair.Value - _lastTotals.GetValueOrDefault(pair.Key)))
            .Where(pair => pair.Delta != 0)
            .OrderByDescending(pair => pair.Delta)
            .Take(TopCalls)
            .Select(pair => $"{pair.Key} x{pair.Delta}")
            .ToList();
        _lastTotals = totals;
        _log.Info($"{Prefix}calls since the last sample: {string.Join(", ", busiest)}");
    }
}
