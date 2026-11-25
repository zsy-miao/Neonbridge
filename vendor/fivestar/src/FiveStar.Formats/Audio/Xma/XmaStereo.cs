// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Audio.Xma;

/// <summary>
/// The channel transform of a stereo subframe. When both channels share the subframe they may be coded as sum and
/// difference, per band: the decoder turns (s, d) back into (s - d, s + d) in the bands that use it and scales the
/// other bands by 181/128 (the square root of two, as the format rounds it).
/// </summary>
internal static class XmaStereo
{
    /// <summary>181/128, the format's square root of two.</summary>
    private const float UncoupledScale = 181f / 128f;

    /// <summary>
    /// Reads the transform of a subframe of a <paramref name="channels"/>-channel stream that <paramref name="sharing"/>
    /// channels take part in: <paramref name="coupled"/> tells whether it uses the transform, <paramref name="bandsOn"/>
    /// receives per band whether it applies. False when the subframe uses a form XMA does not define.
    /// </summary>
    [HotPath]
    public static bool Read(ref XmaBitReader reader, int channels, int sharing, out bool coupled, Span<bool> bandsOn)
    {
        coupled = false;

        if (channels < XmaLayout.MaxChannels)
        {
            return true;
        }

        if (reader.ReadBit())
        {
            return false;
        }

        if (sharing < XmaLayout.MaxChannels)
        {
            return true;
        }

        if (reader.ReadBit())
        {
            return !reader.ReadBit();
        }

        coupled = true;

        if (reader.ReadBit())
        {
            bandsOn.Fill(true);

            return true;
        }

        for (var band = 0; band < bandsOn.Length; band++)
        {
            bandsOn[band] = reader.ReadBit();
        }

        return true;
    }

    /// <summary>Undoes the transform in place on the two channels' coefficients.</summary>
    [HotPath]
    public static void Apply(Span<float> left, Span<float> right, ReadOnlySpan<int> edges, ReadOnlySpan<bool> bandsOn)
    {
        for (var band = 0; band < bandsOn.Length; band++)
        {
            var end = edges[band + 1] < left.Length ? edges[band + 1] : left.Length;

            for (var index = edges[band]; index < end; index++)
            {
                if (bandsOn[band])
                {
                    var sum = left[index];
                    var difference = right[index];
                    left[index] = sum - difference;
                    right[index] = sum + difference;
                }
                else
                {
                    left[index] *= UncoupledScale;
                    right[index] *= UncoupledScale;
                }
            }
        }
    }
}
