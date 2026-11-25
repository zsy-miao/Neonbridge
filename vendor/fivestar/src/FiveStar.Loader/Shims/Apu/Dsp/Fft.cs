// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Numerics;

namespace FiveStar.Loader.Shims.Apu.Dsp;

/// <summary>
/// An in-place radix-2 complex FFT of one power-of-two size on split real and imaginary arrays. The twiddles and the
/// bit-reversal order are computed once; a transform allocates nothing. The forward transform uses e^(-2 pi i k n / N),
/// the inverse e^(+2 pi i k n / N) and scales by 1/N, so Inverse(Forward(x)) = x.
/// </summary>
internal sealed class Fft
{
    private const float Forwards = -1f;
    private const float Backwards = 1f;
    private const int BitsPerByte = 8;

    private readonly int[] _reversed;
    private readonly float[] _cos;
    private readonly float[] _sin;

    public Fft(int size)
    {
        if (size <= 1 || !BitOperations.IsPow2(size))
        {
            throw new ArgumentOutOfRangeException(nameof(size), size, "the FFT size is a power of two above 1");
        }

        Size = size;
        int half = size >> 1;
        _cos = new float[half];
        _sin = new float[half];

        for (int index = 0; index < half; index++)
        {
            double angle = Math.Tau * index / size;
            _cos[index] = (float)Math.Cos(angle);
            _sin[index] = (float)Math.Sin(angle);
        }

        _reversed = new int[size];
        int bits = BitOperations.Log2((uint)size);

        for (int index = 0; index < size; index++)
        {
            _reversed[index] = (int)(ReverseBits((uint)index) >> (sizeof(uint) * BitsPerByte - bits));
        }
    }

    public int Size { get; }

    [HotPath]
    public void Forward(Span<float> real, Span<float> imaginary)
    {
        Transform(real, imaginary, Forwards);
    }

    [HotPath]
    public void Inverse(Span<float> real, Span<float> imaginary)
    {
        Transform(real, imaginary, Backwards);
        float scale = 1f / Size;

        for (int index = 0; index < Size; index++)
        {
            real[index] *= scale;
            imaginary[index] *= scale;
        }
    }

    private static uint ReverseBits(uint value)
    {
        uint result = 0;

        for (int bit = 0; bit < sizeof(uint) * BitsPerByte; bit++)
        {
            result = (result << 1) | (value & 1);
            value >>= 1;
        }

        return result;
    }

    [HotPath]
    private void Transform(Span<float> real, Span<float> imaginary, float direction)
    {
        Permute(real, imaginary);

        for (int half = 1; half < Size; half <<= 1)
        {
            int stride = Size / (half << 1);

            for (int start = 0; start < Size; start += half << 1)
            {
                for (int k = 0; k < half; k++)
                {
                    Butterfly(real, imaginary, start + k, half, k * stride, direction);
                }
            }
        }
    }

    [HotPath]
    private void Butterfly(Span<float> real, Span<float> imaginary, int top, int half, int twiddle, float direction)
    {
        int bottom = top + half;
        float wr = _cos[twiddle];
        float wi = direction * _sin[twiddle];
        float tr = (wr * real[bottom]) - (wi * imaginary[bottom]);
        float ti = (wr * imaginary[bottom]) + (wi * real[bottom]);

        real[bottom] = real[top] - tr;
        imaginary[bottom] = imaginary[top] - ti;
        real[top] += tr;
        imaginary[top] += ti;
    }

    [HotPath]
    private void Permute(Span<float> real, Span<float> imaginary)
    {
        for (int index = 0; index < Size; index++)
        {
            int other = _reversed[index];

            if (other > index)
            {
                (real[index], real[other]) = (real[other], real[index]);
                (imaginary[index], imaginary[other]) = (imaginary[other], imaginary[index]);
            }
        }
    }
}
