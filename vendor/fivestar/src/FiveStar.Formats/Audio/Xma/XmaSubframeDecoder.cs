// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Audio.Xma;

/// <summary>
/// Decodes the next subframe of a frame: the block of the channels that are furthest behind. It reads the subframe
/// header, quantisation, scale factors and coefficients, undoes the channel transform, dequantises per band, runs the
/// inverse MDCT and overlaps the block with what precedes it.
/// </summary>
internal sealed class XmaSubframeDecoder
{
    private const int FillLengthBits = 2;
    private const int FillLengthSizeBits = 4;
    private const int QuantStepBits = 6;
    private const int QuantEscapeBits = 5;

    /// <summary>The 6-bit step values that continue in 5-bit pieces (the largest piece, 31, continues again).</summary>
    private const int QuantStepLow = -32;
    private const int QuantStepHigh = 31;
    private const int QuantModifierBits = 3;
    private const int VectorWidth = 4;

    /// <summary>The inverse transform of n coefficients is scaled by 2 / n, and the output by 2^-15 to full scale 1.</summary>
    private const int Halves = 2;
    private const double DecibelsPerAmplitudeDecade = 20;
    private const double Ten = 10;

    private readonly XmaChannel[] _channels;
    private readonly XmaBandLayout _bands;
    private readonly XmaImdct[] _transforms = new XmaImdct[XmaLayout.BlockSizes];
    private readonly int[] _sharing = new int[XmaLayout.MaxChannels];
    private readonly bool[] _bandsOn = new bool[XmaLayout.MaxBands];
    private readonly float[] _dequantised = new float[XmaLayout.FrameSamples];

    public XmaSubframeDecoder(XmaChannel[] channels, XmaBandLayout bands)
    {
        _channels = channels;
        _bands = bands;

        for (var block = 0; block < XmaLayout.BlockSizes; block++)
        {
            _transforms[block] = new XmaImdct(XmaLayout.FrameSamples >> block);
        }
    }

    /// <summary>Decodes one subframe; <paramref name="frameDone"/> tells whether it was the frame's last.</summary>
    [HotPath]
    public bool Decode(ref XmaBitReader reader, int channelCount, out bool frameDone)
    {
        var length = NextBlock(channelCount, out var sharing, out frameDone);

        if (length == 0 || !ReadHeader(ref reader))
        {
            return false;
        }

        var block = XmaBandLayout.BlockOf(length);
        var bandsOn = _bandsOn.AsSpan(0, _bands.BandCount(block));

        if (!XmaStereo.Read(ref reader, channelCount, sharing, out var coupled, bandsOn))
        {
            return false;
        }

        var coded = false;

        for (var index = 0; index < sharing; index++)
        {
            var channel = _channels[_sharing[index]];
            channel.CodesCoefficients = reader.ReadBit();
            coded |= channel.CodesCoefficients;
        }

        var vectorCountSent = false;

        if (coded && !ReadQuantisation(ref reader, sharing, length, block, out vectorCountSent))
        {
            return false;
        }

        ReadAllCoefficients(ref reader, sharing, length, coded && vectorCountSent);

        if (coded)
        {
            Reconstruct(sharing, length, block, coupled, bandsOn);
        }

        FinishBlock(sharing, length);

        return true;
    }

    /// <summary>
    /// Finds the block of the channels with the fewest decoded samples whose next subframe matches; returns its length,
    /// or 0 when the tiling is used up.
    /// </summary>
    [HotPath]
    private int NextBlock(int channelCount, out int sharing, out bool frameDone)
    {
        var offset = XmaLayout.FrameSamples;
        var length = 0;
        var remaining = XmaLayout.FrameSamples * channelCount;
        sharing = 0;

        for (var index = 0; index < channelCount; index++)
        {
            var channel = _channels[index];

            if (channel.DecodedSamples < offset && channel.CurrentSubframe < channel.SubframeCount)
            {
                offset = channel.DecodedSamples;
                length = channel.SubframeLengths[channel.CurrentSubframe];
            }
        }

        for (var index = 0; index < channelCount && length != 0; index++)
        {
            var channel = _channels[index];
            remaining -= channel.DecodedSamples;

            if (channel.DecodedSamples == offset && channel.CurrentSubframe < channel.SubframeCount
                && channel.SubframeLengths[channel.CurrentSubframe] == length)
            {
                remaining -= length;
                channel.DecodedSamples += length;
                channel.BlockOffset = offset + XmaChannel.FrameStart;
                _sharing[sharing++] = index;
            }
        }

        frameDone = remaining == 0;

        return length;
    }

    /// <summary>The optional fill bits and the reserved bit, which XMA leaves 0.</summary>
    [HotPath]
    private static bool ReadHeader(ref XmaBitReader reader)
    {
        if (reader.ReadBit())
        {
            var fill = (int)reader.Read(FillLengthBits);

            if (fill == 0)
            {
                var size = (int)reader.Read(FillLengthSizeBits);
                fill = (int)reader.Read(size) + 1;
            }

            if (fill > reader.Remaining)
            {
                return false;
            }

            reader.Advance(fill);
        }

        return !reader.ReadBit();
    }

