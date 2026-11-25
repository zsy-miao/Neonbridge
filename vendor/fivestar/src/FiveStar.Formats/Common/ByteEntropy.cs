// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Common;

/// <summary>Shannon entropy of a byte string in bits per byte (0 for random-free data, 8 for uniform noise).</summary>
public static class ByteEntropy
{
    private const int ByteValues = 256;

    public static double BitsPerByte(ReadOnlySpan<byte> data)
    {
        if (data.IsEmpty)
        {
            return 0;
        }

        Span<int> counts = stackalloc int[ByteValues];
        Span<byte> order = stackalloc byte[ByteValues];
        var distinct = 0;

        foreach (var value in data)
        {
            if (counts[value] == 0)
            {
                order[distinct] = value;
                distinct++;
            }

            counts[value]++;
        }

        // Terms are taken in first-seen order so the last digits do not depend on the byte values.
        double entropy = 0;

        for (var index = 0; index < distinct; index++)
        {
            var share = (double)counts[order[index]] / data.Length;
            entropy -= share * Math.Log2(share);
        }

        return entropy;
    }
}
