// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text;

namespace FiveStar.Formats.Pso;

/// <summary>Jenkins one-at-a-time hash of the lower-cased UTF-8 text: the name hash of PSO structs, members and values.</summary>
public static class Joaat
{
    private const int MixShift1 = 10;
    private const int MixShift2 = 6;
    private const int FinalShift1 = 3;
    private const int FinalShift2 = 11;
    private const int FinalShift3 = 15;

    public static uint Hash(string text)
    {
        return HashBytes(Encoding.UTF8.GetBytes(text.ToLowerInvariant()));
    }

    public static uint HashBytes(ReadOnlySpan<byte> bytes)
    {
        uint hash = 0;

        foreach (var value in bytes)
        {
            hash += value;
            hash += hash << MixShift1;
            hash ^= hash >> MixShift2;
        }

        hash += hash << FinalShift1;
        hash ^= hash >> FinalShift2;
        hash += hash << FinalShift3;

        return hash;
    }
}
