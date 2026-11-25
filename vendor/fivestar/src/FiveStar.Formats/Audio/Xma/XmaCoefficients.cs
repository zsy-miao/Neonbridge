// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Audio.Xma;

/// <summary>
/// Reads one channel's quantised coefficients of a subframe. Low frequencies come as vectors of four (escaping to two,
/// then to single values), each non-zero value followed by its sign bit; once enough zeros have been seen in a row, or
/// past the vector-coded count, the rest is run-level coded until an end code.
/// </summary>
internal static class XmaCoefficients
{
    private const int VectorWidth = 4;
    private const int PairWidth = 2;
    private const int NibbleBits = 4;
    private const int NibbleMask = 0xF;

    /// <summary>Zeros in a row that switch to run-level coding: more than the subframe length / 256.</summary>
    private const int ZeroRunShift = 8;

    /// <summary>A large value has 8, 16, 24 or 31 bits, chosen by up to three leading 1 bits.</summary>
    private const int LargeValueStep = 8;
    private const int LargeValueLastStep = 7;

    /// <summary>A run-level escape's short run extension: 2 bits plus one.</summary>
    private const int ShortRunBits = 2;

    /// <summary>A run-level escape's long run extension adds this to the run read.</summary>
    private const int LongRunBase = 4;

    private const int BrokenRun = -1;

    /// <summary>
    /// Reads the coefficients into <paramref name="coefficients"/> (the subframe length). The first
    /// <paramref name="vectorCount"/> may be vector coded; when that count was sent (<paramref name="vectorCountSent"/>),
    /// vector coding continues past runs of zeros.
    /// </summary>
    [HotPath]
    public static void Read(ref XmaBitReader reader, Span<float> coefficients, int vectorCount, bool vectorCountSent)
    {
        var useCode1 = reader.ReadBit();
        var next = ReadVectors(ref reader, coefficients, vectorCount, vectorCountSent);

        if (next >= coefficients.Length)
        {
            return;
        }

        coefficients[next..].Clear();

        if (useCode1)
        {
            ReadRunLevel(ref reader, coefficients, next, WmaProCodebooks.Coefficient1, WmaProTables.Coefficient1Runs, WmaProTables.Coefficient1Levels);
        }
        else
        {
            ReadRunLevel(ref reader, coefficients, next, WmaProCodebooks.Coefficient0, WmaProTables.Coefficient0Runs, WmaProTables.Coefficient0Levels);
        }
    }

    /// <summary>Reads vectors of four; returns the index of the first coefficient not read.</summary>
    [HotPath]
    private static int ReadVectors(ref XmaBitReader reader, Span<float> coefficients, int vectorCount, bool vectorCountSent)
    {
        Span<int> values = stackalloc int[VectorWidth];
        var zeroLimit = coefficients.Length >> ZeroRunShift;
        var zeros = 0;
        var runLevel = false;
        var index = 0;

        while ((vectorCountSent || !runLevel) && index + VectorWidth <= vectorCount)
        {
            ReadVector(ref reader, values);

            for (var lane = 0; lane < VectorWidth; lane++)
            {
                var value = values[lane];

                if (value != 0)
                {
                    coefficients[index] = reader.ReadBit() ? value : -value;
                    zeros = 0;
                }
                else
                {
                    coefficients[index] = 0;
                    zeros++;
                    runLevel |= zeros > zeroLimit;
                }

                index++;
            }
        }

        return index;
    }

    [HotPath]
    private static void ReadVector(ref XmaBitReader reader, scoped Span<int> values)
    {
        var symbol = WmaProCodebooks.Vector4.Decode(ref reader);

        if (symbol != WmaProCodebooks.VectorEscape)
        {
            var packed = symbol - 1;

            for (var lane = VectorWidth - 1; lane >= 0; lane--)
            {
                values[lane] = packed & NibbleMask;
                packed >>= NibbleBits;
            }

            return;
        }

        ReadPair(ref reader, values[..PairWidth]);
        ReadPair(ref reader, values[PairWidth..]);
    }

    [HotPath]
    private static void ReadPair(ref XmaBitReader reader, scoped Span<int> values)
    {
        var symbol = WmaProCodebooks.Vector2.Decode(ref reader);

        if (symbol != WmaProCodebooks.VectorEscape)
        {
            values[0] = (symbol - 1) >> NibbleBits;
            values[1] = (symbol - 1) & NibbleMask;

            return;
        }

        values[0] = ReadSingle(ref reader);
        values[1] = ReadSingle(ref reader);
    }

    [HotPath]
    private static int ReadSingle(ref XmaBitReader reader)
    {
        var value = WmaProCodebooks.Vector1.Decode(ref reader);

        if (value == WmaProCodebooks.Vector1Escape)
        {
            value += (int)ReadLargeValue(ref reader);
        }

        return value;
    }

    /// <summary>Run-level coding from <paramref name="start"/>: runs of zeros, each ended by a signed level.</summary>
    [HotPath]
    private static void ReadRunLevel(ref XmaBitReader reader, Span<float> coefficients, int start, HuffmanCode code, ushort[] runs, byte[] levels)
    {
        var mask = coefficients.Length - 1;
        var runBits = System.Numerics.BitOperations.Log2((uint)coefficients.Length - 1) + 1;

        for (var index = start; index < coefficients.Length; index++)
        {
            var symbol = code.Decode(ref reader);
            float level;

            if (symbol > WmaProCodebooks.RunLevelEnd)
            {
                index += runs[symbol];
                level = levels[symbol];
            }
            else if (symbol == WmaProCodebooks.RunLevelEscape)
            {
                level = ReadLargeValue(ref reader);
                var run = EscapedRun(ref reader, runBits);

                if (run < 0)
                {
                    return;
                }

                index += run;
            }
            else
            {
                return;
            }

            coefficients[index & mask] = reader.ReadBit() ? level : -level;
        }
    }

    /// <summary>The extra run after an escaped level: none (0), 1 to 4 (10 and 2 bits), longer (110 and the run bits); 111 is broken.</summary>
    [HotPath]
    private static int EscapedRun(ref XmaBitReader reader, int runBits)
    {
        if (!reader.ReadBit())
        {
            return 0;
        }

        if (!reader.ReadBit())
        {
            return (int)reader.Read(ShortRunBits) + 1;
        }

        if (reader.ReadBit())
        {
            return BrokenRun;
        }

        return (int)reader.Read(runBits) + LongRunBase;
    }

    [HotPath]
    private static uint ReadLargeValue(ref XmaBitReader reader)
    {
        var bits = LargeValueStep;

        if (reader.ReadBit())
        {
            bits += LargeValueStep;

            if (reader.ReadBit())
            {
                bits += LargeValueStep;

                if (reader.ReadBit())
                {
                    bits += LargeValueLastStep;
                }
            }
        }

        return reader.Read(bits);
    }
}
