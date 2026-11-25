// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>
/// The static rANS model tables of the lane decoder, from <c>mdu_minirans.v</c> of Microsoft silicon-rans (MIT; see
/// THIRD_PARTY_NOTICES.md). Each array is flattened as <c>[alphabet * 160 + model * 10 + segment]</c>: alphabet index
/// 0-8 (symbol bits - 1), model q 0-15, segment 0 for symbol 0 and k for the symbols 2^(k-1) to 2^k - 1.
/// </summary>
/// <param name="Base">Cumulative frequency where each segment starts (the inverse's thresholds).</param>
/// <param name="Freq">Frequency of each symbol of the segment.</param>
/// <param name="Rf">Reciprocal of the frequency for the inverse's divide.</param>
/// <param name="Rs">Extra right shift of the reciprocal divide.</param>
/// <param name="Rn">Rounding offset of the reciprocal divide.</param>
internal sealed record MduModelTable(ushort[] Base, ushort[] Freq, uint[] Rf, byte[] Rs, ushort[] Rn)
{
    public const int AlphabetIndices = 9;

    public const int ModelsPerAlphabet = 16;

    public const int Segments = 10;

    public const int AlphabetStride = ModelsPerAlphabet * Segments;

    public const int Entries = AlphabetIndices * AlphabetStride;

    /// <summary>Where the segments of a model start in the flattened arrays.</summary>
    public static int Row(int alphabet, int model) => (alphabet * AlphabetStride) + (model * Segments);
}
