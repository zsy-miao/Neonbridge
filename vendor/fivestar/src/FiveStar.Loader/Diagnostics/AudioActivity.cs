// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Loader.Diagnostics;

/// <summary>
/// How much of a recording has sound: the 48 kHz stereo 16-bit frames go in 0.1 s windows, and a window has sound when
/// its loudest sample reaches <see cref="Threshold"/> (about -54 dBFS). Keeps the totals, the longest silence, and the
/// share of windows with sound per 10 s.
/// </summary>
internal sealed class AudioActivity
{
    /// <summary>The level a window needs to count as sound.</summary>
    public const int Threshold = 64;

    public const int Rate = 48_000;
    public const int Channels = 2;

    private const int WindowsPerSecond = 10;
    private const int WindowFrames = Rate / WindowsPerSecond;
    private const int TimelineWindows = 100;
    private const int Percent = 100;

    private readonly List<int> _timeline = [];
    private int _framesInWindow;
    private int _peak;
    private int _windows;
    private int _audible;
    private int _silentRun;
    private int _longestSilence;
    private int _timelineAudible;
    private int _timelineWindows;

    public double Seconds => (double)_windows / WindowsPerSecond;

    public double AudibleSeconds => (double)_audible / WindowsPerSecond;

    public double LongestSilenceSeconds => (double)_longestSilence / WindowsPerSecond;

    /// <summary>Adds interleaved stereo samples.</summary>
    public void Add(ReadOnlySpan<short> samples)
    {
        for (int index = 0; index + 1 < samples.Length; index += Channels)
        {
            _peak = Math.Max(_peak, Math.Max(Math.Abs((int)samples[index]), Math.Abs((int)samples[index + 1])));

            if (++_framesInWindow == WindowFrames)
            {
                EndWindow();
            }
        }
    }

    /// <summary>Adds frames of silence.</summary>
    public void AddSilence(long frames)
    {
        for (long frame = 0; frame < frames; frame++)
        {
            if (++_framesInWindow == WindowFrames)
            {
                EndWindow();
            }
        }
    }

    /// <summary>"N s recorded, M s with sound, longest silence S s; sound per 10 s: a% b% ...".</summary>
    public string Summary()
    {
        var shares = _timeline.Select(share => share.ToString(CultureInfo.InvariantCulture) + "%");

        return string.Create(
            CultureInfo.InvariantCulture,
            $"{Seconds:F1} s recorded, {AudibleSeconds:F1} s with sound, longest silence {LongestSilenceSeconds:F1} s; sound per 10 s: {string.Join(' ', shares)}");
    }

    private void EndWindow()
    {
        bool sound = _peak >= Threshold;
        _windows++;
        _timelineWindows++;
        _framesInWindow = 0;
        _peak = 0;

        if (sound)
        {
            _audible++;
            _timelineAudible++;
            _silentRun = 0;
        }
        else
        {
            _silentRun++;
            _longestSilence = Math.Max(_longestSilence, _silentRun);
        }

        if (_timelineWindows == TimelineWindows)
        {
            _timeline.Add(_timelineAudible * Percent / TimelineWindows);
            _timelineAudible = 0;
            _timelineWindows = 0;
        }
    }
}
