// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;
using System.Globalization;
using FiveStar.Shaders.Compilation;

namespace FiveStar.Cli.Shader;

/// <summary>
/// Prints a corpus run's progress every 10 seconds and at the end: programs done, elapsed time, the time left at the
/// rate of the last 5 minutes (cached programs finish much faster than new ones), the estimated finish and a detail
/// from the caller (cache counts). A run that weighs its items (<see cref="CorpusProgress.WorkTotal"/>) is measured in
/// that work, so a few programs that take minutes each count for what they cost, not for their number.
/// </summary>
internal sealed class ConsoleProgress : IProgress<CorpusProgress>
{
    private const int PercentScale = 100;

    private static readonly TimeSpan Interval = TimeSpan.FromSeconds(10);
    private static readonly TimeSpan RateWindow = TimeSpan.FromMinutes(5);

    private readonly Func<string> _detail;
    private readonly Stopwatch _clock = Stopwatch.StartNew();
    private readonly Lock _gate = new();
    private readonly Queue<(TimeSpan Time, double Done)> _samples = new([(TimeSpan.Zero, 0)]);
    private TimeSpan _printed;

    public ConsoleProgress(Func<string> detail)
    {
        _detail = detail;
    }

    public void Report(CorpusProgress value)
    {
        var elapsed = _clock.Elapsed;
        (TimeSpan Time, double Done) oldest;
        var (done, total) = Measure(value);

        lock (_gate)
        {
            if (elapsed - _printed < Interval && value.Done != value.Total)
            {
                return;
            }

            _printed = elapsed;
            _samples.Enqueue((elapsed, done));

            while (_samples.Count > 1 && elapsed - _samples.Peek().Time > RateWindow)
            {
                _samples.Dequeue();
            }

            oldest = _samples.Peek();
        }

        var rate = Math.Max(total / value.Total, done - oldest.Done) / Math.Max(1, (elapsed - oldest.Time).TotalSeconds);
        var left = TimeSpan.FromSeconds((total - done) / rate);
        var percent = done * PercentScale / total;
        var line = string.Create(
            CultureInfo.InvariantCulture,
            $"{value.Done}/{value.Total} ({percent:F1}%)  elapsed {Format(elapsed)}  left ~{Format(left)}  finish ~{DateTime.Now + left:HH:mm}  {_detail()}");
        Console.WriteLine(line);
    }

    /// <summary>The work done and in all: the weights when the run gives them, else the program counts.</summary>
    private static (double Done, double Total) Measure(CorpusProgress value)
    {
        if (value.WorkTotal > 0)
        {
            return (value.WorkDone, value.WorkTotal);
        }

        return (value.Done, Math.Max(1, value.Total));
    }

    private static string Format(TimeSpan time)
    {
        return time.ToString(@"h\:mm\:ss", CultureInfo.InvariantCulture);
    }
}
