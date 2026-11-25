// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Compression;

/// <summary>The fixed tables of the deflate format (RFC 1951 section 3.2.5 to 3.2.7).</summary>
internal static class DeflateTables
{
    /// <summary>Literal/length symbols a dynamic block may declare (HLIT + 257 at most 286).</summary>
    public const int MaxLiteralCodes = 286;

    /// <summary>Distance symbols a dynamic block may declare (HDIST + 1 at most 30).</summary>
    public const int MaxDistanceCodes = 30;

    /// <summary>Literal/length symbols of the fixed code, 286 and 287 included (never valid in data).</summary>
    public const int FixedLiteralCodes = 288;

    /// <summary>Distance symbols of the fixed code, 30 and 31 included (never valid in data).</summary>
    public const int FixedDistanceCodes = 32;

    /// <summary>The symbol that ends a block.</summary>
    public const int EndOfBlock = 256;

    /// <summary>The first length symbol.</summary>
    public const int FirstLengthSymbol = 257;

    /// <summary>Symbols of the code-length code.</summary>
    public const int CodeLengthCodes = 19;

    /// <summary>Bits per code-length code length.</summary>
    public const int CodeLengthBits = 3;

    /// <summary>Code-length symbol 16: repeat the previous length 3 to 6 times (2 extra bits).</summary>
    public const int RepeatPrevious = 16;

    /// <summary>Code-length symbol 17: 3 to 10 zero lengths (3 extra bits).</summary>
    public const int RepeatZeroShort = 17;

    /// <summary>Code-length symbol 18: 11 to 138 zero lengths (7 extra bits).</summary>
    public const int RepeatZeroLong = 18;

    public const int RepeatPreviousBase = 3;
    public const int RepeatPreviousExtraBits = 2;
    public const int RepeatZeroShortBase = 3;
    public const int RepeatZeroShortExtraBits = 3;
    public const int RepeatZeroLongBase = 11;
    public const int RepeatZeroLongExtraBits = 7;

    /// <summary>Order in which the code-length code lengths are stored.</summary>
    public static readonly byte[] CodeLengthOrder = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

    /// <summary>Base match length of length symbols 257..285.</summary>
    public static readonly ushort[] LengthBase =
    [
        3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258,
    ];

    /// <summary>Extra bits of length symbols 257..285.</summary>
    public static readonly byte[] LengthExtra =
    [
        0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
    ];

    /// <summary>Base distance of distance symbols 0..29.</summary>
    public static readonly ushort[] DistanceBase =
    [
        1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097,
        6145, 8193, 12289, 16385, 24577,
    ];

    /// <summary>Extra bits of distance symbols 0..29.</summary>
    public static readonly byte[] DistanceExtra =
    [
        0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13,
    ];

    /// <summary>Code lengths of the fixed literal/length code: 0-143 = 8, 144-255 = 9, 256-279 = 7, 280-287 = 8.</summary>
    public static readonly byte[] FixedLiteralLengths = BuildFixedLiteralLengths();

    /// <summary>Code lengths of the fixed distance code: all 5.</summary>
    public static readonly byte[] FixedDistanceLengths = BuildFixedDistanceLengths();

    private static byte[] BuildFixedLiteralLengths()
    {
        const int EightBitEnd = 144;
        const int NineBitEnd = 256;
        const int SevenBitEnd = 280;
        const byte Seven = 7;
        const byte Eight = 8;
        const byte Nine = 9;

        var lengths = new byte[FixedLiteralCodes];
        lengths.AsSpan(0, EightBitEnd).Fill(Eight);
        lengths.AsSpan(EightBitEnd, NineBitEnd - EightBitEnd).Fill(Nine);
        lengths.AsSpan(NineBitEnd, SevenBitEnd - NineBitEnd).Fill(Seven);
        lengths.AsSpan(SevenBitEnd).Fill(Eight);

        return lengths;
    }

    private static byte[] BuildFixedDistanceLengths()
    {
        const byte Five = 5;

        var lengths = new byte[FixedDistanceCodes];
        lengths.AsSpan().Fill(Five);

        return lengths;
    }
}
