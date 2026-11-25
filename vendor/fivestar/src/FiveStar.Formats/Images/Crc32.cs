// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Images;

/// <summary>The CRC-32 of PNG chunks and zip (reflected polynomial 0xEDB88320, initial and final XOR 0xFFFFFFFF).</summary>
public static class Crc32
{
    private const uint Polynomial = 0xEDB88320;
    private const int TableSize = 256;
    private const int BitsPerByte = 8;

    private static readonly uint[] Table = BuildTable();

    public static uint Compute(ReadOnlySpan<byte> data)
    {
        var crc = uint.MaxValue;

        foreach (var value in data)
        {
            crc = Table[(crc ^ value) & byte.MaxValue] ^ (crc >> BitsPerByte);
        }

        return ~crc;
    }

    private static uint[] BuildTable()
    {
        var table = new uint[TableSize];

        for (uint index = 0; index < TableSize; index++)
        {
            var value = index;

            for (var bit = 0; bit < BitsPerByte; bit++)
            {
                var low = value & 1;
                value >>= 1;

                if (low != 0)
                {
                    value ^= Polynomial;
                }
            }

            table[index] = value;
        }

        return table;
    }
}
