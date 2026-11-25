// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Diagnostics;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>The audio capture's measure of a recording: seconds with sound, the longest silence and the timeline.</summary>
public sealed class AudioActivityTests
{
    [Fact]
    public void Tone_silence_and_tone_count_as_sound_and_one_silence()
    {
        var activity = new AudioActivity();
        var tone = new short[AudioActivity.Rate * 2];

        for (int frame = 0; frame < AudioActivity.Rate; frame++)
        {
            short value = (short)(8000 * Math.Sin(2 * Math.PI * 440 * frame / AudioActivity.Rate));
            (tone[2 * frame], tone[(2 * frame) + 1]) = (value, value);
        }

        activity.Add(tone);
        activity.AddSilence(AudioActivity.Rate * 8);
        activity.Add(tone);
        activity.Add(new short[AudioActivity.Rate * 2]);

        Assert.Equal(11.0, activity.Seconds, 3);
        Assert.Equal(2.0, activity.AudibleSeconds, 3);
        Assert.Equal(8.0, activity.LongestSilenceSeconds, 3);
        Assert.EndsWith("sound per 10 s: 20%", activity.Summary(), StringComparison.Ordinal);
    }
}
