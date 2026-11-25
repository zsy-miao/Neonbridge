// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Compression;

/// <summary>The Adler-32 checksum of RFC 1950 section 8.2.</summary>
internal static class Adler32
{
    private const uint Modulus = 65521;

    // The most bytes summed before the 32-bit sums can overflow (zlib's NMAX).
    private const int MaxRun = 5552;

    private const int SumBits = 16;

    public static uint Compute(ReadOnlySpan<byte> data)
    {
        uint low = 1;
        uint high = 0;

        while (data.Length > 0)
        {
            var run = Math.Min(data.Length, MaxRun);

            foreach (var value in data[..run])
            {
                low += value;
                high += low;
            }

            low %= Modulus;
            high %= Modulus;
            data = data[run..];
        }

        return (high << SumBits) | low;
    }
}
