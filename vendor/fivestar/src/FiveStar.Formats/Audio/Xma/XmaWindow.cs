// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Audio.Xma;

/// <summary>
/// The sine window overlap between two blocks. The overlap is as long as the shorter block and centred on the
/// boundary; both blocks are stored as half inverse transforms, so the folded-out halves are rebuilt from the stored
/// ones while windowing.
/// </summary>
internal static class XmaWindow
{
    private const double HalfSample = 0.5;
    private const int Halves = 2;

    /// <summary>Rising sine slopes of 128, 256 and 512 samples (by block index, largest first): sin((k + 1/2) pi / 2W).</summary>
    private static readonly float[][] Slopes = BuildSlopes();

    /// <summary>Overlaps the block that starts at <paramref name="boundary"/> with the samples before it.</summary>
    [HotPath]
    public static void Overlap(Span<float> samples, int boundary, int previousLength, int length)
    {
        var width = previousLength < length ? previousLength : length;
        var slope = Slopes[XmaBandLayout.BlockOf(width)];
        var half = width / Halves;
        var start = boundary - half;

        for (var index = 0; index < half; index++)
        {
            var mirror = width - 1 - index;
            var before = samples[start + index];
            var after = samples[start + mirror];

            samples[start + index] = (before * slope[mirror]) - (after * slope[index]);
            samples[start + mirror] = (before * slope[index]) + (after * slope[mirror]);
        }
    }

    private static float[][] BuildSlopes()
    {
        var slopes = new float[XmaLayout.BlockSizes][];

        for (var block = 0; block < slopes.Length; block++)
        {
            var width = XmaLayout.FrameSamples >> block;
            var slope = new float[width];

            for (var index = 0; index < width; index++)
            {
                slope[index] = (float)Math.Sin((index + HalfSample) * Math.PI / (Halves * width));
            }

            slopes[block] = slope;
        }

        return slopes;
    }
}