    /// <summary>The vector-coded counts, the quantisation step with per-channel modifiers, and the scale factors.</summary>
    [HotPath]
    private bool ReadQuantisation(ref XmaBitReader reader, int sharing, int length, int block, out bool vectorCountSent)
    {
        vectorCountSent = reader.ReadBit();
        var countBits = System.Numerics.BitOperations.Log2((uint)((length + VectorWidth - 1) / VectorWidth)) + 1;

        for (var index = 0; index < sharing; index++)
        {
            var channel = _channels[_sharing[index]];
            channel.VectorCoefficients = length;

            if (vectorCountSent)
            {
                channel.VectorCoefficients = (int)reader.Read(countBits) * VectorWidth;
            }

            if (channel.VectorCoefficients > length)
            {
                return false;
            }
        }

        ReadQuantSteps(ref reader, sharing, XmaLayout.BaseQuantStep + ReadQuantStep(ref reader));

        for (var index = 0; index < sharing; index++)
        {
            if (!XmaScaleFactors.Read(ref reader, _channels[_sharing[index]], _bands, block))
            {
                return false;
            }
        }

        return true;
    }

    /// <summary>The 6-bit signed step; its two extremes continue in 5-bit pieces away from zero.</summary>
    [HotPath]
    private static int ReadQuantStep(ref XmaBitReader reader)
    {
        var step = reader.ReadSigned(QuantStepBits);

        if (step != QuantStepLow && step != QuantStepHigh)
        {
            return step;
        }

        var extra = 0;
        var piece = step;

        while (reader.Remaining > QuantEscapeBits)
        {
            piece = (int)reader.Read(QuantEscapeBits);

            if (piece != QuantStepHigh)
            {
                break;
            }

            extra += QuantStepHigh;
        }

        if (step == QuantStepHigh)
        {
            return step + extra + piece;
        }

        return step - (extra + piece);
    }

    /// <summary>One channel takes the step as is; with two, a 3-bit size and per channel a flag and a modifier follow.</summary>
    [HotPath]
    private void ReadQuantSteps(ref XmaBitReader reader, int sharing, int step)
    {
        if (sharing == 1)
        {
            _channels[_sharing[0]].QuantStep = step;

            return;
        }

        var modifierBits = (int)reader.Read(QuantModifierBits);

        for (var index = 0; index < sharing; index++)
        {
            var channel = _channels[_sharing[index]];
            channel.QuantStep = step;

            if (reader.ReadBit())
            {
                channel.QuantStep += (int)reader.Read(modifierBits) + 1;
            }
        }
    }

    [HotPath]
    private void ReadAllCoefficients(ref XmaBitReader reader, int sharing, int length, bool vectorCountSent)
    {
        for (var index = 0; index < sharing; index++)
        {
            var channel = _channels[_sharing[index]];
            var coefficients = channel.Samples.AsSpan(channel.BlockOffset, length);

            if (channel.CodesCoefficients && reader.Remaining > 0)
            {
                XmaCoefficients.Read(ref reader, coefficients, channel.VectorCoefficients, vectorCountSent);
            }
            else
            {
                coefficients.Clear();
            }
        }
    }

    /// <summary>Channel transform, dequantisation per band and the inverse MDCT, in place in each channel's samples.</summary>
    [HotPath]
    private void Reconstruct(int sharing, int length, int block, bool coupled, ReadOnlySpan<bool> bandsOn)
    {
        var edges = _bands.Edges(block);

        if (coupled)
        {
            var left = _channels[_sharing[0]];
            var right = _channels[_sharing[1]];
            XmaStereo.Apply(left.Samples.AsSpan(left.BlockOffset, length), right.Samples.AsSpan(right.BlockOffset, length), edges, bandsOn);
        }

        var scale = 1f / (length / Halves) / (1 << (XmaLayout.BitsPerSample - 1));

        for (var index = 0; index < sharing; index++)
        {
            var channel = _channels[_sharing[index]];
            var samples = channel.Samples.AsSpan(channel.BlockOffset, length);
            Dequantise(channel, samples, edges, bandsOn.Length);
            _transforms[block].Inverse(_dequantised.AsSpan(0, length), samples, scale);
        }
    }

    [HotPath]
    private void Dequantise(XmaChannel channel, ReadOnlySpan<float> coefficients, ReadOnlySpan<int> edges, int bands)
    {
        for (var band = 0; band < bands; band++)
        {
            var exponent = channel.QuantStep - ((channel.MaxScaleFactor - channel.ScaleFactors[band]) * channel.ScaleFactorStep);
            var factor = (float)Math.Pow(Ten, exponent / DecibelsPerAmplitudeDecade);
            var end = edges[band + 1] < coefficients.Length ? edges[band + 1] : coefficients.Length;

            for (var index = edges[band]; index < end; index++)
            {
                _dequantised[index] = coefficients[index] * factor;
            }
        }
    }

    /// <summary>Overlaps each channel's new block with the samples before it and moves on to its next subframe.</summary>
    [HotPath]
    private void FinishBlock(int sharing, int length)
    {
        for (var index = 0; index < sharing; index++)
        {
            var channel = _channels[_sharing[index]];
            XmaWindow.Overlap(channel.Samples, channel.BlockOffset, channel.PreviousBlockLength, length);
            channel.PreviousBlockLength = length;
            channel.CurrentSubframe++;
        }
    }
}
