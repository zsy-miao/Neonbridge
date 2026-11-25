// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Tests.Support;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>The chad.dll (XDsp) convolution on the CPU, driven through the shims as the game's convolution effect drives them.</summary>
public sealed class XDspTests : IDisposable
{
    private const int ResponseFrames = 2500;
    private const int Blocks = 5;

    private readonly TestRuntime _test = TestRuntime.Create();

    [Theory]
    [InlineData(1, 1024)]
    [InlineData(2, 256)]
    public void A_convolution_stream_outputs_the_direct_convolution_of_its_blocks_with_the_response(int channels, int frames)
    {
        // A decaying noise response per channel, the channels one after the other; noise input.
        var response = new float[ResponseFrames * channels];

        for (int index = 0; index < response.Length; index++)
        {
            response[index] = Noise(index + 7) * MathF.Exp(-(index % ResponseFrames) / 600f);
        }

        var input = new float[channels][];

        for (int channel = 0; channel < channels; channel++)
        {
            input[channel] = new float[Blocks * frames];

            for (int index = 0; index < input[channel].Length; index++)
            {
                input[channel][index] = Noise((channel * 100_000) + index);
            }
        }

        _test.Runtime.XDsp.Convolve = true;
        using var driver = new XDspDriver(_test, response, channels, frames);
        var output = Run(driver, input, frames);

        for (int channel = 0; channel < channels; channel++)
        {
            var expected = Direct(input[channel], response.AsSpan(channel * ResponseFrames, ResponseFrames));

            for (int index = 0; index < expected.Length; index++)
            {
                Assert.InRange(output[channel][index] - expected[index], -1e-3, 1e-3);
            }
        }
    }

    public void Dispose()
    {
        _test.Dispose();
    }

    /// <summary>Submits the input block by block and joins the output blocks per channel.</summary>
    private static float[][] Run(XDspDriver driver, float[][] input, int frames)
    {
        var output = new float[input.Length][];

        for (int channel = 0; channel < input.Length; channel++)
        {
            output[channel] = new float[input[channel].Length];
        }

        for (int block = 0; block < Blocks; block++)
        {
            var blockInput = input.Select(samples => samples.AsSpan(block * frames, frames).ToArray()).ToArray();
            var blockOutput = driver.Process(blockInput);

            for (int channel = 0; channel < input.Length; channel++)
            {
                blockOutput[channel].CopyTo(output[channel], block * frames);
            }
        }

        return output;
    }

    /// <summary>The first samples of the linear convolution of <paramref name="signal"/> with <paramref name="response"/>, in double precision.</summary>
    private static double[] Direct(float[] signal, ReadOnlySpan<float> response)
    {
        var result = new double[signal.Length];

        for (int index = 0; index < signal.Length; index++)
        {
            double sum = 0;

            for (int tap = 0; tap < response.Length && tap <= index; tap++)
            {
                sum += (double)response[tap] * signal[index - tap];
            }

            result[index] = sum;
        }

        return result;
    }

    /// <summary>Deterministic noise in [-0.5, 0.5].</summary>
    private static float Noise(int index)
    {
        uint value = (uint)index * 2654435761u;
        value ^= value >> 15;
        value *= 2246822519u;
        value ^= value >> 13;

        return (value / (float)uint.MaxValue) - 0.5f;
    }
}
