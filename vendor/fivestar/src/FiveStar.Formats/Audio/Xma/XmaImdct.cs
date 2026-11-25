// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Audio.Xma;

/// <summary>
/// The inverse MDCT of one block size, as its non-redundant half: n coefficients give the middle n of the 2n windowed
/// samples, which is the DCT-IV of the coefficients in reverse order. The DCT-IV runs as a complex FFT of n/2 points
/// between a pre- and a post-rotation.
/// </summary>
internal sealed class XmaImdct
{
    private const int ComplexParts = 2;
    private const double QuarterBin = 0.25;

    private readonly int _size;
    private readonly int _half;
    private readonly float[] _preCos;
    private readonly float[] _preSin;
    private readonly float[] _postCos;
    private readonly float[] _postSin;
    private readonly float[] _rootCos;
    private readonly float[] _rootSin;
    private readonly int[] _bitReverse;
    private readonly float[] _real;
    private readonly float[] _imaginary;

    /// <param name="size">The coefficient count, a power of two.</param>
    public XmaImdct(int size)
    {
        _size = size;
        _half = size / ComplexParts;
        _preCos = new float[_half];
        _preSin = new float[_half];
        _postCos = new float[_half];
        _postSin = new float[_half];

        for (var index = 0; index < _half; index++)
        {
            var pre = Math.PI * index / size;
            var post = Math.PI * (index + QuarterBin) / size;
            _preCos[index] = (float)Math.Cos(pre);
            _preSin[index] = (float)Math.Sin(pre);
            _postCos[index] = (float)Math.Cos(post);
            _postSin[index] = (float)Math.Sin(post);
        }

        _rootCos = new float[_half];
        _rootSin = new float[_half];
        _bitReverse = BitReverse(_half);
        _real = new float[_half];
        _imaginary = new float[_half];
        FillRoots();
    }

    /// <summary>Transforms <paramref name="coefficients"/> into <paramref name="samples"/>, both of the block size.</summary>
    [HotPath]
    public void Inverse(ReadOnlySpan<float> coefficients, Span<float> samples, float scale)
    {
        for (var index = 0; index < _half; index++)
        {
            var even = coefficients[ComplexParts * index];
            var odd = coefficients[_size - 1 - (ComplexParts * index)];
            var target = _bitReverse[index];
            _real[target] = (even * _preCos[index]) + (odd * _preSin[index]);
            _imaginary[target] = (even * _preSin[index]) - (odd * _preCos[index]);
        }

        Butterflies();

        for (var index = 0; index < _half; index++)
        {
            var real = (_real[index] * _postCos[index]) - (_imaginary[index] * _postSin[index]);
            var imaginary = (_real[index] * _postSin[index]) + (_imaginary[index] * _postCos[index]);
            samples[_size - 1 - (ComplexParts * index)] = real * scale;
            samples[ComplexParts * index] = imaginary * scale;
        }
    }

    private static int[] BitReverse(int count)
    {
        var bits = System.Numerics.BitOperations.Log2((uint)count);
        var table = new int[count];

        for (var index = 0; index < count; index++)
        {
            var reversed = 0;

            for (var bit = 0; bit < bits; bit++)
            {
                reversed |= ((index >> bit) & 1) << (bits - 1 - bit);
            }

            table[index] = reversed;
        }

        return table;
    }

    /// <summary>The FFT's roots of unity, exp(+2 pi i k / (n/2)).</summary>
    private void FillRoots()
    {
        for (var index = 0; index < _half; index++)
        {
            var angle = ComplexParts * Math.PI * index / _half;
            _rootCos[index] = (float)Math.Cos(angle);
            _rootSin[index] = (float)Math.Sin(angle);
        }
    }

    /// <summary>An in-place radix-2 FFT with a positive exponent over bit-reversed input.</summary>
    [HotPath]
    private void Butterflies()
    {
        for (var span = ComplexParts; span <= _half; span *= ComplexParts)
        {
            var pairDistance = span / ComplexParts;
            var stride = _half / span;

            for (var start = 0; start < _half; start += span)
            {
                for (var offset = 0; offset < pairDistance; offset++)
                {
                    var top = start + offset;
                    var bottom = top + pairDistance;
                    var cos = _rootCos[offset * stride];
                    var sin = _rootSin[offset * stride];
                    var real = (_real[bottom] * cos) - (_imaginary[bottom] * sin);
                    var imaginary = (_real[bottom] * sin) + (_imaginary[bottom] * cos);

                    _real[bottom] = _real[top] - real;
                    _imaginary[bottom] = _imaginary[top] - imaginary;
                    _real[top] += real;
                    _imaginary[top] += imaginary;
                }
            }
        }
    }
}
