// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

// The BC4 and BC5 lane meanings follow the GTA6-Asset-Info notes (formats/06-bcpack.md, GPL-3.0), checked on the dump;
// see THIRD_PARTY_NOTICES.md.

using System.Runtime.CompilerServices;

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>
/// What the BC4 and BC5 block sinks share: the lane numbers, the zigzag residual, the reference distance and the
/// distance-0 prediction. Endpoints are bytes that add modulo 256; a block with distance 0 predicts 128 for every
/// endpoint.
/// </summary>
internal static class RgtcSymbols
{
    public const int DistanceLane = 0;

    /// <summary>The first endpoint lane: BC4 e0 then e1 in lanes 2 and 3; BC5 channel 0 in lane 2 and channel 1 in lane 3.</summary>
    public const int EndpointLane = 2;

    /// <summary>The first of the 8 index lanes (BC5: 4 per channel).</summary>
    public const int IndexLane = 4;

    public const int IndexLanes = 8;

    public const int RingSize = 256;

    public const uint ByteMask = 0xFF;

    public const int ByteBits = 8;

    /// <summary>Bits of a pair field: two 3-bit indices.</summary>
    public const int PairBits = 6;

    public const uint PairMask = (1u << PairBits) - 1;

    /// <summary>The prediction of distance 0, in every endpoint byte.</summary>
    public const uint Midpoint = 0x80808080;

    /// <summary>Bit position of the 48-bit index field in a BC4 block.</summary>
    public const int IndexShift = 16;

    // Spreading a nibble to the even bits: pairs of bits two apart, then single bits one apart.
    private const uint EvenBitsOfPair = 0x33;
    private const uint EvenBits = 0x55;
    private const int PairSpread = 2;

    /// <summary>The zigzag value of a code (0, -1, +1, -2, ...), modulo 256.</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    public static uint Unzigzag(uint code) => ((code >> 1) ^ (0u - (code & 1))) & ByteMask;

    /// <summary>Two 4-bit symbols interleaved bit by bit, <paramref name="even"/> on the even bits.</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    public static uint Interleave(uint even, uint odd) => Spread(even) | (Spread(odd) << 1);

    /// <summary>Adds a residual code to each endpoint byte of <paramref name="predicted"/> selected by <paramref name="shift"/>.</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    public static uint AddResidual(uint predicted, int shift, uint code) =>
        (((predicted >> shift) + Unzigzag(code)) & ByteMask) << shift;

    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    private static uint Spread(uint nibble)
    {
        var x = (nibble | (nibble << PairSpread)) & EvenBitsOfPair;

        return (x | (x << 1)) & EvenBits;
    }
}
