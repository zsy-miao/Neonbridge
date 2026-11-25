// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Shaders.Compilation;

/// <summary>
/// The hardware wave sizes of a GPU a translation is made for (D3D12_OPTIONS1 WaveLaneCountMin and Max): with them a
/// run compiles only the forms of a program that GPU picks (<see cref="CorpusOptions.Waves"/>).
/// </summary>
public sealed record WaveRange(int Min, int Max)
{
    private const int MinAndMax = 2;

    /// <summary>Whether a compute shader pinned to <paramref name="lanes"/> loads on the GPU.</summary>
    public bool Offers(int lanes) => lanes >= Min && lanes <= Max;

    /// <summary>A list such as <c>32-32,32-64</c>.</summary>
    public static IReadOnlyList<WaveRange> ParseList(string list)
    {
        return list.Split(',', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries).Select(Parse).ToList();
    }

    public override string ToString() => string.Create(CultureInfo.InvariantCulture, $"{Min}-{Max}");

    private static WaveRange Parse(string text)
    {
        var parts = text.Split('-');

        if (parts.Length != MinAndMax || !int.TryParse(parts[0], CultureInfo.InvariantCulture, out var min) || !int.TryParse(parts[1], CultureInfo.InvariantCulture, out var max) || min < 1 || max < min)
        {
            throw new ArgumentException($"a wave range is min-max lanes, such as 32-64: {text}");
        }

        return new WaveRange(min, max);
    }
}
