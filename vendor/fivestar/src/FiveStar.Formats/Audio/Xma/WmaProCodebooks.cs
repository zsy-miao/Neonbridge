// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Audio.Xma;

/// <summary>The prefix codes of <see cref="WmaProTables"/>, built once; they are read-only after construction.</summary>
internal static class WmaProCodebooks
{
    /// <summary>DPCM scale factor differences: the symbol minus <see cref="ScaleBias"/>.</summary>
    public static readonly HuffmanCode Scale = HuffmanCode.FromPairs(WmaProTables.ScaleCodes);

    /// <summary>Run-level scale factor differences: 0 escapes, 1 ends, others index the run and level tables.</summary>
    public static readonly HuffmanCode ScaleRunLevel = HuffmanCode.FromPairs(WmaProTables.ScaleRunLevelCodes);

    /// <summary>Run-level coefficients, code 0: 0 escapes, 1 ends, others index the run and level tables.</summary>
    public static readonly HuffmanCode Coefficient0 = new(
        [.. WmaProTables.Coefficient0Lengths.Select(length => (int)length)],
        [.. WmaProTables.Coefficient0Symbols.Select(symbol => (int)symbol)]);

    /// <summary>Run-level coefficients, code 1.</summary>
    public static readonly HuffmanCode Coefficient1 = HuffmanCode.FromPairs(WmaProTables.Coefficient1Codes);

    /// <summary>Four coefficients of 0 to 15: 0 escapes to two <see cref="Vector2"/> codes.</summary>
    public static readonly HuffmanCode Vector4 = new(
        [.. WmaProTables.Vector4Lengths.Select(length => (int)length)],
        [.. WmaProTables.Vector4Symbols.Select(symbol => (int)symbol)]);

    /// <summary>Two coefficients of 0 to 15: 0 escapes to two <see cref="Vector1"/> codes.</summary>
    public static readonly HuffmanCode Vector2 = HuffmanCode.FromPairs(WmaProTables.Vector2Codes);

    /// <summary>One coefficient; <see cref="Vector1Escape"/> adds a large value read after it.</summary>
    public static readonly HuffmanCode Vector1 = HuffmanCode.FromPairs(WmaProTables.Vector1Codes);

    /// <summary>The DPCM scale factor symbol that means a difference of zero (wmaprodata.h: the code's offset of -60).</summary>
    public const int ScaleBias = 60;

    /// <summary>The run-level symbols with a meaning of their own.</summary>
    public const int RunLevelEscape = 0;

    public const int RunLevelEnd = 1;

    /// <summary>The vector symbol that escapes to a smaller vector; other symbols hold their values plus one.</summary>
    public const int VectorEscape = 0;

    /// <summary>The <see cref="Vector1"/> symbol (the last of 101) whose value continues with a large value.</summary>
    public const int Vector1Escape = 100;
}
