// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Audio.Xma;

/// <summary>
/// The scale factor bands of each block size at one sample rate, and which band of another block size covers the
/// middle of each band (scale factors carry over between subframes of different sizes).
/// </summary>
internal sealed class XmaBandLayout
{
    /// <summary>A block of n coefficients spans half the sample rate, so frequency f sits at n * 2 * f / rate.</summary>
    private const int NyquistFactor = 2;
    private const int EdgeRounding = 2;
    private const int EdgeAlignment = ~3;
    private const int Rate48000 = 48_000;
    private const int Rate44100 = 44_100;
    private const int Rate32000 = 32_000;
    private const int Rate24000 = 24_000;

    private readonly int[][] _edges = new int[XmaLayout.BlockSizes][];
    private readonly int[] _counts = new int[XmaLayout.BlockSizes];
    private readonly int[][][] _carry = new int[XmaLayout.BlockSizes][][];

    public XmaBandLayout(int sampleRate)
    {
        var rate = BandRate(sampleRate);

        for (var block = 0; block < XmaLayout.BlockSizes; block++)
        {
            _edges[block] = new int[XmaLayout.MaxBands + 1];
            _counts[block] = Edges(XmaLayout.FrameSamples >> block, rate, _edges[block]);
        }

        for (var block = 0; block < XmaLayout.BlockSizes; block++)
        {
            _carry[block] = new int[XmaLayout.BlockSizes][];

            for (var other = 0; other < XmaLayout.BlockSizes; other++)
            {
                _carry[block][other] = BuildCarry(block, other);
            }
        }
    }

    /// <summary>The block index of a subframe length: 0 for a whole frame, 1 for half, 2 for a quarter.</summary>
    [HotPath]
    public static int BlockOf(int length) => System.Numerics.BitOperations.Log2((uint)(XmaLayout.FrameSamples / length));

    [HotPath]
    public int BandCount(int block) => _counts[block];

    /// <summary>The band edges of a block: band b covers coefficients [edges[b], edges[b + 1]).</summary>
    [HotPath]
    public ReadOnlySpan<int> Edges(int block) => _edges[block];

    /// <summary>For each band of <paramref name="block"/>, the band of <paramref name="from"/> that covers its middle.</summary>
    [HotPath]
    public ReadOnlySpan<int> Carry(int block, int from) => _carry[block][from];

    /// <summary>The rate the band edges are computed for: the nearest of the four XMA rates at or above the stream's.</summary>
    private static int BandRate(int sampleRate)
    {
        if (sampleRate > Rate44100)
        {
            return Rate48000;
        }

        if (sampleRate > Rate32000)
        {
            return Rate44100;
        }

        if (sampleRate > Rate24000)
        {
            return Rate32000;
        }

        return Rate24000;
    }

    /// <summary>Fills the edges of one block size from the critical frequencies; returns the band count.</summary>
    private static int Edges(int size, int rate, int[] edges)
    {
        var bands = 1;
        var frequencies = WmaProTables.CriticalFrequencies;

        for (var index = 0; index < frequencies.Length && edges[bands - 1] < size; index++)
        {
            var edge = (((size * NyquistFactor * frequencies[index]) / rate) + EdgeRounding) & EdgeAlignment;

            if (edge > edges[bands - 1])
            {
                edges[bands++] = edge;
            }

            if (edge >= size)
            {
                break;
            }
        }

        edges[bands - 1] = size;

        return bands - 1;
    }

    private int[] BuildCarry(int block, int from)
    {
        var bands = new int[_counts[block]];
        var edges = _edges[block];
        var fromEdges = _edges[from];

        for (var band = 0; band < bands.Length; band++)
        {
            var middle = ((edges[band] + edges[band + 1] - 1) << block) >> 1;
            var covering = 0;

            while ((fromEdges[covering + 1] << from) < middle)
            {
                covering++;
            }

            bands[band] = covering;
        }

        return bands;
    }
}
