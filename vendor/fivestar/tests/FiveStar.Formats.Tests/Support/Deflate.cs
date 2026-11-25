// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.IO.Compression;

namespace FiveStar.Formats.Tests.Support;

/// <summary>Test data: zlib streams made by the runtime's compressor and incompressible noise.</summary>
public static class Deflate
{
    public static byte[] Zlib(byte[] data)
    {
        using var output = new MemoryStream();

        using (var zlib = new ZLibStream(output, CompressionLevel.SmallestSize))
        {
            zlib.Write(data);
        }

        return output.ToArray();
    }

    /// <summary>Bytes from a fixed linear congruential sequence: the same every run, and incompressible.</summary>
    public static byte[] Noise(int length, uint seed)
    {
        var bytes = new byte[length];
        var state = seed;

        for (var index = 0; index < length; index++)
        {
            state = (state * 1664525) + 1013904223;
            bytes[index] = (byte)(state >> 24);
        }

        return bytes;
    }

    /// <summary>Text-like bytes that compress well.</summary>
    public static byte[] Pattern(int length)
    {
        var bytes = new byte[length];

        for (var index = 0; index < length; index++)
        {
            bytes[index] = (byte)('a' + (index % 7) + (index / 4096 % 3));
        }

        return bytes;
    }
}
