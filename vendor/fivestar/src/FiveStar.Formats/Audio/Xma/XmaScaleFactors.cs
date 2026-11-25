// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Audio.Xma;

/// <summary>
/// Reads a subframe's scale factors (one per band, in steps of 1 to 4 dB). The first subframe of a frame sends them
/// DPCM coded; later subframes either reuse the last ones, resampled to their band layout, or send run-level coded
/// differences to those.
/// </summary>
internal static class XmaScaleFactors
{
    private const int StepBits = 2;

    /// <summary>The DPCM start value is 45 dB in units of the step.</summary>
    private const int StartDecibels = 45;

    /// <summary>The escape of a run-level difference: 14 bits holding level (8), run (5) and sign (1).</summary>
    private const int EscapeBits = 14;
    private const int EscapeLevelShift = 6;
    private const int EscapeRunMask = 0x3F;

    /// <summary>Reads the scale factors of one channel; false when the coding is broken.</summary>
    [HotPath]
    public static bool Read(ref XmaBitReader reader, XmaChannel channel, XmaBandLayout layout, int block)
    {
        var bands = layout.BandCount(block);
        var current = channel.ScaleFactorSets[1 - channel.TransmittedSet];
        channel.ScaleFactors = current;

        if (channel.HasScaleFactors)
        {
            var carry = layout.Carry(block, channel.TransmittedBlock);
            var transmitted = channel.ScaleFactorSets[channel.TransmittedSet];

            for (var band = 0; band < bands; band++)
            {
                current[band] = transmitted[carry[band]];
            }
        }

        if (channel.CurrentSubframe == 0 || reader.ReadBit())
        {
            if (!ReadNew(ref reader, channel, current, bands))
            {
                return false;
            }

            channel.TransmittedSet = 1 - channel.TransmittedSet;
            channel.TransmittedBlock = block;
            channel.HasScaleFactors = true;
        }

        channel.MaxScaleFactor = Max(current.AsSpan(0, bands));

        return true;
    }

    [HotPath]
    private static bool ReadNew(ref XmaBitReader reader, XmaChannel channel, int[] current, int bands)
    {
        if (channel.HasScaleFactors)
        {
            return ReadDifferences(ref reader, current, bands);
        }

        channel.ScaleFactorStep = (int)reader.Read(StepBits) + 1;
        var value = StartDecibels / channel.ScaleFactorStep;

        for (var band = 0; band < bands; band++)
        {
            var symbol = WmaProCodebooks.Scale.Decode(ref reader);

            if (symbol < 0)
            {
                return false;
            }

            value += symbol - WmaProCodebooks.ScaleBias;
            current[band] = value;
        }

        return true;
    }

    /// <summary>Run-level differences to the carried-over values, until the end code.</summary>
    [HotPath]
    private static bool ReadDifferences(ref XmaBitReader reader, int[] current, int bands)
    {
        for (var band = 0; band < bands; band++)
        {
            var symbol = WmaProCodebooks.ScaleRunLevel.Decode(ref reader);
            int skip;
            int level;
            bool positive;

            if (symbol == WmaProCodebooks.RunLevelEnd)
            {
                break;
            }

            if (symbol == WmaProCodebooks.RunLevelEscape)
            {
                var code = (int)reader.Read(EscapeBits);
                level = code >> EscapeLevelShift;
                skip = (code & EscapeRunMask) >> 1;
                positive = (code & 1) != 0;
            }
            else if (symbol >= 0)
            {
                skip = WmaProTables.ScaleRunLevelRuns[symbol];
                level = WmaProTables.ScaleRunLevelLevels[symbol];
                positive = reader.ReadBit();
            }
            else
            {
                return false;
            }

            band += skip;

            if (band >= bands)
            {
                return false;
            }

            if (positive)
            {
                current[band] += level;
            }
            else
            {
                current[band] -= level;
            }
        }

        return true;
    }

    [HotPath]
    private static int Max(ReadOnlySpan<int> values)
    {
        var max = values[0];

        foreach (var value in values)
        {
            if (value > max)
            {
                max = value;
            }
        }

        return max;
    }
}
