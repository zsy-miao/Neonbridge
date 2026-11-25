// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Audio;

/// <summary>The CRC-32 of an Ogg page (RFC 3533): polynomial 0x04C11DB7, most significant bit first, initial value 0.</summary>
public static class OggCrc
{
    private const uint Polynomial = 0x04C11DB7;
    private const uint TopBit = 0x80000000;
    private const int ByteBits = 8;
    private const int TopByteShift = 24;
    private const int TableSize = 256;

    private static readonly uint[] Table = BuildTable();

    public static uint Compute(ReadOnlySpan<byte> data)
    {
        uint crc = 0;

        foreach (var value in data)
        {
            crc = (crc << ByteBits) ^ Table[(byte)(crc >> TopByteShift) ^ value];
        }

        return crc;
    }

    private static uint[] BuildTable()
    {
        var table = new uint[TableSize];

        for (var index = 0; index < TableSize; index++)
        {
            var crc = (uint)index << TopByteShift;

            for (var bit = 0; bit < ByteBits; bit++)
            {
                crc = (crc & TopBit) != 0 ? (crc << 1) ^ Polynomial : crc << 1;
            }

            table[index] = crc;
        }

        return table;
    }
}
