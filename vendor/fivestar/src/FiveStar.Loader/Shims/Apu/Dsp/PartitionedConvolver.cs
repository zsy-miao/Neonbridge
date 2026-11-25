// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Apu.Dsp;

/// <summary>
/// Convolves one channel, block by block, with an impulse response: uniformly partitioned overlap-save. The response is
/// cut into partitions of one block, each transformed once (FFT of two blocks, the second half zero). Every block of
/// input is transformed with the block before it and kept in a delay line of spectra; the output block is the inverse
/// transform of the sum over partitions of partition spectrum times the input spectrum that many blocks old, second
/// half. The output equals the direct convolution of the whole input with the response, block for block, with no
/// added latency. Only the bins up to half the FFT size are kept: the signals are real.
/// </summary>
internal sealed class PartitionedConvolver
{
    private const int TransformBlocks = 2;

    private readonly Fft _fft;
    private readonly int _block;
    private readonly int _bins;
    private readonly int _partitions;
    private readonly float[] _responseReal;
    private readonly float[] _responseImaginary;
    private readonly float[] _inputReal;
    private readonly float[] _inputImaginary;
    private readonly float[] _history;
    private readonly float[] _real;
    private readonly float[] _imaginary;
    private readonly float[] _sumReal;
    private readonly float[] _sumImaginary;
    private int _newest;

    /// <summary>A convolver of <paramref name="block"/>-sample blocks (a power of two) with <paramref name="response"/>.</summary>
    public PartitionedConvolver(ReadOnlySpan<float> response, int block)
    {
        _block = block;
        _fft = new Fft(block * TransformBlocks);
        _bins = block + 1;
        _partitions = Math.Max(1, (response.Length + block - 1) / block);

        _responseReal = new float[_partitions * _bins];
        _responseImaginary = new float[_partitions * _bins];
        _inputReal = new float[_partitions * _bins];
        _inputImaginary = new float[_partitions * _bins];
        _history = new float[_fft.Size];
        _real = new float[_fft.Size];

        _imaginary = new float[_fft.Size];
        _sumReal = new float[_bins];
        _sumImaginary = new float[_bins];
        TransformResponse(response);
    }

    /// <summary>Partitions of the response, one block each.</summary>
    public int Partitions => _partitions;

    /// <summary>Convolves the next block: <paramref name="input"/> and <paramref name="output"/> are one block long and may overlap.</summary>
    [HotPath]
    public void Process(ReadOnlySpan<float> input, Span<float> output)
    {
        _history.AsSpan(_block, _block).CopyTo(_history);
        input[.._block].CopyTo(_history.AsSpan(_block));

        _newest = (_newest + 1) % _partitions;
        _history.CopyTo(_real, 0);
        Array.Clear(_imaginary);
        _fft.Forward(_real, _imaginary);
        _real.AsSpan(0, _bins).CopyTo(_inputReal.AsSpan(_newest * _bins, _bins));
        _imaginary.AsSpan(0, _bins).CopyTo(_inputImaginary.AsSpan(_newest * _bins, _bins));

        Accumulate();
        InverseToOutput(output);
    }

    /// <summary>Sums partition p's spectrum times the input spectrum p blocks old.</summary>
    [HotPath]
    private void Accumulate()
    {
        Array.Clear(_sumReal);
        Array.Clear(_sumImaginary);

        for (int partition = 0; partition < _partitions; partition++)
        {
            int slot = (_newest - partition + _partitions) % _partitions;
            var xr = _inputReal.AsSpan(slot * _bins, _bins);
            var xi = _inputImaginary.AsSpan(slot * _bins, _bins);
            var hr = _responseReal.AsSpan(partition * _bins, _bins);
            var hi = _responseImaginary.AsSpan(partition * _bins, _bins);

            for (int bin = 0; bin < _bins; bin++)
            {
                _sumReal[bin] += (xr[bin] * hr[bin]) - (xi[bin] * hi[bin]);
                _sumImaginary[bin] += (xr[bin] * hi[bin]) + (xi[bin] * hr[bin]);
            }
        }
    }

    /// <summary>Rebuilds the full spectrum from its real-signal half, transforms back and keeps the valid second half.</summary>
    [HotPath]
    private void InverseToOutput(Span<float> output)
    {
        int size = _fft.Size;

        for (int bin = 0; bin < _bins; bin++)
        {
            _real[bin] = _sumReal[bin];
            _imaginary[bin] = _sumImaginary[bin];
        }

        for (int bin = _bins; bin < size; bin++)
        {
            _real[bin] = _sumReal[size - bin];
            _imaginary[bin] = -_sumImaginary[size - bin];
        }

        _fft.Inverse(_real, _imaginary);
        _real.AsSpan(_block, _block).CopyTo(output);
    }

    private void TransformResponse(ReadOnlySpan<float> response)
    {
        for (int partition = 0; partition < _partitions; partition++)
        {
            Array.Clear(_real);
            Array.Clear(_imaginary);
            int start = partition * _block;
            int count = Math.Min(_block, response.Length - start);

            if (count > 0)
            {
                response.Slice(start, count).CopyTo(_real);
            }

            _fft.Forward(_real, _imaginary);
            _real.AsSpan(0, _bins).CopyTo(_responseReal.AsSpan(partition * _bins, _bins));
            _imaginary.AsSpan(0, _bins).CopyTo(_responseImaginary.AsSpan(partition * _bins, _bins));
        }
    }
}
