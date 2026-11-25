// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>
/// The six endpoint fields R0 G0 B0 R1 G1 B1 of a BC1 block in its first 32 bits, <c>c0 | c1 &lt;&lt; 16</c> with each
/// colour RGB 5:6:5. Lanes 2-7 of a BC1 page code one residual per field.
/// </summary>
internal static class Bc1Fields
{
    public const int Count = 6;

    /// <summary>The top bit of each field: an add that keeps these apart wraps every field on its own.</summary>
    public const uint TopBits = 0x84108410;

    /// <summary>Bit position of each field.</summary>
    public static readonly int[] Shift = [11, 5, 0, 27, 21, 16];

    /// <summary>Bits of each field.</summary>
    public static readonly int[] Width = [5, 6, 5, 5, 6, 5];
}
