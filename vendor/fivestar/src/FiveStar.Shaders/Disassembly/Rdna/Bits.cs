// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>Field extraction from instruction dwords.</summary>
internal static class Bits
{
    /// <summary>The <paramref name="count"/> bits of <paramref name="word"/> starting at bit <paramref name="low"/>.</summary>
    public static int Field(uint word, int low, int count)
    {
        return (int)((word >> low) & ((1u << count) - 1));
    }

    /// <summary>Whether bit <paramref name="bit"/> of <paramref name="word"/> is set.</summary>
    public static bool Flag(uint word, int bit)
    {
        return ((word >> bit) & 1) != 0;
    }

    /// <summary>A field read as a two's-complement signed value.</summary>
    public static int Signed(uint word, int low, int count)
    {
        var value = Field(word, low, count);
        var sign = 1 << (count - 1);

        return (value ^ sign) - sign;
    }
}
